#[macro_use]
extern crate cstr_macro;
#[macro_use]
extern crate lazy_static;
#[macro_use]
extern crate janus_plugin as janus;
#[macro_use]
extern crate serde_json;

use std::collections::{HashSet};
use std::error::Error;
use std::ffi::CString;
use std::os::raw::{c_char, c_int};
use std::ptr;
use std::sync::{mpsc, Arc, Mutex, RwLock, Weak};
use std::sync::atomic::{AtomicIsize, Ordering};
use std::thread;
use serde_json::Value as JsonValue;
use janus::{JanssonValue, RawJanssonValue,
            LogLevel, Plugin, PluginCallbacks, PluginMetadata,
            PluginResultInfo, PluginResultType, PluginHandle};
use janus::session::SessionHandle;

/// Inefficiently converts a Jansson JSON value to a serde JSON value.
pub fn to_serde_json(input: JanssonValue) -> JsonValue {
    serde_json::from_str(&input.to_string(0)).unwrap()
}

/// Inefficiently converts a serde JSON value to a Jansson JSON value.
pub fn from_serde_json(input: JsonValue) -> JanssonValue {
    JanssonValue::from_str(&input.to_string(), 0).unwrap()
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum ConnectionRole {
    Unknown,
    Publisher { user_id: i64 },
    Subscriber { user_id: i64, target_id: i64 },
}

impl ConnectionRole {
    pub fn user_id(&self) -> Option<i64> {
        match *self {
            ConnectionRole::Publisher { user_id } => Some(user_id),
            ConnectionRole::Subscriber { user_id, .. } => Some(user_id),
            ConnectionRole::Unknown => None
        }
    }
}

#[derive(Debug)]
pub struct ConnectionState {
    pub role: ConnectionRole,
}

impl ConnectionState {
    pub fn set_role(&mut self, role: ConnectionRole) -> Result<ConnectionRole, Box<Error+Send+Sync>> {
        match self.role {
            ConnectionRole::Unknown => { self.role = role; Ok(role) },
            x if x == role => Ok(x),
            _ => Err(From::from(format!("Connection role already configured as {:?}; can't set to {:?}.", self.role, role)))
        }
    }
}

pub type Connection = SessionHandle<Mutex<ConnectionState>>;

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum MessageKind {
    Join,
    List,
}

impl MessageKind {
    pub fn classify(json: &JsonValue) -> Result<Option<Self>, Box<Error+Send+Sync>> {
        if let Some(&JsonValue::String(ref kind)) = json.get("kind") {
            return Self::parse(&kind).map(|k| Some(k))
        } else {
            Ok(None)
        }
    }

    pub fn parse(name: &str) -> Result<Self, Box<Error+Send+Sync>> {
        match name {
            "join" => Ok(MessageKind::Join),
            "list" => Ok(MessageKind::List),
            _ => Err(From::from("Invalid message kind."))
        }
    }
}

#[derive(Debug)]
pub struct RawMessage {
    pub conn: Weak<Connection>,
    pub txn: *mut c_char,
    pub msg: Option<JanssonValue>,
    pub jsep: Option<JanssonValue>,
}

unsafe impl Send for RawMessage {}

type MessageProcessingError = Box<Error+Send+Sync>;
type MessageProcessingResult = Result<(), MessageProcessingError>;

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
    pub next_user_id: AtomicIsize,
}

lazy_static! {
    static ref STATE: State = State {
        connections: RwLock::new(Vec::new()),
        message_channel: Mutex::new(None),
        next_user_id: AtomicIsize::new(0)
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
        role: ConnectionRole::Unknown,
    }));
    (*STATE.connections.write().unwrap()).push(conn);
}

fn notify_publishers(myself: *mut PluginHandle, msg: JanssonValue) -> Result<(), Box<Error+Send+Sync>> {
    let connections = STATE.connections.read().unwrap();
    let push_event = gateway_callbacks().push_event;
    for other in connections.iter() {
        if other.handle != myself {
            let other_state = other.lock().unwrap();
            if let ConnectionRole::Publisher { .. } = other_state.role {
                janus::get_result(push_event(other.handle, &mut PLUGIN, ptr::null(), msg.ptr, ptr::null_mut()))?
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
    STATE.connections.write().unwrap().retain(|ref c| c.handle != handle);

    // notify all other publishers that this connection is gone
    if let ConnectionRole::Publisher { user_id } = conn_role {
        let response = from_serde_json(json!({"event": "leave", "user_id": user_id}));
        notify_publishers(ptr::null_mut(), response).unwrap_or_else(|e| {
            janus::log(LogLevel::Err, &format!("Error notifying publishers on leave: {}", e));
        });
    };
}

extern "C" fn query_session(handle: *mut PluginHandle) -> *mut RawJanssonValue {
    if handle.is_null() {
        janus::log(LogLevel::Err, "No session associated with handle!");
        return ptr::null_mut();
    }
    ptr::null_mut()
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
    if let ConnectionRole::Publisher { user_id } = conn_role {
        janus::log(LogLevel::Huge, &format!("RTP packet received from user ID {}.", user_id));
        let relay_rtp = gateway_callbacks().relay_rtp;
        let connections = STATE.connections.read().unwrap();
        for other in connections.iter() {
            let other_state = &*(other.lock().unwrap());
            match other_state.role {
                ConnectionRole::Subscriber { user_id: subscriber_id, target_id } if target_id == user_id => {
                    // this connection is subscribing to us, forward our RTP
                    janus::log(LogLevel::Huge, &format!("RTP packet forwarded from user ID {} to {}.", user_id, subscriber_id));
                    relay_rtp(other.handle, video, buf, len);
                },
                _ => {
                    // this connection doesn't care about our RTP
                }
            }
        }
    } else {
        janus::log(LogLevel::Err, "Received RTP from non-publisher. Discarding.");
    }
}

extern "C" fn incoming_rtcp(handle: *mut PluginHandle, video: c_int, buf: *mut c_char, len: c_int) {
    if handle.is_null() {
        janus::log(LogLevel::Err, "No session associated with handle!");
        return;
    }

    let conn = Arc::clone(Connection::from_ptr(handle));
    let conn_role = conn.lock().unwrap().role;
    if let ConnectionRole::Publisher { user_id } = conn_role {
        janus::log(LogLevel::Huge, &format!("RTCP packet received from user ID {}.", user_id));
        let relay_rtcp = gateway_callbacks().relay_rtcp;
        let connections = STATE.connections.read().unwrap();
        for other in connections.iter() {
            let other_state = &*(other.lock().unwrap());
            match other_state.role {
                ConnectionRole::Subscriber { user_id: subscriber_id, target_id } if target_id == user_id => {
                    // this connection is subscribing to us, forward our RTCP
                    janus::log(LogLevel::Huge, &format!("RTCP packet forwarded from user ID {} to {}.", user_id, subscriber_id));
                    relay_rtcp(other.handle, video, buf, len);
                },
                _ => {
                    // this connection doesn't care about our RTCP
                }
            }
        }
    } else {
        janus::log(LogLevel::Huge, "Received RTCP from non-publisher. Discarding.");
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
        if handle != other.handle {
            if let ConnectionRole::Publisher { .. } = other_state.role {
                relay_data(other.handle, buf, len);
            }
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

fn generate_user_id() -> i64 {
    STATE.next_user_id.fetch_add(1, Ordering::Relaxed) as i64
}

fn get_user_list() -> HashSet<i64> {
    let connections = STATE.connections.read().unwrap();
    connections.iter().filter_map(|c| c.lock().unwrap().role.user_id()).collect()
}

fn handle_join(conn: &Connection, txn: *mut c_char, msg: JsonValue) -> MessageProcessingResult {
    let user_id = match msg.get("user_id") {
        None => Ok(generate_user_id()),
        Some(&JsonValue::Number(ref existing_id)) if existing_id.is_i64() => {
            Ok(existing_id.as_i64().unwrap())
        },
        _ => Err(MessageProcessingError::from("Invalid user ID specified (must be an integer.)")),
    }?;

    let role = match msg.get("role") {
        Some(&JsonValue::String(ref role_str)) if role_str == "publisher" => {
            janus::log(LogLevel::Info, &format!("Configuring connection {:?} as publisher for {}.", conn, user_id));
            conn.lock().unwrap().set_role(ConnectionRole::Publisher { user_id })
        },
        Some(&JsonValue::String(ref role_str)) if role_str == "subscriber" => {
            if let Some(&JsonValue::Number(ref target_id)) = msg.get("target_id") {
                janus::log(LogLevel::Info, &format!("Configuring connection {:?} as subscriber from {} to {}.", conn, user_id, target_id));
                conn.lock().unwrap().set_role(ConnectionRole::Subscriber { user_id, target_id: target_id.as_i64().unwrap() })
            } else {
                Err(From::from("No target ID specified for subscription."))
            }
        },
        _ => Err(From::from("Unknown session kind specified (neither publisher nor subscriber.)"))
    }?;

    let push_event = gateway_callbacks().push_event;
    let response = from_serde_json(json!({
        "event": "join_self",
        "user_id": user_id,
        "user_ids": get_user_list()
    }));
    janus::get_result(push_event(conn.handle, &mut PLUGIN, txn, response.ptr, ptr::null_mut()))?;

    if let ConnectionRole::Publisher { .. } = role {
        notify_publishers(conn.handle, from_serde_json(json!({
            "event": "join_other",
            "user_id": user_id
        })))
    } else {
        Ok(())
    }
}

fn handle_list(conn: &Connection, txn: *mut c_char) -> MessageProcessingResult {
    let user_list = get_user_list();
    let push_event = gateway_callbacks().push_event;
    let response = from_serde_json(json!({"user_ids": user_list}));
    janus::get_result(push_event(conn.handle, &mut PLUGIN, txn, response.ptr, ptr::null_mut()))
}

fn handle_jsep(conn: &Connection, txn: *mut c_char, jsep: JsonValue) -> MessageProcessingResult {
    if let Some(&JsonValue::String(ref sdp)) = jsep.get("sdp") {
        let offer = janus::sdp::parse_sdp(CString::new(&**sdp)?)?;
        let answer = answer_sdp!(offer, janus::sdp::OfferAnswerParameters::Video, 0);
        let answer_str = janus::sdp::write_sdp(&answer);
        let answer_msg = from_serde_json(json!({}));
        let answer_jsep = from_serde_json(json!({
            "type": "answer",
            "sdp": answer_str.to_str()?
        }));
        let push_event = gateway_callbacks().push_event;
        janus::get_result(push_event(conn.handle, &mut PLUGIN, txn, answer_msg.ptr, answer_jsep.ptr))
    } else {
        Err(From::from("No SDP supplied in JSEP."))
    }
}

fn handle_message_async(RawMessage { jsep, msg, txn, conn }: RawMessage) -> MessageProcessingResult {
    match conn.upgrade() {
        Some(ref conn) => {
            // if we have a JSEP, handle it independently of whether or not we have a message
            jsep.map(to_serde_json).map_or(Ok(()), |jsep| {
                janus::log(LogLevel::Verb, &format!("Processing JSEP on connection {:?}.", conn));
                handle_jsep(conn, txn, jsep)
            })?;

            // if we have a message, handle that
            msg.map(to_serde_json).map_or(Ok(()), |msg| {
                MessageKind::classify(&msg).and_then(|x| {
                    janus::log(LogLevel::Verb, &format!("Processing {:?} on connection {:?}.", x, conn));
                    match x {
                        Some(MessageKind::Join) => handle_join(conn, txn, msg),
                        Some(MessageKind::List) => handle_list(conn, txn),
                        None => Ok(())
                    }
                })
            })
        },
        // getting messages for destroyed connections is slightly concerning,
        // because messages shouldn't be backed up for that long, so warn if it happens
        None => Ok(janus::log(LogLevel::Warn, "Message received for destroyed session; discarding.")),
    }
}

extern "C" fn handle_message(handle: *mut PluginHandle, transaction: *mut c_char,
                             message: *mut RawJanssonValue, jsep: *mut RawJanssonValue) -> *mut PluginResultInfo {
    janus::log(LogLevel::Verb, "Queueing signalling message.");
    Box::into_raw(
        if handle.is_null() {
            janus::create_result(PluginResultType::JANUS_PLUGIN_ERROR, cstr!("No handle associated with message!"), None)
        } else {
            let msg = RawMessage {
                conn: Arc::downgrade(Connection::from_ptr(handle)),
                txn: transaction,
                msg: JanssonValue::new(message),
                jsep: JanssonValue::new(jsep)
            };
            STATE.message_channel.lock().unwrap().as_ref().unwrap().send(msg).ok();
            janus::create_result(PluginResultType::JANUS_PLUGIN_OK_WAIT, cstr!("Processing."), None)
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
