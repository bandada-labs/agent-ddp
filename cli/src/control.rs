//! The app's control endpoint (ADR 0008): JSON-RPC over a WebSocket on 127.0.0.1. The server is the library in the
//! app, so this client only speaks what it sends: unfragmented text frames, ping and close.
use crate::fail;
use crate::py;
use crate::state::{failure, Result};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const TEXT: u8 = 1;
const CLOSE: u8 = 8;
const PING: u8 = 9;
const PONG: u8 = 10;
const TIMEOUT: Duration = Duration::from_secs(10);

pub struct Control {
    stream: TcpStream,
    reader: BufReader<TcpStream>,
    session: Value,
    next_id: u64,
    events: VecDeque<Value>,
}

impl Control {
    pub fn open(session: &Value) -> Result<Self> {
        let port = session["port"].as_u64().unwrap_or(0) as u16;
        let unreachable = |error: String| {
            failure(
                format!("can't reach the app ({error})"),
                "unreachable",
                Some(&format!(
                    "is it running? ddp connect {} --app {}, or ddp doctor",
                    py::str(&session["device"]),
                    py::str(&session["app"])
                )),
            )
        };
        let stream = TcpStream::connect_timeout(&SocketAddr::from(([127, 0, 0, 1], port)), TIMEOUT)
            .map_err(|e| unreachable(e.to_string()))?;
        let _ = stream.set_read_timeout(Some(TIMEOUT));
        let _ = stream.set_nodelay(true);
        let reader = BufReader::new(stream.try_clone().map_err(|e| unreachable(e.to_string()))?);
        let mut control = Control { stream, reader, session: session.clone(), next_id: 1, events: VecDeque::new() };
        // The key only proves the server speaks WebSocket; ours does, so a fixed one is enough.
        let request = format!(
            "GET / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
             Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n"
        );
        control.stream.write_all(request.as_bytes()).map_err(|e| unreachable(e.to_string()))?;
        let mut status = String::new();
        // adb forward accepts the connection even when nothing listens in the app, then closes it.
        if control.reader.read_line(&mut status).unwrap_or(0) == 0 {
            return Err(unreachable("the app closed the connection without answering".into()));
        }
        if !status.starts_with("HTTP/1.1 101") {
            return Err(unreachable(format!("the app answered {}", status.trim())));
        }
        loop {
            let mut line = String::new();
            if control.reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                break;
            }
        }
        Ok(control)
    }

    /// Sends a request and waits for its answer. Events that arrive first are kept for `next_event`.
    pub fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({"id": id, "method": method, "params": params}))?;
        loop {
            let Some(message) = self.read_message()? else {
                fail!(format!("the app closed the connection during {method}"), "unreachable", self.doctor());
            };
            if message.get("id").and_then(Value::as_u64) != Some(id) {
                if message.get("method").is_some() {
                    self.events.push_back(message);
                }
                continue;
            }
            if let Some(error) = message.get("error") {
                fail!(py::str(&error["message"]), "app_error");
            }
            return Ok(message["result"].clone());
        }
    }

    /// The next event (`{method, params}`), or None when the app closed the connection or the deadline passed.
    pub fn next_event(&mut self, deadline: Option<Instant>) -> Result<Option<Value>> {
        if let Some(event) = self.events.pop_front() {
            return Ok(Some(event));
        }
        let wait = match deadline {
            Some(deadline) => match deadline.checked_duration_since(Instant::now()) {
                Some(left) if !left.is_zero() => Some(left),
                _ => return Ok(None),
            },
            None => None,
        };
        let _ = self.stream.set_read_timeout(wait);
        let message = self.read_message();
        let _ = self.stream.set_read_timeout(Some(TIMEOUT));
        match message {
            // The stream is over either way, so a frame cut by the deadline does not matter.
            Err(error) if error.code == "timeout" => Ok(None),
            other => other,
        }
    }

    pub fn close(&self) {
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
    }

    fn doctor(&self) -> String {
        format!(
            "is the app running? ddp doctor {} --app {}",
            py::str(&self.session["device"]),
            py::str(&self.session["app"])
        )
    }

    fn send(&mut self, message: &Value) -> Result<()> {
        let frame = Self::frame(TEXT, py::dumps(message).as_bytes());
        self.stream.write_all(&frame).map_err(|e| failure(format!("can't send to the app ({e})"), "unreachable", None))
    }

    /// One JSON message, answering pings on the way. None when the app closed the connection.
    fn read_message(&mut self) -> Result<Option<Value>> {
        loop {
            let (opcode, payload) = match self.read_frame() {
                Ok(frame) => frame,
                Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                    fail!("the app did not answer in time", "timeout", self.doctor());
                }
                Err(_) => return Ok(None),
            };
            match opcode {
                TEXT => {
                    return serde_json::from_slice(&payload).map(Some).map_err(|e| {
                        failure(format!("the app sent something that is not JSON ({e})"), "app_error", None)
                    })
                }
                PING => {
                    let _ = self.stream.write_all(&Self::frame(PONG, &payload));
                }
                CLOSE => return Ok(None),
                _ => {}
            }
        }
    }

    fn read_frame(&mut self) -> std::io::Result<(u8, Vec<u8>)> {
        let mut head = [0u8; 2];
        self.reader.read_exact(&mut head)?;
        let length = match head[1] & 0x7f {
            126 => {
                let mut bytes = [0u8; 2];
                self.reader.read_exact(&mut bytes)?;
                u16::from_be_bytes(bytes) as usize
            }
            127 => {
                let mut bytes = [0u8; 8];
                self.reader.read_exact(&mut bytes)?;
                u64::from_be_bytes(bytes) as usize
            }
            short => short as usize,
        };
        let mut mask = [0u8; 4];
        if head[1] & 0x80 != 0 {
            self.reader.read_exact(&mut mask)?;
        }
        let mut payload = vec![0u8; length];
        self.reader.read_exact(&mut payload)?;
        payload.iter_mut().enumerate().for_each(|(i, byte)| *byte ^= mask[i % 4]);
        Ok((head[0] & 0x0f, payload))
    }

    /// A client frame is always masked (RFC 6455). The mask hides nothing here; any value will do.
    fn frame(opcode: u8, payload: &[u8]) -> Vec<u8> {
        let mask = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0).to_be_bytes();
        let mut frame = vec![0x80 | opcode];
        match payload.len() {
            length if length < 126 => frame.push(0x80 | length as u8),
            length if length <= u16::MAX as usize => {
                frame.push(0x80 | 126);
                frame.extend_from_slice(&(length as u16).to_be_bytes());
            }
            length => {
                frame.push(0x80 | 127);
                frame.extend_from_slice(&(length as u64).to_be_bytes());
            }
        }
        frame.extend_from_slice(&mask);
        frame.extend(payload.iter().enumerate().map(|(i, byte)| byte ^ mask[i % 4]));
        frame
    }
}

#[cfg(test)]
mod tests {
    use super::Control;

    #[test]
    fn a_client_frame_is_masked_and_sized() {
        let payload = vec![b'x'; 300];
        let frame = Control::frame(1, &payload);
        assert_eq!(frame[0], 0x81);
        assert_eq!(frame[1], 0x80 | 126);
        assert_eq!(u16::from_be_bytes([frame[2], frame[3]]), 300);
        let mask = &frame[4..8];
        let unmasked: Vec<u8> = frame[8..].iter().enumerate().map(|(i, b)| b ^ mask[i % 4]).collect();
        assert_eq!(unmasked, payload);
    }
}
