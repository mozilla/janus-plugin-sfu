#[macro_use]
extern crate cstr_macro;

#[macro_use]
extern crate janus_plugin as janus;
extern crate janus_plugin_sys as janus_internal;

use std::ptr;
use std::os::raw::{c_char, c_int};
use janus::{PluginCallbacks, PluginMetadata, PluginResult, PluginResultType, PluginSession};

pub const METADATA: PluginMetadata = PluginMetadata {
    version: 1,
    version_str: cstr!("0.0.1"),
    description: cstr!("Janus WebRTC reverse proxy for Reticulum."),
    name: cstr!("Janus retproxy plugin"),
    author: cstr!("Marshall Quander"),
    package: cstr!("janus.plugin.retproxy")
};

unsafe extern fn init(_callback: *mut PluginCallbacks, _config_path: *const c_char) -> c_int {
    janus::log(cstr!("Janus retproxy plugin initialized!\n"));
    0
}

unsafe extern fn destroy() {
    janus::log(cstr!("Janus retproxy plugin destroyed!\n"));
}

unsafe extern fn handle_message(_handle: *mut PluginSession, _transaction: *mut c_char, _message: *mut janus::Json, _jsep: *mut janus::Json) -> *mut PluginResult {
    janus_internal::janus_plugin_result_new(PluginResultType::JANUS_PLUGIN_OK, ptr::null(), janus_internal::json_object())
}

unsafe extern fn query_session(_handle: *mut PluginSession) -> *mut janus::Json {
    janus_internal::json_object()
}

extern fn create_session(_handle: *mut PluginSession, _error: *mut c_int) {}
extern fn setup_media(_handle: *mut PluginSession) {}
extern fn incoming_rtp(_handle: *mut PluginSession, _video: c_int, _buf: *mut c_char, _len: c_int) {}
extern fn incoming_rtcp(_handle: *mut PluginSession, _video: c_int, _buf: *mut c_char, _len: c_int) {}
extern fn incoming_data(_handle: *mut PluginSession, _buf: *mut c_char, _len: c_int) {}
extern fn slow_link(_handle: *mut PluginSession, _uplink: c_int, _video: c_int) {}
extern fn hangup_media(_handle: *mut PluginSession) {}
extern fn destroy_session(_handle: *mut PluginSession, _error: *mut c_int) {}

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
