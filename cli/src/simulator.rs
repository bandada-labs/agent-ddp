//! iOS simulator apps without the library, driven from the Mac. Touches and keys go into the simulator's own HID
//! service (`dtuhidd`); elements come from its accessibility service. Both are XPC endpoints CoreSimulator hands out for
//! a booted device, so nothing runs in the simulator and there is no XCTest. Every piece is private (docs/hooks.md,
//! "Mac host"): each request waits for the simulator's answer, and no answer is an error naming the hook.
use crate::fail;
use crate::state::{capture, failure, Result};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::ffi::{c_char, c_void, CStr, CString};
use std::sync::mpsc;
use std::time::Duration;

pub const HID_HOOK: &str = "simulator.hid";
pub const ACCESSIBILITY_HOOK: &str = "simulator.accessibility";
const HID_SERVICE: &str = "com.apple.coredevice.feature.remote.hid.digitizer";
const ACCESSIBILITY_SERVICE: &str = "com.apple.CoreSimulator.accessibility";
/// dtuhidd answers in under a millisecond; a second means it is gone or still starting.
const HID_ANSWER: Duration = Duration::from_secs(2);
/// One element read; idb waits as long. A busy app can be slow to answer its accessibility.
const ACCESSIBILITY_ANSWER: Duration = Duration::from_secs(5);
const BOOTED: u64 = 3;
const DRAIN: Duration = Duration::from_millis(80);
/// Finger phases dtuhidd takes.
pub const START: u64 = 0;
pub const MOVE: u64 = 1;
pub const END: u64 = 2;
/// The translator's flat tree, what VoiceOver reads: one element per thing a person can act on or read.
/// The flat tree, one element per VoiceOver stop.
const FLAT_TREE: u64 = 0;
/// Every element with its children. Only asked for a container the flat tree leaves empty: on iOS 26 and later
/// SwiftUI's navigation bar and tab bar are such containers, their buttons only listed here.
const FULL_TREE: u64 = 2;
const FRONTMOST_APPLICATION: u64 = 4;
const READ_ATTRIBUTES: u64 = 5;
/// frame, role, isRemoteElement, label, identifier, value, isEnabled, children. Never isVisible (112): it makes the
/// app hit-test every element, a whole tree goes from milliseconds to seconds.
const ATTRIBUTES: [u64; 8] = [21, 45, 66, 33, 25, 53, 27, 8];
/// Roles that hold other elements: one with no children in the flat tree is asked again in the full one.
const CONTAINERS: [&str; 5] = ["Group", "TabGroup", "ScrollArea", "Grid", "Sheet"];
const MAX_ELEMENTS: usize = 3000;
const MAX_DEPTH: usize = 50;

type Id = *mut c_void;
type Sel = *const c_void;
type Xpc = *mut c_void;

#[link(name = "objc")]
extern "C" {
    fn objc_getClass(name: *const c_char) -> Id;
    fn sel_registerName(name: *const c_char) -> Sel;
    fn objc_msgSend();
    fn class_getInstanceMethod(class: Id, selector: Sel) -> *const c_void;
    fn class_getClassMethod(class: Id, selector: Sel) -> *const c_void;
    fn objc_autoreleasePoolPush() -> *mut c_void;
    fn objc_autoreleasePoolPop(pool: *mut c_void);
}

#[link(name = "Foundation", kind = "framework")]
extern "C" {}

extern "C" {
    fn dlopen(path: *const c_char, mode: i32) -> *mut c_void;
    fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
    static _NSConcreteGlobalBlock: c_void;
    static _xpc_type_dictionary: c_void;
    static _xpc_type_data: c_void;
    fn xpc_dictionary_create(keys: *const *const c_char, values: *const Xpc, count: usize) -> Xpc;
    fn xpc_dictionary_set_string(dictionary: Xpc, key: *const c_char, value: *const c_char);
    fn xpc_dictionary_set_bool(dictionary: Xpc, key: *const c_char, value: bool);
    fn xpc_dictionary_set_uint64(dictionary: Xpc, key: *const c_char, value: u64);
    fn xpc_dictionary_set_double(dictionary: Xpc, key: *const c_char, value: f64);
    fn xpc_dictionary_set_value(dictionary: Xpc, key: *const c_char, value: Xpc);
    fn xpc_dictionary_get_value(dictionary: Xpc, key: *const c_char) -> Xpc;
    fn xpc_dictionary_get_string(dictionary: Xpc, key: *const c_char) -> *const c_char;
    fn xpc_data_create(bytes: *const c_void, length: usize) -> Xpc;
    fn xpc_data_get_bytes_ptr(data: Xpc) -> *const c_void;
    fn xpc_data_get_length(data: Xpc) -> usize;
    fn xpc_get_type(object: Xpc) -> *const c_void;
    fn xpc_release(object: Xpc);
    fn xpc_connection_create_from_endpoint(endpoint: Xpc) -> Xpc;
    fn xpc_connection_set_event_handler(connection: Xpc, handler: *const c_void);
    fn xpc_connection_resume(connection: Xpc);
    fn xpc_connection_cancel(connection: Xpc);
    fn xpc_connection_send_message(connection: Xpc, message: Xpc);
    fn xpc_connection_send_message_with_reply_sync(connection: Xpc, message: Xpc) -> Xpc;
}

const RTLD_NOW: i32 = 2;
const RTLD_DEFAULT: *mut c_void = -2isize as *mut c_void;

fn c(text: &str) -> CString {
    CString::new(text).unwrap_or_default()
}

fn selector(name: &str) -> Sel {
    unsafe { sel_registerName(c(name).as_ptr()) }
}

// objc_msgSend called with the exact signature of each method, as the ABI requires.
unsafe fn send0<R>(receiver: Id, name: &str) -> R {
    let function: unsafe extern "C" fn(Id, Sel) -> R = std::mem::transmute(objc_msgSend as *const ());
    function(receiver, selector(name))
}
unsafe fn send1<A, R>(receiver: Id, name: &str, a: A) -> R {
    let function: unsafe extern "C" fn(Id, Sel, A) -> R = std::mem::transmute(objc_msgSend as *const ());
    function(receiver, selector(name), a)
}
unsafe fn send2<A, B, R>(receiver: Id, name: &str, a: A, b: B) -> R {
    let function: unsafe extern "C" fn(Id, Sel, A, B) -> R = std::mem::transmute(objc_msgSend as *const ());
    function(receiver, selector(name), a, b)
}
unsafe fn send3<A, B, C, R>(receiver: Id, name: &str, a: A, b: B, c: C) -> R {
    let function: unsafe extern "C" fn(Id, Sel, A, B, C) -> R = std::mem::transmute(objc_msgSend as *const ());
    function(receiver, selector(name), a, b, c)
}

fn missing(hook: &str, what: &str) -> crate::state::Failure {
    failure(
        format!("{what} is missing in this Xcode ({hook}); ddp cannot drive this simulator without the library"),
        "hook_failed",
        Some("add the library to the Debug build and drop --device, or ddp doctor"),
    )
}

/// A class with the methods ddp sends it: a private piece that changed fails here, never as a crash later.
fn class(hook: &str, name: &str, instance_methods: &[&str], class_methods: &[&str]) -> Result<Id> {
    let found = unsafe { objc_getClass(c(name).as_ptr()) };
    if found.is_null() {
        return Err(missing(hook, name));
    }
    for method in instance_methods {
        if unsafe { class_getInstanceMethod(found, selector(method)) }.is_null() {
            return Err(missing(hook, &format!("-[{name} {method}]")));
        }
    }
    for method in class_methods {
        if unsafe { class_getClassMethod(found, selector(method)) }.is_null() {
            return Err(missing(hook, &format!("+[{name} {method}]")));
        }
    }
    Ok(found)
}

fn ns_string(text: &str) -> Id {
    let text = c(text);
    unsafe { send1(objc_getClass(c"NSString".as_ptr()), "stringWithUTF8String:", text.as_ptr()) }
}

fn ns_number(value: u64) -> Id {
    unsafe { send1(objc_getClass(c"NSNumber".as_ptr()), "numberWithUnsignedLongLong:", value) }
}

fn ns_dictionary(key: &str, value: Id) -> Id {
    let (keys, values) = ([ns_string(key)], [value]);
    unsafe {
        send3(
            objc_getClass(c"NSDictionary".as_ptr()),
            "dictionaryWithObjects:forKeys:count:",
            values.as_ptr(),
            keys.as_ptr(),
            1usize,
        )
    }
}

fn is_a(object: Id, class_name: &CStr) -> bool {
    !object.is_null() && unsafe { send1::<Id, i8>(object, "isKindOfClass:", objc_getClass(class_name.as_ptr())) } != 0
}

/// The text of an NSString or NSAttributedString, the number of an NSNumber; nothing for anything else.
fn text_of(object: Id) -> Option<String> {
    let string = if is_a(object, c"NSString") {
        object
    } else if is_a(object, c"NSAttributedString") {
        unsafe { send0(object, "string") }
    } else if is_a(object, c"NSNumber") {
        unsafe { send0(object, "stringValue") }
    } else {
        return None;
    };
    let utf8: *const c_char = unsafe { send0(string, "UTF8String") };
    (!utf8.is_null()).then(|| unsafe { CStr::from_ptr(utf8) }.to_string_lossy().into_owned()).filter(|t| !t.is_empty())
}

/// An NSValue holding a rect, read without a struct return (x86_64 returns a CGRect in memory).
fn rect_of(object: Id) -> Option<[f64; 4]> {
    if !is_a(object, c"NSValue") || is_a(object, c"NSNumber") {
        return None;
    }
    let kind: *const c_char = unsafe { send0(object, "objCType") };
    let kind = unsafe { CStr::from_ptr(kind) }.to_string_lossy();
    if !(kind.starts_with("{CGRect") || kind.starts_with("{NSRect") || kind.starts_with("{_NSRect")) {
        return None;
    }
    let mut rect = [0f64; 4];
    unsafe { send2::<*mut c_void, usize, ()>(object, "getValue:size:", rect.as_mut_ptr().cast(), 32) };
    Some(rect)
}

/// The length of an NSArray; 0 for anything else.
fn count_of(object: Id) -> usize {
    if is_a(object, c"NSArray") {
        unsafe { send0(object, "count") }
    } else {
        0
    }
}

fn number_of(object: Id) -> Option<i64> {
    is_a(object, c"NSNumber").then(|| unsafe { send0::<i64>(object, "longLongValue") })
}

/// A raw pointer that may cross to the thread waiting for an XPC answer.
struct Shared(*mut c_void);
unsafe impl Send for Shared {}

/// The handler libxpc requires before a connection runs. Answers come back to the caller; events are not needed.
#[repr(C)]
struct Block {
    isa: *const c_void,
    flags: i32,
    reserved: i32,
    invoke: unsafe extern "C" fn(*const Block, Xpc),
    descriptor: *const [usize; 2],
}
unsafe extern "C" fn ignore_event(_: *const Block, _: Xpc) {}
static BLOCK_DESCRIPTOR: [usize; 2] = [0, std::mem::size_of::<Block>()];

struct Connection(Xpc);

impl Drop for Connection {
    fn drop(&mut self) {
        unsafe { xpc_connection_cancel(self.0) };
    }
}

impl Connection {
    /// The answer to `message`, or nothing within `patience`. A message never answered stays with the thread.
    fn ask(&self, message: Xpc, patience: Duration) -> Option<Xpc> {
        let (sender, receiver) = mpsc::channel();
        let (connection, request) = (Shared(self.0), Shared(message));
        std::thread::spawn(move || {
            let (connection, request) = (connection, request);
            let answer = unsafe { xpc_connection_send_message_with_reply_sync(connection.0, request.0) };
            let _ = sender.send(Shared(answer));
        });
        let answer = receiver.recv_timeout(patience).ok()?.0;
        unsafe { xpc_release(message) };
        (unsafe { xpc_get_type(answer) } == std::ptr::addr_of!(_xpc_type_dictionary)).then_some(answer)
    }
}

fn xpc_dictionary() -> Xpc {
    unsafe { xpc_dictionary_create(std::ptr::null(), std::ptr::null(), 0) }
}

pub struct Simulator {
    pool: *mut c_void,
    device: Id,
    /// The screen in pixels and its scale: dtuhidd takes points as fractions of the screen.
    pixels: (f64, f64),
    scale: f64,
    hid: Option<Connection>,
    accessibility: Option<Connection>,
}

impl Drop for Simulator {
    fn drop(&mut self) {
        if self.hid.is_some() {
            // dtuhidd's answer means it took the events, not that the app has them: closing now drops what is still
            // on its way (a single tap went missing every time). idb waits as long before it disconnects.
            std::thread::sleep(DRAIN);
        }
        self.hid = None;
        self.accessibility = None;
        unsafe { objc_autoreleasePoolPop(self.pool) };
    }
}

impl Simulator {
    /// A booted simulator, by UDID, through CoreSimulator in this process.
    pub fn open(udid: &str) -> Result<Simulator> {
        let core_simulator = c"/Library/Developer/PrivateFrameworks/CoreSimulator.framework/CoreSimulator";
        if unsafe { dlopen(core_simulator.as_ptr(), RTLD_NOW) }.is_null() {
            return Err(missing(HID_HOOK, "CoreSimulator.framework"));
        }
        let pool = unsafe { objc_autoreleasePoolPush() };
        let mut simulator = Simulator {
            pool,
            device: std::ptr::null_mut(),
            pixels: (0.0, 0.0),
            scale: 0.0,
            hid: None,
            accessibility: None,
        };
        let context = class(
            HID_HOOK,
            "SimServiceContext",
            &["defaultDeviceSetWithError:"],
            &["sharedServiceContextForDeveloperDir:error:"],
        )?;
        class(HID_HOOK, "SimDeviceSet", &["devicesByUDID"], &[])?;
        class(HID_HOOK, "SimDevice", &["state", "lookup:error:", "deviceType"], &[])?;
        class(HID_HOOK, "SimDeviceType", &["mainScreenSize", "mainScreenScale"], &[])?;
        let developer_dir = std::env::var("DEVELOPER_DIR")
            .ok()
            .unwrap_or_else(|| capture("xcode-select", &["-p"]).stdout.trim().to_string());
        let null_error: *mut Id = std::ptr::null_mut();
        unsafe {
            let context: Id =
                send2(context, "sharedServiceContextForDeveloperDir:error:", ns_string(&developer_dir), null_error);
            let set: Id =
                if context.is_null() { context } else { send1(context, "defaultDeviceSetWithError:", null_error) };
            if set.is_null() {
                fail!("CoreSimulator gave no device set", "hook_failed", "xcrun simctl list devices");
            }
            let uuid: Id = send0(objc_getClass(c"NSUUID".as_ptr()), "alloc");
            let uuid: Id = send1(uuid, "initWithUUIDString:", ns_string(udid));
            let devices: Id = send0(set, "devicesByUDID");
            simulator.device = if uuid.is_null() { uuid } else { send1(devices, "objectForKey:", uuid) };
            if simulator.device.is_null() {
                fail!(format!("no simulator {udid}"), "unknown_device", "xcrun simctl list devices booted");
            }
            if send0::<u64>(simulator.device, "state") != BOOTED {
                fail!(
                    format!("simulator {udid} is not booted"),
                    "device_not_booted",
                    format!("xcrun simctl boot {udid}")
                );
            }
            #[repr(C)]
            struct Size(f64, f64);
            let device_type: Id = send0(simulator.device, "deviceType");
            let pixels: Size = send0(device_type, "mainScreenSize");
            simulator.pixels = (pixels.0, pixels.1);
            simulator.scale = send0::<f32>(device_type, "mainScreenScale") as f64;
        }
        if simulator.pixels.0 <= 0.0 || simulator.scale <= 0.0 {
            return Err(missing(HID_HOOK, "the simulator's screen size"));
        }
        Ok(simulator)
    }

    /// A connection to one of the simulator's services: its mach port from CoreSimulator, made an XPC endpoint.
    fn connect(&self, hook: &str, service: &str) -> Result<Connection> {
        type EndpointFromPort = unsafe extern "C" fn(u32, u64, u64) -> Xpc;
        type EnableSimToHost = unsafe extern "C" fn(Xpc);
        let endpoint_from_port = unsafe { dlsym(RTLD_DEFAULT, c"xpc_endpoint_create_mach_port_4sim".as_ptr()) };
        let enable_sim_to_host = unsafe { dlsym(RTLD_DEFAULT, c"xpc_connection_enable_sim2host_4sim".as_ptr()) };
        if endpoint_from_port.is_null() || enable_sim_to_host.is_null() {
            return Err(missing(hook, "libxpc's simulator endpoints"));
        }
        let null_error: *mut Id = std::ptr::null_mut();
        let port: u32 = unsafe { send2(self.device, "lookup:error:", ns_string(service), null_error) };
        if port == 0 {
            fail!(
                format!("the simulator offers no {service} ({hook})"),
                "hook_failed",
                "is it still booting? xcrun simctl bootstatus <udid> -b"
            );
        }
        unsafe {
            let endpoint = std::mem::transmute::<*mut c_void, EndpointFromPort>(endpoint_from_port)(port, 0, 0);
            let connection = if endpoint.is_null() { endpoint } else { xpc_connection_create_from_endpoint(endpoint) };
            if connection.is_null() {
                return Err(missing(hook, "an XPC connection to the simulator"));
            }
            std::mem::transmute::<*mut c_void, EnableSimToHost>(enable_sim_to_host)(connection);
            let handler = Box::leak(Box::new(Block {
                isa: std::ptr::addr_of!(_NSConcreteGlobalBlock),
                flags: 1 << 28, // a global block: nothing captured, never copied
                reserved: 0,
                invoke: ignore_event,
                descriptor: &BLOCK_DESCRIPTOR,
            }));
            xpc_connection_set_event_handler(connection, (handler as *const Block).cast());
            xpc_connection_resume(connection);
            Ok(Connection(connection))
        }
    }

    fn hid(&mut self) -> Result<&Connection> {
        if self.hid.is_none() {
            let connection = self.connect(HID_HOOK, HID_SERVICE)?;
            // dtuhidd can still be starting right after a boot: try a few times before any touch is sent.
            let answered = (0..3).any(|attempt| {
                if attempt > 0 {
                    std::thread::sleep(Duration::from_millis(300));
                }
                Self::acknowledge(&connection)
            });
            if !answered {
                fail!(
                    format!("the simulator's HID service does not answer ({HID_HOOK})"),
                    "hook_failed",
                    "wait for the boot to finish and try again, or ddp doctor"
                );
            }
            self.hid = Some(connection);
        }
        Ok(self.hid.as_ref().unwrap())
    }

    fn hid_message(kind: &str, payload: Xpc, barrier: bool) -> Xpc {
        let message = xpc_dictionary();
        unsafe {
            xpc_dictionary_set_string(message, c"messageType".as_ptr(), c(kind).as_ptr());
            xpc_dictionary_set_bool(message, c"isBarrier".as_ptr(), barrier);
            xpc_dictionary_set_string(message, c"featureIdentifier".as_ptr(), c(HID_SERVICE).as_ptr());
            xpc_dictionary_set_value(message, c"payload".as_ptr(), payload);
            xpc_release(payload);
        }
        message
    }

    fn key_payload(usage: u64, down: bool) -> Xpc {
        let payload = xpc_dictionary();
        unsafe {
            xpc_dictionary_set_uint64(payload, c"usageCode".as_ptr(), usage);
            xpc_dictionary_set_uint64(payload, c"state".as_ptr(), if down { 1 } else { 2 });
        }
        payload
    }

    /// A send to a dtuhidd that died succeeds without a word: only an answer proves the events before it were taken.
    fn acknowledge(connection: &Connection) -> bool {
        let barrier = Self::hid_message("IndigoKeyboardButtonEvent", Self::key_payload(0, false), true);
        let answer = connection.ask(barrier, HID_ANSWER);
        answer.inspect(|answer| unsafe { xpc_release(*answer) }).is_some()
    }

    /// One finger event at a point in the app's points.
    pub fn finger(&mut self, phase: u64, (x, y): (f64, f64)) -> Result<()> {
        let (pixels, scale) = (self.pixels, self.scale);
        let connection = self.hid()?;
        let point = xpc_dictionary();
        let payload = xpc_dictionary();
        unsafe {
            xpc_dictionary_set_double(point, c"x".as_ptr(), x * scale / pixels.0);
            xpc_dictionary_set_double(point, c"y".as_ptr(), y * scale / pixels.1);
            xpc_dictionary_set_value(payload, c"pointOne".as_ptr(), point);
            xpc_release(point);
            xpc_dictionary_set_uint64(payload, c"eventType".as_ptr(), phase);
            xpc_dictionary_set_uint64(payload, c"edge".as_ptr(), 0);
            xpc_dictionary_set_uint64(payload, c"target".as_ptr(), 0);
            let message = Self::hid_message("IndigoDigitizerEvent", payload, false);
            xpc_connection_send_message(connection.0, message);
            xpc_release(message);
        }
        Ok(())
    }

    /// A key down or up, by its USB HID usage.
    pub fn key(&mut self, usage: u64, down: bool) -> Result<()> {
        let connection = self.hid()?;
        let message = Self::hid_message("IndigoKeyboardButtonEvent", Self::key_payload(usage, down), false);
        unsafe {
            xpc_connection_send_message(connection.0, message);
            xpc_release(message);
        }
        Ok(())
    }

    /// Waits until dtuhidd took everything sent so far.
    pub fn delivered(&mut self) -> Result<()> {
        if !Self::acknowledge(self.hid()?) {
            fail!(
                format!("the simulator's HID service stopped answering; the input may not have arrived ({HID_HOOK})"),
                "hook_failed",
                "ddp tree --device <udid> to see where the app is"
            );
        }
        Ok(())
    }

    /// Text as key presses on a US keyboard. Anything else is refused before a key is pressed.
    pub fn type_text(&mut self, text: &str) -> Result<()> {
        let keys: Vec<(u64, bool)> = text
            .chars()
            .map(|character| {
                key_usage(character).ok_or_else(|| {
                    failure(
                        format!("'{character}' has no key on a US keyboard; without the library ddp types ASCII only"),
                        "bad_value",
                        None,
                    )
                })
            })
            .collect::<Result<_>>()?;
        for (usage, shift) in keys {
            if shift {
                self.key(LEFT_SHIFT, true)?;
            }
            self.key(usage, true)?;
            self.key(usage, false)?;
            if shift {
                self.key(LEFT_SHIFT, false)?;
            }
        }
        self.delivered()
    }

    /// The translator's framework, its classes checked, and the connection to the service.
    fn load_accessibility(&mut self) -> Result<()> {
        if self.accessibility.is_some() {
            return Ok(());
        }
        let framework =
            c"/System/Library/PrivateFrameworks/AccessibilityPlatformTranslation.framework/AccessibilityPlatformTranslation";
        if unsafe { dlopen(framework.as_ptr(), RTLD_NOW) }.is_null() {
            return Err(missing(ACCESSIBILITY_HOOK, "AccessibilityPlatformTranslation.framework"));
        }
        class(
            ACCESSIBILITY_HOOK,
            "AXPTranslatorRequest",
            &["setRequestType:", "setClientType:", "setParameters:"],
            &["requestWithTranslation:"],
        )?;
        class(ACCESSIBILITY_HOOK, "AXPTranslatorResponse", &["resultData"], &[])?;
        class(ACCESSIBILITY_HOOK, "AXPTranslationObject", &["pid", "objectID", "setBridgeDelegateToken:"], &[])?;
        self.accessibility = Some(self.connect(ACCESSIBILITY_HOOK, ACCESSIBILITY_SERVICE)?);
        Ok(())
    }

    fn accessibility_request(&mut self, kind: u64, translation: Id, parameters: Id, client: u64) -> Result<Id> {
        self.load_accessibility()?;
        let silent = || {
            failure(
                format!("the simulator's accessibility service did not answer ({ACCESSIBILITY_HOOK})"),
                "hook_failed",
                Some("is the app busy or frozen? try again, or ddp doctor"),
            )
        };
        unsafe {
            let request: Id =
                send1(objc_getClass(c"AXPTranslatorRequest".as_ptr()), "requestWithTranslation:", translation);
            send1::<u64, ()>(request, "setRequestType:", kind);
            send1::<u64, ()>(request, "setClientType:", client);
            send1::<Id, ()>(request, "setParameters:", parameters);
            let null_error: *mut Id = std::ptr::null_mut();
            let archived: Id = send3(
                objc_getClass(c"NSKeyedArchiver".as_ptr()),
                "archivedDataWithRootObject:requiringSecureCoding:error:",
                request,
                true,
                null_error,
            );
            if archived.is_null() {
                return Err(missing(ACCESSIBILITY_HOOK, "an archivable AXPTranslatorRequest"));
            }
            let data = xpc_data_create(send0(archived, "bytes"), send0(archived, "length"));
            let payload = xpc_dictionary();
            xpc_dictionary_set_value(payload, c"translatorRequest".as_ptr(), data);
            xpc_release(data);
            let message = xpc_dictionary();
            xpc_dictionary_set_string(
                message,
                c"SimAccessibility_PayloadClassName".as_ptr(),
                c"AXPTranslatorRequest".as_ptr(),
            );
            xpc_dictionary_set_value(message, c"SimAccessibility_Payload".as_ptr(), payload);
            xpc_release(payload);
            let answer = self.accessibility.as_ref().unwrap().ask(message, ACCESSIBILITY_ANSWER).ok_or_else(silent)?;
            let class_name = xpc_dictionary_get_string(answer, c"SimAccessibility_PayloadClassName".as_ptr());
            let payload = xpc_dictionary_get_value(answer, c"SimAccessibility_Payload".as_ptr());
            let data = if payload.is_null() {
                payload
            } else {
                xpc_dictionary_get_value(payload, c"translatorResponse".as_ptr())
            };
            if class_name.is_null()
                || CStr::from_ptr(class_name) != c"AXPTranslatorResponse"
                || data.is_null()
                || xpc_get_type(data) != std::ptr::addr_of!(_xpc_type_data)
            {
                xpc_release(answer);
                return Err(missing(ACCESSIBILITY_HOOK, "an AXPTranslatorResponse in the answer"));
            }
            let bytes: Id = send2(
                objc_getClass(c"NSData".as_ptr()),
                "dataWithBytes:length:",
                xpc_data_get_bytes_ptr(data),
                xpc_data_get_length(data),
            );
            xpc_release(answer);
            let response: Id = send3(
                objc_getClass(c"NSKeyedUnarchiver".as_ptr()),
                "unarchivedObjectOfClass:fromData:error:",
                objc_getClass(c"AXPTranslatorResponse".as_ptr()),
                bytes,
                null_error,
            );
            if response.is_null() {
                return Err(missing(ACCESSIBILITY_HOOK, "a readable AXPTranslatorResponse"));
            }
            Ok(send0(response, "resultData"))
        }
    }

    /// What is on screen, as the platform's accessibility gives it: elements with a label, an id or a value, in
    /// reading order, frames in points. Any element the service does not answer for fails the whole read: a tree
    /// with holes would read as a screen without those elements.
    pub fn elements(&mut self) -> Result<Vec<Value>> {
        let root = self.front_app()?;
        let token = ns_string(&format!("ddp-{}", std::process::id()));
        let attributes: Vec<Id> = ATTRIBUTES.iter().map(|attribute| ns_number(*attribute)).collect();
        let attributes: Id = unsafe {
            send2(objc_getClass(c"NSArray".as_ptr()), "arrayWithObjects:count:", attributes.as_ptr(), attributes.len())
        };
        let parameters = ns_dictionary("attributes", attributes);
        let children_attribute = [ns_number(8)];
        let children_only = ns_dictionary("attributes", unsafe {
            send2(objc_getClass(c"NSArray".as_ptr()), "arrayWithObjects:count:", children_attribute.as_ptr(), 1usize)
        });
        let mut walk = Walk { token, parameters, children_only, elements: Vec::new(), read: 0, seen: HashSet::new() };
        self.read(&mut walk, root, 0)?;
        Ok(walk.elements)
    }

    /// The app in front, as the accessibility service names it. Right after a launch it can still be resolving.
    pub fn front_app(&mut self) -> Result<Id> {
        self.load_accessibility()?;
        for attempt in 0..3 {
            if attempt > 0 {
                std::thread::sleep(Duration::from_millis(300));
            }
            let fresh: Id = unsafe { send0(objc_getClass(c"AXPTranslationObject".as_ptr()), "new") };
            let root = self.accessibility_request(
                FRONTMOST_APPLICATION,
                fresh,
                ns_dictionary("displayId", ns_number(0)),
                FLAT_TREE,
            )?;
            if is_a(root, c"AXPTranslationObject") {
                return Ok(root);
            }
        }
        fail!("no app in front on the simulator", "no_app", "ddp up, or open the app first");
    }

    /// Whether dtuhidd answers, for `ddp doctor`.
    pub fn hid_answers(&mut self) -> Result<()> {
        self.hid().map(|_| ())
    }

    fn read(&mut self, walk: &mut Walk, element: Id, depth: usize) -> Result<()> {
        walk.read += 1;
        if walk.read > MAX_ELEMENTS || depth > MAX_DEPTH {
            fail!(format!("more than {MAX_ELEMENTS} elements or {MAX_DEPTH} levels on screen"), "too_many_elements");
        }
        walk.seen.insert(unsafe { (send0(element, "pid"), send0(element, "objectID")) });
        unsafe { send1::<Id, ()>(element, "setBridgeDelegateToken:", walk.token) };
        let answer = self.accessibility_request(READ_ATTRIBUTES, element, walk.parameters, FLAT_TREE)?;
        if !is_a(answer, c"NSDictionary") {
            fail!(
                format!("the simulator could not read an element on screen ({ACCESSIBILITY_HOOK})"),
                "hook_failed",
                "try again when the screen is still"
            );
        }
        let (role, entry) = entry_of(answer);
        if role != "Application" && ["label", "id", "value"].iter().any(|field| entry.get(field).is_some()) {
            walk.elements.push(entry);
        }
        let children_of = |answer: Id| -> Id {
            if is_a(answer, c"NSDictionary") {
                unsafe { send1(answer, "objectForKey:", ns_number(8)) }
            } else {
                std::ptr::null_mut()
            }
        };
        let mut children = children_of(answer);
        if count_of(children) == 0 && CONTAINERS.contains(&role) {
            children =
                children_of(self.accessibility_request(READ_ATTRIBUTES, element, walk.children_only, FULL_TREE)?);
        }
        for index in 0..count_of(children) {
            let child: Id = unsafe { send1(children, "objectAtIndex:", index) };
            if !is_a(child, c"AXPTranslationObject") {
                continue;
            }
            let child_identity: (i32, u64) = unsafe { (send0(child, "pid"), send0(child, "objectID")) };
            if !walk.seen.contains(&child_identity) {
                self.read(walk, child, depth + 1)?;
            }
        }
        Ok(())
    }
}

/// An element as `tree` lists it, from the translator's answer: role, frame in points, label, id, value, enabled.
fn entry_of(answer: Id) -> (&'static str, Value) {
    let value = |attribute: u64| -> Id { unsafe { send1(answer, "objectForKey:", ns_number(attribute)) } };
    let remote = number_of(value(66)).unwrap_or(0) != 0;
    let role = if remote { "Group" } else { role_name(number_of(value(45)).unwrap_or(0)) };
    let frame = rect_of(value(21)).unwrap_or_default();
    let mut entry = json!({"type": role, "frame": frame.iter().map(|v| (v * 10.0).round() / 10.0).collect::<Vec<_>>()});
    for (field, attribute) in [("label", 33), ("id", 25), ("value", 53)] {
        if let Some(text) = text_of(value(attribute)) {
            entry[field] = json!(text);
        }
    }
    if number_of(value(27)) == Some(0) {
        entry["enabled"] = json!(false);
    }
    (role, entry)
}

struct Walk {
    token: Id,
    parameters: Id,
    children_only: Id,
    elements: Vec<Value>,
    read: usize,
    /// Elements read so far, by pid and object id: an element listed twice (among its own descendants, or by the
    /// full tree too) is read once.
    seen: HashSet<(i32, u64)>,
}

/// The translator's role numbers, by AppKit's names without the `AX` prefix.
fn role_name(role: i64) -> &'static str {
    const ROLES: [&str; 22] = [
        "Element",
        "Application",
        "Button",
        "CheckBox",
        "Element",
        "Group",
        "Heading",
        "Image",
        "Link",
        "RadioButton",
        "ScrollArea",
        "ScrollBar",
        "TabGroup",
        "Slider",
        "StaticText",
        "TextField",
        "WebArea",
        "TextArea",
        "Splitter",
        "PopUpButton",
        "Sheet",
        "Grid",
    ];
    usize::try_from(role).ok().and_then(|index| ROLES.get(index)).copied().unwrap_or("Element")
}

const LEFT_SHIFT: u64 = 225;
pub const ENTER: u64 = 40;
pub const BACKSPACE: u64 = 42;
pub const TAB: u64 = 43;

/// The USB HID usage of a character on a US keyboard, and whether it needs shift.
fn key_usage(character: char) -> Option<(u64, bool)> {
    const SHIFTED_DIGITS: &str = ")!@#$%^&*(";
    const SYMBOLS: [(char, char, u64); 11] = [
        ('-', '_', 45),
        ('=', '+', 46),
        ('[', '{', 47),
        (']', '}', 48),
        ('\\', '|', 49),
        (';', ':', 51),
        ('\'', '"', 52),
        ('`', '~', 53),
        (',', '<', 54),
        ('.', '>', 55),
        ('/', '?', 56),
    ];
    let digit_usage = |digit: u32| if digit == 0 { 39 } else { 29 + digit as u64 };
    match character {
        'a'..='z' => Some((4 + (character as u64 - 'a' as u64), false)),
        'A'..='Z' => Some((4 + (character as u64 - 'A' as u64), true)),
        '0'..='9' => Some((digit_usage(character.to_digit(10)?), false)),
        '\n' => Some((ENTER, false)),
        '\t' => Some((TAB, false)),
        ' ' => Some((44, false)),
        _ => SHIFTED_DIGITS.find(character).map(|digit| (digit_usage(digit as u32), true)).or_else(|| {
            SYMBOLS.iter().find_map(|&(plain, shifted, usage)| {
                (character == plain).then_some((usage, false)).or((character == shifted).then_some((usage, true)))
            })
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_on_a_us_keyboard() {
        assert_eq!(key_usage('a'), Some((4, false)));
        assert_eq!(key_usage('Z'), Some((29, true)));
        assert_eq!(key_usage('1'), Some((30, false)));
        assert_eq!(key_usage('0'), Some((39, false)));
        assert_eq!(key_usage('!'), Some((30, true)));
        assert_eq!(key_usage(')'), Some((39, true)));
        assert_eq!(key_usage('?'), Some((56, true)));
        assert_eq!(key_usage('-'), Some((45, false)));
        assert_eq!(key_usage('é'), None);
        assert_eq!(role_name(2), "Button");
        assert_eq!(role_name(99), "Element");
    }
}
