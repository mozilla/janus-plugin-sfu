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
            PluginResultInfo, PluginResultType, PluginSession};
use jansson::json_t as Json;

#[derive(Debug)]
enum PluginSuccess {
    Ok(*mut Json),
    OkWait(&'static CStr)
}

type PluginResult = Result<PluginSuccess, Box<Error+Send+Sync>>;

struct ProxySession {
    pub has_audio: bool,
    pub has_data: bool,
    pub bitrate: u32,
    pub slowlink_count: u16,
    pub hanging_up: i32,
    pub destroyed: i64,
    pub handle: *mut PluginSession,
}

unsafe impl Sync for ProxySession {}
unsafe impl Send for ProxySession {}

struct ProxyMessage {
    pub session: ProxySession,
    pub transaction: String,
}

struct ProxyPluginState {
    pub sessions: RwLock<Vec<Box<ProxySession>>>,
    pub messages: RwLock<Vec<Box<ProxyMessage>>>,
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
    static ref STATE: ProxyPluginState = ProxyPluginState {
        sessions: RwLock::new(Vec::new()),
        messages: RwLock::new(Vec::new()),
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

extern "C" fn create_session(handle: *mut PluginSession, _error: *mut c_int) {
    janus::log(LogLevel::Info, "Initializing retproxy session...");
    let session = Box::new(ProxySession {
        has_audio: false,
        has_data: false,
        bitrate: 0,
        destroyed: 0,
        hanging_up: 0,
        slowlink_count: 0,
        handle: handle
    });
    unsafe {
        (*handle).plugin_handle = session.as_ref() as *const _ as *mut _;
    }
    (*STATE.sessions.write().unwrap()).push(session);
}

extern "C" fn destroy_session(handle: *mut PluginSession, error: *mut c_int) {
    janus::log(LogLevel::Info, "Destroying retproxy session...");
    if handle.is_null() {
        janus::log(LogLevel::Err, "No session associated with handle!");
        unsafe { *error = -1 };
        return
    }
    let session = unsafe { &mut *((*handle).plugin_handle as *mut ProxySession) };
    session.destroyed = 1;
}

extern "C" fn query_session(handle: *mut PluginSession) -> *mut janus::Json {
    if handle.is_null() {
        janus::log(LogLevel::Err, "No session associated with handle!");
        return ptr::null_mut();
    }
    let session = unsafe { &mut *((*handle).plugin_handle as *mut ProxySession) };
    unsafe {
        let result = jansson::json_object();
        jansson::json_object_set_new(result, cstr!("bitrate"), jansson::json_integer(session.bitrate as i64));
        jansson::json_object_set_new(result, cstr!("slowlink_count"), jansson::json_integer(session.slowlink_count as i64));
        jansson::json_object_set_new(result, cstr!("destroyed"), jansson::json_integer(session.destroyed));
        result
    }
}

extern "C" fn setup_media(handle: *mut PluginSession) {
    janus::log(LogLevel::Verb, "WebRTC media is now available.");
    if handle.is_null() {
        janus::log(LogLevel::Err, "No session associated with handle!");
        return;
    }
    let session = unsafe { &mut *((*handle).plugin_handle as *mut ProxySession) };
    session.hanging_up = 0;
}

extern "C" fn incoming_rtp(handle: *mut PluginSession, video: c_int, buf: *mut c_char, len: c_int) {
    janus::log(LogLevel::Huge, "RTP packet received!");
    if handle.is_null() {
        janus::log(LogLevel::Err, "No session associated with handle!");
        return;
    }
    (gateway_callbacks().relay_rtp)(handle, video, buf, len);
}

extern "C" fn incoming_rtcp(handle: *mut PluginSession, video: c_int, buf: *mut c_char, len: c_int) {
    janus::log(LogLevel::Huge, "RTCP packet received!");
    if handle.is_null() {
        janus::log(LogLevel::Err, "No session associated with handle!");
        return;
    }

    (gateway_callbacks().relay_rtcp)(handle, video, buf, len);
}

extern "C" fn incoming_data(handle: *mut PluginSession, buf: *mut c_char, len: c_int) {
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

extern "C" fn slow_link(handle: *mut PluginSession, _uplink: c_int, _video: c_int) {
    janus::log(LogLevel::Verb, "Slow link message received!");
    if handle.is_null() {
        janus::log(LogLevel::Err, "No session associated with handle!");
        return;
    }
}

extern "C" fn hangup_media(handle: *mut PluginSession) {
    janus::log(LogLevel::Verb, "Hanging up WebRTC media.");
    if handle.is_null() {
        janus::log(LogLevel::Err, "No session associated with handle!");
        return;
    }
}

fn push_event(handle: *mut PluginSession, transaction: *mut c_char, message: *mut Json, jsep: *mut Json) -> Result<(), Box<Error+Send+Sync>> {
    let f = gateway_callbacks().push_event;
    janus::get_result(f(handle, &mut PLUGIN, transaction, message, jsep))
}

fn handle_contents(_handle: *mut PluginSession, _transaction: *mut c_char, message: &Json) -> PluginResult {
    if message.type_ != jansson::json_type::JSON_OBJECT {
        Err(From::from("Message wasn't a JSON object."))
    } else {
        Ok(PluginSuccess::Ok(ptr::null_mut()))
    }
}

fn handle_jsep(handle: *mut PluginSession, transaction: *mut c_char, jsep: &Json) -> PluginResult {
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
                push_event(handle, transaction, jansson::json_object(), answer_jsep).map(
                    |_| PluginSuccess::Ok(ptr::null_mut())
                )
            }
        }
    }
}

fn handle_message_core(handle: *mut PluginSession, transaction: *mut c_char, message: *mut Json, jsep: *mut Json) -> PluginResult {
    if !jsep.is_null() {
        handle_jsep(handle, transaction, unsafe { &*jsep })?;
    }
    if !message.is_null() {
        handle_contents(handle, transaction, unsafe { &*message })
    } else {
        Ok(PluginSuccess::Ok(ptr::null_mut()))
    }
}

extern "C" fn handle_message(handle: *mut PluginSession, transaction: *mut c_char, message: *mut Json, jsep: *mut Json) -> *mut PluginResultInfo {
    janus::log(LogLevel::Verb, "Received signalling message.");
    Box::into_raw(
        if handle.is_null() {
            janus::create_result(PluginResultType::JANUS_PLUGIN_ERROR, cstr!("No handle associated with message!"), ptr::null_mut())
        } else {
            match handle_message_core(handle, transaction, message, jsep) {
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
