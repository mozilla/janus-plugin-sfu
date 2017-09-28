#[macro_use]
extern crate cstr_macro;
#[macro_use]
extern crate lazy_static;
#[macro_use]
extern crate janus_plugin as janus;
extern crate jansson_sys as jansson;

use std::ptr;
use std::ffi::CString;
use std::os::raw::{c_char, c_int, c_void};
use std::sync::{Mutex, RwLock};
use janus::{LogLevel, Plugin, PluginCallbacks, PluginMetadata, PluginResult, PluginResultType, PluginSession};

pub struct ProxySession {
    pub has_audio: bool,
    pub has_data: bool,
    pub bitrate: u32,
    pub slowlink_count: u16,
    pub hanging_up: i32,
    pub destroyed: i64,
}

pub struct ProxyMessage {
    pub session: ProxySession,
    pub transaction: String,
}

pub struct ProxyPluginState<'a> {
    pub sessions: RwLock<Vec<Box<ProxySession>>>,
    pub messages: RwLock<Vec<Box<ProxyMessage>>>,
    pub callbacks: Mutex<Option<&'a PluginCallbacks>>,
}

lazy_static! {
    static ref STATE: ProxyPluginState<'static> = ProxyPluginState {
        sessions: RwLock::new(Vec::new()),
        messages: RwLock::new(Vec::new()),
        callbacks: Mutex::new(None)
    };
}

pub const METADATA: PluginMetadata = PluginMetadata {
    version: 1,
    version_str: cstr!("0.0.1"),
    description: cstr!("Janus WebRTC reverse proxy for Reticulum."),
    name: cstr!("Janus retproxy plugin"),
    author: cstr!("Marshall Quander"),
    package: cstr!("janus.plugin.retproxy"),
};

extern "C" fn init(callbacks: *mut PluginCallbacks, config_path: *const c_char) -> c_int {
    if callbacks.is_null() || config_path.is_null() {
        janus::log(LogLevel::Err, "Invalid parameters for retproxy plugin initialization!");
        return -1;
    }

    *STATE.callbacks.lock().unwrap() = unsafe { callbacks.as_ref() };
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
    });
    unsafe {
        (*handle).plugin_handle = session.as_ref() as *const ProxySession as *mut c_void;
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
    let session: &mut ProxySession = unsafe { &mut *((*handle).plugin_handle as *mut ProxySession) };
    session.destroyed = 1;
}

extern "C" fn query_session(handle: *mut PluginSession) -> *mut janus::Json {
    if handle.is_null() {
        janus::log(LogLevel::Err, "No session associated with handle!");
        return ptr::null_mut();
    }
    let session: &mut ProxySession = unsafe { &mut *((*handle).plugin_handle as *mut ProxySession) };
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
    let session: &mut ProxySession = unsafe { &mut *((*handle).plugin_handle as *mut ProxySession) };
    session.hanging_up = 0;
}

extern "C" fn incoming_rtp(handle: *mut PluginSession, video: c_int, buf: *mut c_char, len: c_int) {
    janus::log(LogLevel::Huge, "RTP packet received!");
    if handle.is_null() {
        janus::log(LogLevel::Err, "No session associated with handle!");
        return;
    }
    ((*STATE.callbacks.lock().unwrap()).as_ref().unwrap().relay_rtp)(handle, video, buf, len);
}

extern "C" fn incoming_rtcp(handle: *mut PluginSession, video: c_int, buf: *mut c_char, len: c_int) {
    janus::log(LogLevel::Huge, "RTCP packet received!");
    if handle.is_null() {
        janus::log(LogLevel::Err, "No session associated with handle!");
        return;
    }
    ((*STATE.callbacks.lock().unwrap()).as_ref().unwrap().relay_rtcp)(handle, video, buf, len);
}

extern "C" fn incoming_data(handle: *mut PluginSession, buf: *mut c_char, len: c_int) {
    janus::log(LogLevel::Huge, "SCTP packet received!");
    if handle.is_null() {
        janus::log(LogLevel::Err, "No session associated with handle!");
        return;
    }
    ((*STATE.callbacks.lock().unwrap()).as_ref().unwrap().relay_data)(handle, buf, len);
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

extern "C" fn handle_message(
    handle: *mut PluginSession,
    transaction: *mut c_char,
    message: *mut janus::Json,
    jsep: *mut janus::Json,
) -> *mut PluginResult {
    janus::log(LogLevel::Verb, "Received signalling message.");
    if handle.is_null() {
        janus::log(LogLevel::Err, "No session associated with handle!");
        return Box::into_raw(janus::create_result(PluginResultType::JANUS_PLUGIN_ERROR, cstr!("No session associated with handle!"), ptr::null_mut()));
    }
    if message.is_null() {
        janus::log(LogLevel::Err, "Null message received!");
        return Box::into_raw(janus::create_result(PluginResultType::JANUS_PLUGIN_ERROR, cstr!("Null message received!"), ptr::null_mut()));
    }

    let (root, jsep) = unsafe { (&*message, &*jsep) };

    if root.type_ != jansson::json_type::JSON_OBJECT {
        janus::log(LogLevel::Err, "Message wasn't a JSON object.");
        return Box::into_raw(janus::create_result(PluginResultType::JANUS_PLUGIN_ERROR, cstr!("Message wasn't a JSON object."), ptr::null_mut()));
    }

    let sdp_val = unsafe { jansson::json_string_value(jansson::json_object_get(jsep, cstr!("sdp"))) };
    if sdp_val.is_null() {
        let ret = ((*STATE.callbacks.lock().unwrap()).as_ref().unwrap().push_event)(
            handle,
            &mut PLUGIN,
            transaction,
            unsafe { jansson::json_object() },
            ptr::null_mut());
        janus::log(LogLevel::Verb, &format!("Sent event. Received {} ({}).", ret, janus::get_api_error(ret)));
    } else {
        let offer_str = unsafe { CString::from_raw(sdp_val as *mut _) };
        janus::log(LogLevel::Info, &format!("Received SDP offer: {}", offer_str.to_str().unwrap()));
        let offer = janus::sdp::parse_sdp(offer_str).unwrap();
        let answer = answer_sdp!(&offer, janus::sdp::OfferAnswerParameters::Video, 0);
        let answer_str = janus::sdp::write_sdp(&answer);
        unsafe {
            let jsep = jansson::json_object();
            jansson::json_object_set_new(jsep, cstr!("type"), jansson::json_string(cstr!("answer")));
            jansson::json_object_set_new(jsep, cstr!("sdp"), jansson::json_string(answer_str.as_ptr()));
            let _ret = ((*STATE.callbacks.lock().unwrap()).as_ref().unwrap().push_event)(
                handle,
                &mut PLUGIN,
                transaction,
                jansson::json_object(),
                jsep);
        }
    }

    Box::into_raw(janus::create_result(PluginResultType::JANUS_PLUGIN_OK, ptr::null(), unsafe { jansson::json_object() }))
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
