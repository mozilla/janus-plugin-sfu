#[macro_use]
extern crate cstr_macro;
#[macro_use]
extern crate lazy_static;
#[macro_use]
extern crate janus_plugin as janus;
extern crate jansson_sys as jansson;
extern crate rand;

use std::collections::HashSet;
use std::error::Error;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int};
use std::ptr;
use std::sync::{Arc, Mutex, RwLock, Weak};
use std::sync::mpsc;
use std::thread;
use janus::{LogLevel, Plugin, PluginCallbacks, PluginMetadata,
            PluginResultInfo, PluginResultType, PluginHandle};
use janus::session::SessionHandle;
use jansson::json_t as Json;

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
enum ConnectionRole {
    Unknown,
    Publisher,
    Subscriber(u32),
}

#[derive(Debug)]
enum MessageKind {
    None,
    Join,
    List,
}

#[derive(Debug)]
struct ConnectionState {
    pub user_id: Option<u32>,
    pub role: ConnectionRole,
}

impl ConnectionState {
    fn set_role(&mut self, role: ConnectionRole) -> Result<(), Box<Error+Send+Sync>> {
        match self.role {
            ConnectionRole::Unknown => { self.role = role; Ok(()) },
            x if x == role => Ok(()),
            _ => Err(From::from(format!("Connection role already configured as {:?}; can't set to {:?}.", self.role, role)))
        }
    }
}

type Connection = SessionHandle<Mutex<ConnectionState>>;

#[derive(Debug)]
struct RawMessage {
    pub connection: Weak<Connection>,
    pub transaction: *mut c_char,
    pub message: *mut Json,
    pub jsep: *mut Json
}

impl RawMessage {
    pub fn classify(&self) -> Result<MessageKind, Box<Error+Send+Sync>> {
        let has_msg = !self.message.is_null();
        if !has_msg {
            return Ok(MessageKind::None);
        }
        unsafe {
            let kind_json = jansson::json_object_get(self.message, cstr!("kind"));
            if kind_json.is_null() || (*kind_json).type_ != jansson::json_type::JSON_STRING {
                return Ok(MessageKind::None);
            }
            let kind = CStr::from_ptr(jansson::json_string_value(kind_json));
            if kind == CStr::from_ptr(cstr!("join")) {
                Ok(MessageKind::Join)
            } else if kind == CStr::from_ptr(cstr!("list")) {
                Ok(MessageKind::List)
            } else {
                Err(From::from("Unknown message kind specified."))
            }
        }
    }
}

unsafe impl Send for RawMessage {}

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

#[derive(Debug)]
struct State {
    pub connections: RwLock<Vec<Box<Arc<Connection>>>>,
    pub message_channel: Mutex<Option<mpsc::SyncSender<RawMessage>>>,
}

lazy_static! {
    static ref STATE: State = State {
        connections: RwLock::new(Vec::new()),
        message_channel: Mutex::new(None)
    };
}

extern "C" fn init(callbacks: *mut PluginCallbacks, config_path: *const c_char) -> c_int {
    if callbacks.is_null() || config_path.is_null() {
        janus::log(LogLevel::Err, "Invalid parameters for retproxy plugin initialization!");
        return -1;
    }

    unsafe { CALLBACKS = callbacks.as_ref() };

    let (messages_tx, messages_rx) = mpsc::sync_channel(0);
    *(STATE.message_channel.lock().unwrap()) = Some(messages_tx);

    thread::spawn(move || {
        janus::log(LogLevel::Verb, "Message processing thread is alive.");
        for msg in messages_rx.iter() {
            janus::log(LogLevel::Verb, &format!("Processing message: {:?}", msg));
            handle_message_async(msg).err().map(|e| {
                janus::log(LogLevel::Err, &format!("Error processing message: {}", e));
            });
        }
    });

    janus::log(LogLevel::Info, "Janus retproxy plugin initialized!");
    0
}

extern "C" fn destroy() {
    janus::log(LogLevel::Info, "Janus retproxy plugin destroyed!");
}

extern "C" fn create_session(handle: *mut PluginHandle, _error: *mut c_int) {
    janus::log(LogLevel::Info, &format!("Initializing retproxy session {:?}...", unsafe { &*handle }));
    let conn = Connection::establish(handle, Mutex::new(ConnectionState {
        user_id: None,
        role: ConnectionRole::Unknown,
    }));
    (*STATE.connections.write().unwrap()).push(conn);
}

fn notify_publishers(myself: *mut PluginHandle, msg: *mut Json) -> Result<(), Box<Error+Send+Sync>> {
    let connections = STATE.connections.read().unwrap();
    let push_event = gateway_callbacks().push_event;
    for other in connections.iter() {
        if other.handle != myself {
            let other_state = other.lock().unwrap();
            if other_state.role == ConnectionRole::Publisher {
                janus::get_result(push_event(other.handle, &mut PLUGIN, ptr::null(), msg, ptr::null_mut()))?
            }
        }
    }
    Ok(())
}

extern "C" fn destroy_session(handle: *mut PluginHandle, error: *mut c_int) {
    janus::log(LogLevel::Info, "Destroying retproxy session...");
    if handle.is_null() {
        janus::log(LogLevel::Err, "No session associated with handle!");
        unsafe { *error = -1 };
        return
    }
    let conn = Arc::clone(Connection::from_ptr(handle));
    let conn_role = conn.lock().unwrap().role;
    let conn_user_id = conn.lock().unwrap().user_id;
    STATE.connections.write().unwrap().retain(|ref c| c.handle != handle);

    if conn_role == ConnectionRole::Publisher {
        if let Some(user_id) = conn_user_id {
            // notify all other publishers that this connection is gone
            unsafe {
                let response = jansson::json_object();
                jansson::json_object_set_new(response, cstr!("event"), jansson::json_string(cstr!("leave")));
                jansson::json_object_set_new(response, cstr!("user_id"), jansson::json_integer(user_id as i64));
                let result = notify_publishers(ptr::null_mut(), response);
                if let Err(err) = result {
                    janus::log(LogLevel::Err, &format!("Error notifying publishers on leave: {}", err));
                }
            }
        }
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
    if handle.is_null() {
        janus::log(LogLevel::Err, "No session associated with handle!");
        return;
    }

    let conn = Arc::clone(Connection::from_ptr(handle));
    let conn_role = conn.lock().unwrap().role;
    let conn_user_id = conn.lock().unwrap().user_id.unwrap();
    if conn_role != ConnectionRole::Publisher {
        janus::log(LogLevel::Err, &format!("Received RTP from non-publisher (user ID {:?}). Discarding.", conn_user_id));
        return;
    } else {
        janus::log(LogLevel::Huge, &format!("RTP packet received from user ID {:?}.", conn_user_id));
    }

    let relay_rtp = gateway_callbacks().relay_rtp;
    let connections = STATE.connections.read().unwrap();
    for other in connections.iter() {
        let other_state = &*(other.lock().unwrap());
        if other_state.role == ConnectionRole::Subscriber(conn_user_id) {
            relay_rtp(other.handle, video, buf, len);
        }
    }
}

extern "C" fn incoming_rtcp(handle: *mut PluginHandle, video: c_int, buf: *mut c_char, len: c_int) {
    if handle.is_null() {
        janus::log(LogLevel::Err, "No session associated with handle!");
        return;
    }

    let conn = Arc::clone(Connection::from_ptr(handle));
    let conn_role = conn.lock().unwrap().role;
    let conn_user_id = conn.lock().unwrap().user_id.unwrap();

    janus::log(LogLevel::Huge, &format!("RTCP packet received from user ID {:?}.", conn_user_id));
    if conn_role == ConnectionRole::Publisher {
        let relay_rtcp = gateway_callbacks().relay_rtcp;
        let connections = STATE.connections.read().unwrap();
        for other in connections.iter() {
            let other_state = &*(other.lock().unwrap());
            if other_state.role == ConnectionRole::Subscriber(conn_user_id) {
                relay_rtcp(other.handle, video, buf, len);
            }
        }
    }
}

extern "C" fn incoming_data(handle: *mut PluginHandle, buf: *mut c_char, len: c_int) {
    janus::log(LogLevel::Verb, "SCTP packet received!");
    if handle.is_null() {
        janus::log(LogLevel::Err, "No session associated with handle!");
        return;
    }

    let relay_data = gateway_callbacks().relay_data;
    let connections = STATE.connections.read().unwrap();
    for other in connections.iter() {
        let other_state = &*(other.lock().unwrap());
        if other_state.role == ConnectionRole::Publisher && handle != other.handle {
            relay_data(other.handle, buf, len);
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

type MessageProcessingResult = Result<(), Box<Error+Send+Sync>>;

fn user_id_taken(candidate: u32) -> bool {
    let connections = STATE.connections.read().unwrap();
    connections.iter().any(|c| c.lock().unwrap().user_id == Some(candidate))
}

fn generate_user_id() -> u32 {
    let mut candidate = rand::random::<u32>();
    while user_id_taken(candidate) {
        candidate = rand::random::<u32>();
    }
    candidate
}

fn get_user_list() -> *mut Json {
    let connections = STATE.connections.read().unwrap();
    let user_set: HashSet<u32> = connections.iter().filter_map(|c| c.lock().unwrap().user_id).collect();
    unsafe {
        let user_list = jansson::json_array();
        for user_id in user_set {
            jansson::json_array_append_new(user_list, jansson::json_integer(user_id as i64));
        }
        user_list
    }
}

fn handle_join(conn: &Connection, txn: *mut c_char, message: &Json) -> MessageProcessingResult {
    let push_event = gateway_callbacks().push_event;
    let user_list = get_user_list();
    unsafe {
        let user_id_json = jansson::json_object_get(message, cstr!("user_id"));
        let user_id = if user_id_json.is_null() {
            generate_user_id()
        } else if (*user_id_json).type_ == jansson::json_type::JSON_INTEGER {
            jansson::json_integer_value(user_id_json) as u32
        } else {
            return Err(From::from("Invalid user ID specified (must be an integer.)"))
        };
        janus::log(LogLevel::Info, &format!("Setting connection {:?} user ID to {:?}.", conn, user_id));
        conn.lock().unwrap().user_id = Some(user_id);

        let role_json = jansson::json_object_get(message, cstr!("role"));
        if !role_json.is_null() && (*role_json).type_ == jansson::json_type::JSON_STRING {
            let role = CStr::from_ptr(jansson::json_string_value(role_json));
            if role == CStr::from_ptr(cstr!("publisher")) {
                janus::log(LogLevel::Info, &format!("Configuring connection {:?} as publisher.", conn));
                conn.lock().unwrap().set_role(ConnectionRole::Publisher)?
            } else if role == CStr::from_ptr(cstr!("subscriber")) {
                let target_id_json = jansson::json_object_get(message, cstr!("target_id"));
                if !target_id_json.is_null() && (*target_id_json).type_ == jansson::json_type::JSON_INTEGER {
                    let target_id = jansson::json_integer_value(target_id_json) as u32;
                    janus::log(LogLevel::Info, &format!("Configuring connection {:?} as subscriber to {}.", conn, target_id));
                    conn.lock().unwrap().set_role(ConnectionRole::Subscriber(target_id))?
                } else {
                    return Err(From::from("No target ID specified for subscription."));
                }
            } else {
                return Err(From::from("Unknown session kind specified (neither publisher nor subscriber.)"))
            }
        }
        let response = jansson::json_object();
        jansson::json_object_set_new(response, cstr!("event"), jansson::json_string(cstr!("join_self")));
        jansson::json_object_set_new(response, cstr!("user_id"), jansson::json_integer(user_id as i64));
        jansson::json_object_set_new(response, cstr!("user_ids"), user_list);
        janus::get_result(push_event(conn.handle, &mut PLUGIN, txn, response, ptr::null_mut()))?;

        let conn_role = conn.lock().unwrap().role;
        if conn_role == ConnectionRole::Publisher {
            // notify all other publishers that this user has joined
            let response = jansson::json_object();
            jansson::json_object_set_new(response, cstr!("event"), jansson::json_string(cstr!("join_other")));
            jansson::json_object_set_new(response, cstr!("user_id"), jansson::json_integer(user_id as i64));
            notify_publishers(conn.handle, response)
        } else {
            Ok(())
        }
    }
}

fn handle_list(conn: &Connection, txn: *mut c_char) -> MessageProcessingResult {
    let user_list = get_user_list();
    let push_event = gateway_callbacks().push_event;
    unsafe {
        let response = jansson::json_object();
        jansson::json_object_set_new(response, cstr!("user_ids"), user_list);
        janus::get_result(push_event(conn.handle, &mut PLUGIN, txn, response, ptr::null_mut()))
    }
}

fn handle_jsep(conn: &Connection, txn: *mut c_char, jsep: &Json) -> MessageProcessingResult {
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
                let push_event = gateway_callbacks().push_event;
                janus::get_result(push_event(conn.handle, &mut PLUGIN, txn, jansson::json_object(), answer_jsep))
            }
        }
    }
}

fn handle_message_async(message: RawMessage) -> MessageProcessingResult {
    match message.connection.upgrade() {
        Some(ref conn) => {
            if !message.jsep.is_null() {
                handle_jsep(conn, message.transaction, unsafe { &*message.jsep })?
            }
            message.classify().and_then(|x| {
                janus::log(LogLevel::Verb, &format!("Processing {:?} on connection {:?}.", x, conn));
                match x {
                    MessageKind::Join => handle_join(conn, message.transaction, unsafe { &*message.message }),
                    MessageKind::List => handle_list(conn, message.transaction),
                    MessageKind::None => Ok(())
                }
            })
        },
        None => {
            janus::log(LogLevel::Info, "Message received for destroyed session; discarding.");
            Ok(())
        }
    }
}

extern "C" fn handle_message(handle: *mut PluginHandle, transaction: *mut c_char, message: *mut Json, jsep: *mut Json) -> *mut PluginResultInfo {
    janus::log(LogLevel::Verb, "Queueing signalling message.");
    Box::into_raw(
        if handle.is_null() {
            janus::create_result(PluginResultType::JANUS_PLUGIN_ERROR, cstr!("No handle associated with message!"), ptr::null_mut())
        } else {
            let connection = Arc::downgrade(Connection::from_ptr(handle));
            STATE.message_channel.lock().unwrap().as_ref().unwrap().send(RawMessage { connection, transaction, message, jsep }).ok();
            janus::create_result(PluginResultType::JANUS_PLUGIN_OK_WAIT, cstr!("Processing."), ptr::null_mut())
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
