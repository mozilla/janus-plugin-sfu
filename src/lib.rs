#[macro_use]
extern crate cstr_macro;

#[macro_use]
extern crate lazy_static;

#[macro_use]
extern crate janus_plugin as janus;
extern crate janus_plugin_sys as janus_internal;

use std::ptr;
use std::os::raw::{c_char, c_int, c_void};
use std::sync::{Arc, Mutex};
use janus::{PluginCallbacks, PluginMetadata, PluginResult, PluginResultType, PluginSession};

pub struct ProxySession {
    pub has_audio: bool,
    pub has_video: bool,
    pub has_data: bool,
    pub audio_active: bool,
    pub video_active: bool,
    pub bitrate: i32,
    pub slowlink_count: i16,
    pub hanging_up: i32,
    pub destroyed: i64,
}

pub struct ProxyMessage {
    pub session: ProxySession,
    pub transaction: String,
}

pub struct ProxyPluginState {
    pub sessions: Vec<ProxySession>,
    pub messages: Vec<ProxyMessage>,
    pub callbacks: Arc<Mutex<Option<Box<PluginCallbacks>>>>,
}

lazy_static! {
    static ref STATE: ProxyPluginState = ProxyPluginState {
        sessions: Vec::new(),
        messages: Vec::new(),
        callbacks: Arc::new(Mutex::new(None))
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
        return -1;
    }

    let callback_mutex = Arc::clone(&STATE.callbacks);
    *callback_mutex.lock().unwrap() = Some(unsafe { Box::from_raw(callbacks) });
    janus::log(janus::LogLevel::Info, "Janus retproxy plugin initialized!\n");
    0
}

extern "C" fn destroy() {
    janus::log(janus::LogLevel::Info, "Janus retproxy plugin destroyed!\n");
}

extern "C" fn create_session(handle: *mut PluginSession, _error: *mut c_int) {
    janus::log(janus::LogLevel::Info, "Initializing retproxy session...\n");
    let session = Box::new(ProxySession {
        has_audio: false,
        has_video: false,
        has_data: false,
        audio_active: true,
        video_active: true,
        bitrate: 0,
        destroyed: 0,
        hanging_up: 0,
        slowlink_count: 0,
    });
    unsafe {
        (*handle).plugin_handle = Box::into_raw(session) as *mut c_void;
    }
}

extern "C" fn destroy_session(handle: *mut PluginSession, _error: *mut c_int) {
    janus::log(janus::LogLevel::Info, "Destroying retproxy session...\n");
    let session: &mut ProxySession = unsafe { &mut *((*handle).plugin_handle as *mut ProxySession) };
    session.destroyed = 1;
}


unsafe extern "C" fn handle_message(
    _handle: *mut PluginSession,
    _transaction: *mut c_char,
    _message: *mut janus::Json,
    _jsep: *mut janus::Json,
) -> *mut PluginResult {
    janus_internal::janus_plugin_result_new(PluginResultType::JANUS_PLUGIN_OK, ptr::null(), janus_internal::json_object())
}

unsafe extern "C" fn query_session(_handle: *mut PluginSession) -> *mut janus::Json {
    janus_internal::json_object()
}

extern "C" fn setup_media(_handle: *mut PluginSession) {}
extern "C" fn incoming_rtp(_handle: *mut PluginSession, _video: c_int, _buf: *mut c_char, _len: c_int) {}
extern "C" fn incoming_rtcp(_handle: *mut PluginSession, _video: c_int, _buf: *mut c_char, _len: c_int) {}
extern "C" fn incoming_data(_handle: *mut PluginSession, _buf: *mut c_char, _len: c_int) {}
extern "C" fn slow_link(_handle: *mut PluginSession, _uplink: c_int, _video: c_int) {}
extern "C" fn hangup_media(_handle: *mut PluginSession) {}

export_plugin!(
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
