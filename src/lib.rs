#[macro_use]
extern crate cstr_macro;
#[macro_use]
extern crate lazy_static;
#[macro_use]
extern crate janus_plugin as janus;
extern crate jansson_sys as jansson;

use std::error::Error;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int};
use std::ptr;
use std::sync::{RwLock};
use janus::{LogLevel, Plugin, PluginCallbacks, PluginMetadata,
            PluginResultInfo, PluginResultType, PluginHandle};
use jansson::json_t as Json;

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
enum SessionKind {
    Unknown,
    Publisher,
    Listener,
}

#[derive(Debug)]
enum PluginSuccess {
    Ok(*mut Json),
    OkWait(&'static CStr)
}

type PluginResult = Result<PluginSuccess, Box<Error+Send+Sync>>;

#[derive(Debug)]
struct Session {
    pub kind: SessionKind,
    pub handle: *mut PluginHandle,
}

impl Session {
    fn set_kind(&mut self, kind: SessionKind) -> Result<(), Box<Error+Send+Sync>> {
        match self.kind {
            SessionKind::Unknown => { self.kind = kind; Ok(()) },
            x if x == kind => Ok(()),
            _ => Err(From::from(format!("Session already configured as {:?}; can't set to {:?}.", self.kind, kind)))
        }
    }
}

unsafe impl Sync for Session {}
unsafe impl Send for Session {}

struct State {
    pub sessions: RwLock<Vec<Box<Session>>>
}

const METADATA: PluginMetadata = PluginMetadata {
    version: 1,
    version_str: cstr!("0.0.1"),
    description: cstr!("Janus WebRTC reverse proxy for Reticulum."),
    name: cstr!("Janus retproxy plugin"),
    author: cstr!("Marshall Quander"),
    package: cstr!("janus.plugin.retproxy"),
};

static mut CALLBACKS: Option<&PluginCallbacks> = None;

/// Returns a ref to the callback struct provided by Janus containing function pointers to pass data back to the gateway.
fn gateway_callbacks() -> &'static PluginCallbacks {
    unsafe { CALLBACKS.expect("Callbacks not initialized -- did plugin init() succeed?") }
}

lazy_static! {
    static ref STATE: State = State {
        sessions: RwLock::new(Vec::new())
    };
}

extern "C" fn init(callbacks: *mut PluginCallbacks, config_path: *const c_char) -> c_int {
    if callbacks.is_null() || config_path.is_null() {
        janus::log(LogLevel::Err, "Invalid parameters for retproxy plugin initialization!");
        return -1;
    }

    unsafe { CALLBACKS = callbacks.as_ref() };
    janus::log(LogLevel::Info, "Janus retproxy plugin initialized!");
    0
}

extern "C" fn destroy() {
    janus::log(LogLevel::Info, "Janus retproxy plugin destroyed!");
}

extern "C" fn create_session(handle: *mut PluginHandle, _error: *mut c_int) {
    janus::log(LogLevel::Info, "Initializing retproxy session...");
    let session = Box::new(Session {
        kind: SessionKind::Unknown,
        handle: handle
    });
    unsafe {
        (*handle).plugin_handle = session.as_ref() as *const _ as *mut _;
    }
    (*STATE.sessions.write().unwrap()).push(session);
}

extern "C" fn destroy_session(handle: *mut PluginHandle, error: *mut c_int) {
    janus::log(LogLevel::Info, "Destroying retproxy session...");
    if handle.is_null() {
        janus::log(LogLevel::Err, "No session associated with handle!");
        unsafe { *error = -1 };
        return
    }
}

extern "C" fn query_session(handle: *mut PluginHandle) -> *mut janus::Json {
    if handle.is_null() {
        janus::log(LogLevel::Err, "No session associated with handle!");
        return ptr::null_mut();
    }
    unsafe {
        jansson::json_object()
    }
}

extern "C" fn setup_media(handle: *mut PluginHandle) {
    janus::log(LogLevel::Verb, "WebRTC media is now available.");
    if handle.is_null() {
        janus::log(LogLevel::Err, "No session associated with handle!");
        return;
    }
}

extern "C" fn incoming_rtp(handle: *mut PluginHandle, video: c_int, buf: *mut c_char, len: c_int) {
    janus::log(LogLevel::Huge, "RTP packet received!");
    if handle.is_null() {
        janus::log(LogLevel::Err, "No session associated with handle!");
        return;
    }
    (gateway_callbacks().relay_rtp)(handle, video, buf, len);
}

extern "C" fn incoming_rtcp(handle: *mut PluginHandle, video: c_int, buf: *mut c_char, len: c_int) {
    janus::log(LogLevel::Huge, "RTCP packet received!");
    if handle.is_null() {
        janus::log(LogLevel::Err, "No session associated with handle!");
        return;
    }

    (gateway_callbacks().relay_rtcp)(handle, video, buf, len);
}

extern "C" fn incoming_data(handle: *mut PluginHandle, buf: *mut c_char, len: c_int) {
    janus::log(LogLevel::Verb, "SCTP packet received!");
    if handle.is_null() {
        janus::log(LogLevel::Err, "No session associated with handle!");
        return;
    }

    let sessions = &*STATE.sessions.read().unwrap();
    let this_session_ptr = unsafe { (*handle).plugin_handle };
    for other_session in sessions {
        let other_session_ptr = other_session.as_ref() as *const _ as *mut _;
        if this_session_ptr != other_session_ptr {
            (gateway_callbacks().relay_data)(other_session.handle, buf, len);
        }
    }
}

extern "C" fn slow_link(handle: *mut PluginHandle, _uplink: c_int, _video: c_int) {
    janus::log(LogLevel::Verb, "Slow link message received!");
    if handle.is_null() {
        janus::log(LogLevel::Err, "No session associated with handle!");
        return;
    }
}

extern "C" fn hangup_media(handle: *mut PluginHandle) {
    janus::log(LogLevel::Verb, "Hanging up WebRTC media.");
    if handle.is_null() {
        janus::log(LogLevel::Err, "No session associated with handle!");
        return;
    }
}

fn push_event(handle: *mut PluginHandle, transaction: *mut c_char, message: *mut Json, jsep: *mut Json) -> Result<(), Box<Error+Send+Sync>> {
    let f = gateway_callbacks().push_event;
    janus::get_result(f(handle, &mut PLUGIN, transaction, message, jsep))
}

fn handle_contents(session: &mut Session, _transaction: *mut c_char, message: &Json) -> PluginResult {
    if message.type_ != jansson::json_type::JSON_OBJECT {
        Err(From::from("Message wasn't a JSON object."))
    } else {
        unsafe {
            let kind_json = jansson::json_object_get(message, cstr!("kind"));
            if !kind_json.is_null() && (*kind_json).type_ == jansson::json_type::JSON_STRING {
                let kind = CStr::from_ptr(jansson::json_string_value(kind_json));
                if kind == CStr::from_ptr(cstr!("publisher")) {
                    janus::log(LogLevel::Verb, &format!("Configuring session {:?} as publisher.", session));
                    session.set_kind(SessionKind::Publisher).map(|_| PluginSuccess::Ok(ptr::null_mut()))
                } else if kind == CStr::from_ptr(cstr!("listener")) {
                    janus::log(LogLevel::Verb, &format!("Configuring session {:?} as listener.", session));
                    session.set_kind(SessionKind::Listener).map(|_| PluginSuccess::Ok(ptr::null_mut()))
                } else {
                    Err(From::from("Unknown session kind specified (neither publisher nor listener.)"))
                }
            } else {
                Ok(PluginSuccess::Ok(ptr::null_mut()))
            }
        }
    }
}

fn handle_jsep(session: &Session, transaction: *mut c_char, jsep: &Json) -> PluginResult {
    if jsep.type_ != jansson::json_type::JSON_OBJECT {
        Err(From::from("JSEP wasn't a JSON object."))
    } else {
        let sdp_val = unsafe { jansson::json_string_value(jansson::json_object_get(jsep, cstr!("sdp"))) };
        if sdp_val.is_null() {
            Err(From::from("No SDP supplied in JSEP."))
        } else {
            let offer_str = unsafe { CString::from_raw(sdp_val as *mut _) };
            let offer = janus::sdp::parse_sdp(offer_str)?;
            let answer = answer_sdp!(&offer, janus::sdp::OfferAnswerParameters::Video, 0);
            let answer_str = janus::sdp::write_sdp(&answer);
            unsafe {
                let answer_jsep = jansson::json_object();
                jansson::json_object_set_new(answer_jsep, cstr!("type"), jansson::json_string(cstr!("answer")));
                jansson::json_object_set_new(answer_jsep, cstr!("sdp"), jansson::json_string(answer_str.as_ptr()));
                push_event(session.handle, transaction, jansson::json_object(), answer_jsep).map(
                    |_| PluginSuccess::Ok(ptr::null_mut())
                )
            }
        }
    }
}

fn handle_message_core(session: &mut Session, transaction: *mut c_char, message: *mut Json, jsep: *mut Json) -> PluginResult {
    if !jsep.is_null() {
        handle_jsep(session, transaction, unsafe { &*jsep })?;
    }
    if !message.is_null() {
        handle_contents(session, transaction, unsafe { &*message })
    } else {
        Ok(PluginSuccess::Ok(ptr::null_mut()))
    }
}

extern "C" fn handle_message(handle: *mut PluginHandle, transaction: *mut c_char, message: *mut Json, jsep: *mut Json) -> *mut PluginResultInfo {
    janus::log(LogLevel::Verb, "Received signalling message.");
    Box::into_raw(
        if handle.is_null() {
            janus::create_result(PluginResultType::JANUS_PLUGIN_ERROR, cstr!("No handle associated with message!"), ptr::null_mut())
        } else {
            let session = unsafe { &mut *((*handle).plugin_handle as *mut Session) };
            match handle_message_core(session, transaction, message, jsep) {
                Ok(PluginSuccess::Ok(json)) => janus::create_result(PluginResultType::JANUS_PLUGIN_OK, ptr::null(), json),
                Ok(PluginSuccess::OkWait(msg)) => janus::create_result(PluginResultType::JANUS_PLUGIN_OK_WAIT, msg.as_ptr(), ptr::null_mut()),
                Err(err) => {
                    janus::log(LogLevel::Err, &format!("Error handling message: {}", err));
                    janus::create_result(PluginResultType::JANUS_PLUGIN_OK, ptr::null(), ptr::null_mut()) // todo: send error to client
                }
            }
        }
    )
}

const PLUGIN: Plugin = build_plugin!(
    METADATA,
    init,
    destroy,
    create_session,
    handle_message,
    setup_media,
    incoming_rtp,
    incoming_rtcp,
    incoming_data,
    slow_link,
    hangup_media,
    destroy_session,
    query_session
);

export_plugin!(&PLUGIN);
