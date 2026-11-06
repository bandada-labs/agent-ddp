package dev.agentddp

import java.io.ByteArrayOutputStream
import java.io.EOFException
import java.io.InputStream
import java.io.OutputStream
import java.security.MessageDigest
import java.util.Base64

// Shared by the library and the runner APK (android/runner): one copy of the control endpoint's wire.

internal class HttpRequest(val path: String, val headers: Map<String, String>) {
    companion object {
        private const val MAX_HEADER_BYTES = 16_384

        /** Reads up to the blank line and no further, so WebSocket frames that follow stay in the stream. */
        fun read(input: InputStream): HttpRequest? {
            val lines = mutableListOf<String>()
            val line = ByteArrayOutputStream()
            var total = 0
            while (total++ < MAX_HEADER_BYTES) {
                val byte = input.read()
                if (byte < 0) return null
                if (byte != '\n'.code) {
                    if (byte != '\r'.code) line.write(byte)
                    continue
                }
                if (line.size() == 0) break
                lines += line.toString()
                line.reset()
            }
            val target = lines.firstOrNull()?.split(" ")?.getOrNull(1) ?: return null
            val headers = lines.drop(1).mapNotNull { header ->
                header.split(":", limit = 2).takeIf { it.size == 2 }?.let { (name, value) -> name.trim().lowercase() to value.trim() }
            }.toMap()
            return HttpRequest(target.substringBefore("?"), headers)
        }
    }
}

/** RFC 6455, only what the control endpoint needs: text, ping, pong and close frames. Clients mask, the server does not. */
internal object WebSocket {
    const val CONTINUATION = 0
    const val TEXT = 1
    const val CLOSE = 8
    const val PING = 9
    const val PONG = 10
    private const val MAX_MESSAGE_BYTES = 64 * 1024 * 1024
    private const val GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"

    class Frame(val opcode: Int, val payload: ByteArray)

    fun acceptKey(key: String): String =
        Base64.getEncoder().encodeToString(MessageDigest.getInstance("SHA-1").digest((key + GUID).toByteArray()))

    /** One message: continuation frames are joined, so the caller always sees whole messages. */
    fun readFrame(input: InputStream): Frame {
        var opcode = -1
        val message = ByteArrayOutputStream()
        while (true) {
            val first = input.readByte()
            val second = input.readByte()
            var length = (second and 0x7F).toLong()
            if (length == 126L) length = input.readNumber(2)
            if (length == 127L) length = input.readNumber(8)
            if (message.size() + length > MAX_MESSAGE_BYTES) throw EOFException("message too large")
            val mask = if (second and 0x80 != 0) ByteArray(4) { input.readByte().toByte() } else null
            val payload = ByteArray(length.toInt()) { input.readByte().toByte() }
            mask?.let { payload.indices.forEach { index -> payload[index] = (payload[index].toInt() xor mask[index % 4].toInt()).toByte() } }
            val frameOpcode = first and 0x0F
            // Control frames may arrive between the pieces of a fragmented message.
            if (frameOpcode >= CLOSE) return Frame(frameOpcode, payload)
            if (frameOpcode != CONTINUATION) opcode = frameOpcode
            message.write(payload)
            if (first and 0x80 != 0) return Frame(opcode, message.toByteArray())
        }
    }

    fun writeText(output: OutputStream, text: String) = write(output, TEXT, text.toByteArray())

    fun write(output: OutputStream, opcode: Int, payload: ByteArray) {
        output.write(0x80 or opcode)
        when {
            payload.size < 126 -> output.write(payload.size)
            payload.size < 65_536 -> {
                output.write(126)
                output.write(payload.size shr 8)
                output.write(payload.size and 0xFF)
            }
            else -> {
                output.write(127)
                (7 downTo 0).forEach { output.write((payload.size.toLong() shr (it * 8)).toInt() and 0xFF) }
            }
        }
        output.write(payload)
        output.flush()
    }

    private fun InputStream.readByte(): Int = read().also { if (it < 0) throw EOFException() }

    private fun InputStream.readNumber(bytes: Int): Long = (0 until bytes).fold(0L) { number, _ -> (number shl 8) or readByte().toLong() }
}
