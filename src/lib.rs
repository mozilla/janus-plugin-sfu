#[macro_use]
extern crate bitflags;
#[macro_use]
extern crate cstr_macro;
#[macro_use]
extern crate lazy_static;
#[macro_use]
extern crate janus_plugin as janus;
#[macro_use]
extern crate serde_json;
#[macro_use]
extern crate serde_derive;

mod userid;

use userid::{AtomicUserId, UserId};
use std::collections::{HashSet, HashMap};
use std::error::Error;
use std::ffi::CString;
use std::os::raw::{c_char, c_int};
use std::ptr;
use std::sync::{mpsc, Arc, Mutex, RwLock, Weak};
use std::sync::atomic::Ordering;
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

bitflags! {
    pub struct ContentKind: u8 {
        const AUDIO = 0b00000001;
        const VIDEO = 0b00000010;
        const DATA = 0b00000100;
    }
}

#[derive(Debug)]
pub struct Subscription {
    pub conn: Weak<Connection>,
    pub kind: ContentKind
}

#[derive(Debug)]
pub struct ConnectionState {
    pub user_id: AtomicUserId
}

pub type Connection = SessionHandle<ConnectionState>;

#[derive(Debug, Copy, Clone, Serialize, Deserialize)]
pub enum MessageKind {
    Join { user_id: Option<UserId>, role: ConnectionRole },
    List,
}

#[derive(Debug, Copy, Clone, Serialize, Deserialize)]
pub enum ConnectionRole {
    Publisher,
    Subscriber { target_id: UserId }
}

impl MessageKind {
    pub fn parse(json: JsonValue) -> Result<Option<Self>, MessageProcessingError> {
        if let Some(&JsonValue::String(ref kind)) = json.get("kind") {
            match &**kind {
                "list" => Ok(Some(MessageKind::List)),
                "join" => {
                    let user_id = match json.get("user_id") {
                        None => Ok(None),
                        Some(&JsonValue::Number(ref n)) if n.is_u64() => {
                            Ok(Some(UserId::try_from(n.as_u64().unwrap() as usize)?))
                        },
                        _ => Err(MessageProcessingError::from("Invalid user ID specified."))
                    }?;
                    let role = match json.get("role") {
                        Some(&JsonValue::String(ref r)) if r == "publisher" => Ok(ConnectionRole::Publisher),
                        Some(&JsonValue::String(ref r)) if r == "subscriber" => {
                            let target_id = match json.get("target_id") {
                                Some(&JsonValue::Number(ref n)) if n.is_u64() => {
                                    UserId::try_from(n.as_u64().unwrap() as usize)
                                },
                                _ => Err(From::from("Invalid target ID specified."))
                            }?;
                            Ok(ConnectionRole::Subscriber { target_id })
                        }
                        _ => Err(MessageProcessingError::from("Invalid role specified."))
                    }?;
                    Ok(Some(MessageKind::Join { user_id, role }))
                },
                _ => Err(From::from("Invalid message kind."))
            }
        } else {
            Ok(None)
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
    pub subscriptions: RwLock<HashMap<UserId, Vec<Subscription>>>,
    pub message_channel: Mutex<Option<mpsc::SyncSender<RawMessage>>>,
    pub next_user_id: AtomicUserId,
}

lazy_static! {
    static ref STATE: State = State {
        connections: RwLock::new(Vec::new()),
        subscriptions: RwLock::new(HashMap::new()),
        message_channel: Mutex::new(None),
        next_user_id: AtomicUserId::first()
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
    let conn = Connection::establish(handle, ConnectionState {
        user_id: AtomicUserId::empty()
    });
    (*STATE.connections.write().unwrap()).push(conn);
}

fn notify(myself: UserId, msg: JanssonValue) -> Result<(), Box<Error+Send+Sync>> {
    let connections = STATE.connections.read().unwrap();
    let push_event = gateway_callbacks().push_event;
    for other in connections.iter() {
        if other.user_id.load(Ordering::Relaxed) != Some(myself) {
            janus::get_result(push_event(other.handle, &mut PLUGIN, ptr::null(), msg.ptr, ptr::null_mut()))?
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
    let user_id = conn.user_id.load(Ordering::Relaxed);
    STATE.connections.write().unwrap().retain(|ref c| c.handle != handle);

    if let Some(user_id) = user_id {
        let user_exists = STATE.connections.read().unwrap().iter().any(|ref c| Some(user_id) == c.user_id.load(Ordering::Relaxed));
        if !user_exists {
            remove_publication(&user_id);
            let response = from_serde_json(json!({"event": "leave", "user_id": user_id}));
            notify(user_id, response).unwrap_or_else(|e| {
                janus::log(LogLevel::Err, &format!("Error notifying publishers on leave: {}", e));
            });
        }
    }
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

fn relay<T>(from: *mut PluginHandle, kind: ContentKind, send: T) -> Result<(), Box<Error+Send+Sync>> where T: Fn(&Connection) {
    if from.is_null() {
        Err(From::from("No session associated with handle!"))
    } else {
        let conn = Arc::clone(Connection::from_ptr(from));
        if let Some(user_id) = conn.user_id.load(Ordering::Relaxed) {
            janus::log(LogLevel::Dbg, &format!("Packet of kind {:?} received from {:?}.", kind, user_id));
            if let Some(subscriptions) = STATE.subscriptions.read().unwrap().get(&user_id) {
                for subscription in subscriptions {
                    if let Some(subscriber_conn) = subscription.conn.upgrade() {
                        if subscription.kind.contains(kind) {
                            janus::log(LogLevel::Dbg, &format!("Forwarding packet from {:?} to {:?}.", user_id, **subscriber_conn));
                            send(subscriber_conn.as_ref());
                        }
                    }
                }
            }
            Ok(())
        } else {
            Err(From::from("No user ID associated with connection; can't relay."))
        }
    }
}

extern "C" fn incoming_rtp(handle: *mut PluginHandle, video: c_int, buf: *mut c_char, len: c_int) {
    let content_kind = if video == 1 { ContentKind::VIDEO } else { ContentKind::AUDIO };
    let relay_rtp = gateway_callbacks().relay_rtp;
    if let Err(e) = relay(handle, content_kind, |other| { relay_rtp(other.handle, video, buf, len); }) {
        janus::log(LogLevel::Err, &format!("Discarding RTP packet: {}", e))
    }
}

extern "C" fn incoming_rtcp(handle: *mut PluginHandle, video: c_int, buf: *mut c_char, len: c_int) {
    let content_kind = if video == 1 { ContentKind::VIDEO } else { ContentKind::AUDIO };
    let relay_rtcp = gateway_callbacks().relay_rtcp;
    if let Err(e) = relay(handle, content_kind, |other| { relay_rtcp(other.handle, video, buf, len); }) {
        janus::log(LogLevel::Err, &format!("Discarding RTCP packet: {}", e))
    }
}

extern "C" fn incoming_data(handle: *mut PluginHandle, buf: *mut c_char, len: c_int) {
    let relay_data = gateway_callbacks().relay_data;
    if let Err(e) = relay(handle, ContentKind::DATA, |other| { relay_data(other.handle, buf, len); }) {
        janus::log(LogLevel::Err, &format!("Discarding data packet: {}", e))
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

fn get_user_list() -> HashSet<UserId> {
    let connections = STATE.connections.read().unwrap();
    connections.iter().filter_map(|c| c.user_id.load(Ordering::Relaxed)).collect()
}

fn add_subscription(conn: &Arc<Connection>, to: UserId, kind: ContentKind) {
    let mut subscriptions = STATE.subscriptions.write().unwrap();
    let subscribers = subscriptions.entry(to).or_insert_with(Vec::new);
    subscribers.push(Subscription { conn: Arc::downgrade(conn), kind });
}

fn remove_publication(publisher: &UserId) {
    STATE.subscriptions.write().unwrap().remove(publisher);
}

fn handle_join(conn: &Arc<Connection>, txn: *mut c_char, user_id: Option<UserId>, role: ConnectionRole) -> MessageProcessingResult {
    let other_user_ids = get_user_list();
    let user_id = match user_id {
        Some(n) => { conn.user_id.store(n, Ordering::Relaxed); n },
        None => {
            let new_user_id = STATE.next_user_id.next(Ordering::Relaxed)
                .expect("next_user_id is always a non-empty user ID.");
            conn.user_id.store(new_user_id, Ordering::Relaxed);
            let notification = from_serde_json(json!({ "event": "join_other", "user_id": new_user_id }));
            if let Err(e) = notify(new_user_id, notification) {
                janus::log(LogLevel::Err, &format!("Error sending notification for user join: {:?}", e))
            }
            new_user_id
        }
    };

    match role {
        ConnectionRole::Subscriber { target_id } => {
            add_subscription(&conn, target_id, ContentKind::AUDIO | ContentKind::VIDEO);
        }
        ConnectionRole::Publisher => {
            for other_user_id in &other_user_ids {
                add_subscription(&conn, *other_user_id, ContentKind::DATA);
            }
        },
    };

    let push_event = gateway_callbacks().push_event;
    let response = from_serde_json(json!({
        "event": "join_self",
        "user_id": user_id,
        "user_ids": other_user_ids
    }));
    janus::get_result(push_event(conn.handle, &mut PLUGIN, txn, response.ptr, ptr::null_mut()))
}

fn handle_list(conn: &Arc<Connection>, txn: *mut c_char) -> MessageProcessingResult {
    let user_list = get_user_list();
    let push_event = gateway_callbacks().push_event;
    let response = from_serde_json(json!({"user_ids": user_list}));
    janus::get_result(push_event(conn.handle, &mut PLUGIN, txn, response.ptr, ptr::null_mut()))
}

fn handle_jsep(conn: &Arc<Connection>, txn: *mut c_char, jsep: JsonValue) -> MessageProcessingResult {
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
                janus::log(LogLevel::Info, &format!("Processing JSEP on connection {:?}.", conn));
                handle_jsep(&conn, txn, jsep)
            })?;

            // if we have a message, handle that
            msg.map(to_serde_json).map_or(Ok(()), |msg| {
                MessageKind::parse(msg).and_then(|x| {
                    janus::log(LogLevel::Info, &format!("Processing {:?} on connection {:?}.", x, conn));
                    match x {
                        Some(MessageKind::List) => handle_list(&conn, txn),
                        Some(MessageKind::Join { user_id, role }) => handle_join(&conn, txn, user_id, role),
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
