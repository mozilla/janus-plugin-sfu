#[macro_use]
extern crate cstr_macro;
#[macro_use]
extern crate lazy_static;
#[macro_use]
extern crate janus_plugin as janus;
extern crate jansson_sys as jansson;
extern crate rand;

use std::collections::{HashMap, HashSet};
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
    Master,
    Listener,
}

#[derive(Debug)]
enum MessageKind {
    None,
    Join,
    List,
    Subscribe,
    Unsubscribe,
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
            } else if kind == CStr::from_ptr(cstr!("subscribe")) {
                Ok(MessageKind::Subscribe)
            } else if kind == CStr::from_ptr(cstr!("unsubscribe")) {
                Ok(MessageKind::Unsubscribe)
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
    pub subscriptions: Mutex<HashMap<i32, Vec<i32>>>,
    pub message_channel: Mutex<Option<mpsc::SyncSender<RawMessage>>>,
}

lazy_static! {
    static ref STATE: State = State {
        connections: RwLock::new(Vec::new()),
        subscriptions: Mutex::new(HashMap::new()),
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

extern "C" fn destroy_session(handle: *mut PluginHandle, error: *mut c_int) {
    janus::log(LogLevel::Info, "Destroying retproxy session...");
    if handle.is_null() {
        janus::log(LogLevel::Err, "No session associated with handle!");
        unsafe { *error = -1 };
        return
    }
    (*STATE.connections.write().unwrap()).retain(|ref c| c.handle != handle);
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

    let connections = &*STATE.connections.read().unwrap();
    for other in connections {
        if handle != other.handle {
            (gateway_callbacks().relay_data)(other.handle, buf, len);
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

fn handle_join(conn: &Connection, txn: *mut c_char, message: &Json) -> MessageProcessingResult {
    let push_event = gateway_callbacks().push_event;
    unsafe {
        let user_id_json = jansson::json_object_get(message, cstr!("user_id"));
        if user_id_json.is_null() {
            let user_id = generate_user_id();
            janus::log(LogLevel::Verb, &format!("Setting connection {:?} user ID to {:?}.", conn, user_id));
            conn.lock().unwrap().user_id = Some(user_id);
            let response = jansson::json_object();
            jansson::json_object_set_new(response, cstr!("user_id"), jansson::json_integer(user_id as i64));
            janus::get_result(push_event(conn.handle, &mut PLUGIN, txn, response, ptr::null_mut()))?
        } else if (*user_id_json).type_ == jansson::json_type::JSON_INTEGER {
            let user_id = jansson::json_integer_value(user_id_json) as u32;
            janus::log(LogLevel::Verb, &format!("Setting connection {:?} user ID to {:?}.", conn, user_id));
            conn.lock().unwrap().user_id = Some(user_id);
            let response = jansson::json_object();
            jansson::json_object_set_new(response, cstr!("user_id"), jansson::json_integer(user_id as i64));
            janus::get_result(push_event(conn.handle, &mut PLUGIN, txn, response, ptr::null_mut()))?
        }

        let role_json = jansson::json_object_get(message, cstr!("role"));
        if !role_json.is_null() && (*role_json).type_ == jansson::json_type::JSON_STRING {
            let role = CStr::from_ptr(jansson::json_string_value(role_json));
            if role == CStr::from_ptr(cstr!("master")) {
                janus::log(LogLevel::Verb, &format!("Configuring connection {:?} as master.", conn));
                conn.lock().unwrap().set_role(ConnectionRole::Master)?
            } else if role == CStr::from_ptr(cstr!("listener")) {
                janus::log(LogLevel::Verb, &format!("Configuring connection {:?} as listener.", conn));
                conn.lock().unwrap().set_role(ConnectionRole::Listener)?
            } else {
                return Err(From::from("Unknown session kind specified (neither publisher nor listener.)"))
            }
        }
        Ok(())
    }
}

fn handle_list(conn: &Connection, txn: *mut c_char) -> MessageProcessingResult {
    let connections = STATE.connections.read().unwrap();
    let user_set: HashSet<u32> = connections.iter().filter_map(|c| c.lock().unwrap().user_id).collect();
    let push_event = gateway_callbacks().push_event;
    unsafe {
        let response = jansson::json_object();
        let xs = jansson::json_array();
        for user_id in user_set {
            jansson::json_array_append_new(xs, jansson::json_integer(user_id as i64));
        }
        jansson::json_object_set_new(response, cstr!("user_ids"), xs);
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
                    MessageKind::Subscribe => Ok(()),
                    MessageKind::Unsubscribe => Ok(()),
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
