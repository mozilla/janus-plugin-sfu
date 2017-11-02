extern crate atom;
#[macro_use]
extern crate bitflags;
#[macro_use]
extern crate cstr_macro;
#[macro_use]
extern crate janus_plugin as janus;
#[macro_use]
extern crate lazy_static;
extern crate serde;
#[macro_use]
extern crate serde_derive;
#[macro_use]
extern crate serde_json;

mod messages;
mod sessions;
mod switchboard;

use atom::AtomSetOnce;
use messages::{RoomId, UserId};
use janus::{JanssonDecodingFlags, JanssonEncodingFlags, JanssonValue, LogLevel, Plugin, PluginCallbacks, PluginMetadata, PluginResult,
            PluginResultType, PluginSession, RawPluginResult, RawJanssonValue};
use janus::sdp::Sdp;
use messages::{ContentKind, JsepKind, MessageKind, OptionalField, SubscriptionSpec};
use serde_json::Result as JsonResult;
use serde_json::Value as JsonValue;
use sessions::{Session, SessionState};
use std::collections::HashSet;
use std::error::Error;
use std::ffi::CString;
use std::os::raw::{c_char, c_int};
use std::ptr;
use std::slice;
use std::sync::{mpsc, Arc, RwLock, Weak};
use std::sync::atomic::Ordering;
use std::thread;
use switchboard::Switchboard;

/// A single signalling message that came in off the wire, associated with one session.
///
/// These will be queued up asynchronously and processed in order later.
#[derive(Debug)]
pub struct RawMessage {
    /// A reference to the sender's session. Possibly null if the session has been destroyed
    /// in between receiving and processing this message.
    pub from: Weak<Session>,

    /// The transaction ID used to mark any responses to this message.
    pub txn: *mut c_char,

    /// An arbitrary message from the client. Will be deserialized as a MessageKind.
    pub msg: Option<JanssonValue>,

    /// A JSEP message (SDP offer or answer) from the client. Will be deserialized as a JsepKind.
    pub jsep: Option<JanssonValue>,
}

// covers the txn pointer -- careful that the other fields are really threadsafe!
unsafe impl Send for RawMessage {}

/// Inefficiently converts a serde JSON value to a Jansson JSON value.
fn from_serde_json(input: JsonValue) -> JanssonValue {
    JanssonValue::from_str(&input.to_string(), JanssonDecodingFlags::empty()).unwrap()
}

/// Inefficiently converts a Jansson JSON value to a serde JSON value.
fn to_serde_json<T>(input: JanssonValue) -> JsonResult<T> where T: serde::de::DeserializeOwned {
    serde_json::from_str(input.to_libcstring(JanssonEncodingFlags::empty()).to_str().unwrap())
}

/// A result which carries a signalling message to send to a client.
type MessageProcessingResult = Result<JsonValue, Box<Error>>;

/// A result which carries a JSEP offer or answer to send to a client.
type JsepResult = Result<JsonValue, Box<Error>>;

const METADATA: PluginMetadata = PluginMetadata {
    version: 1,
    version_str: cstr!("0.0.1"),
    description: cstr!("Janus SFU for game networking."),
    name: cstr!("Janus SFU plugin"),
    author: cstr!("Marshall Quander"),
    package: cstr!("janus.plugin.sfu"),
};

static mut CALLBACKS: Option<&PluginCallbacks> = None;

/// Returns a ref to the callback struct provided by Janus containing function pointers to pass data back to the gateway.
fn gateway_callbacks() -> &'static PluginCallbacks {
    unsafe { CALLBACKS.expect("Callbacks not initialized -- did plugin init() succeed?") }
}

#[derive(Debug)]
struct State {
    pub sessions: RwLock<Vec<Box<Arc<Session>>>>,
    pub switchboard: RwLock<Switchboard>,
    pub message_channel: AtomSetOnce<Box<mpsc::SyncSender<RawMessage>>>,
}

lazy_static! {
    static ref STATE: State = State {
        sessions: RwLock::new(Vec::new()),
        switchboard: RwLock::new(Switchboard::new()),
        message_channel: AtomSetOnce::empty(),
    };
}

fn get_sessions(user_id: UserId) -> HashSet<Arc<Session>> {
    STATE.sessions.read().expect("Sessions table is poisoned :(")
        .iter()
        .filter(|s| {
            match s.get() {
                Some(state) => state.user_id == user_id,
                None => false
            }
        })
        .map(|s| Arc::clone(s))
        .collect()
}

fn get_room_ids(sessions: &Vec<Box<Arc<Session>>>) -> HashSet<RoomId> {
    sessions.iter().filter_map(|s| s.get()).map(|s| s.room_id).collect()
}

fn get_user_ids(sessions: &Vec<Box<Arc<Session>>>, room_id: RoomId) -> HashSet<UserId> {
    sessions.iter().filter_map(|s| s.get()).filter(|s| s.room_id == room_id).map(|s| s.user_id).collect()
}

fn send_notification(myself: &SessionState, json: JsonValue) -> Result<(), Box<Error>> {
    janus::log(LogLevel::Info, &format!("{:?} sending notification: {}.", myself, json));
    let msg = from_serde_json(json);
    let push_event = gateway_callbacks().push_event;
    for other in STATE.sessions.read()?.iter() {
        if let Some(other_state) = other.get() {
            if other_state.user_id != myself.user_id && other_state.notify {
                janus::get_result(push_event(other.as_ptr(), &mut PLUGIN, ptr::null(), msg.as_mut_ref(), ptr::null_mut()))?
            }
        }
    }
    Ok(())
}

extern "C" fn init(callbacks: *mut PluginCallbacks, _config_path: *const c_char) -> c_int {
    match unsafe { callbacks.as_ref() } {
        Some(c) => {
            unsafe { CALLBACKS = Some(c) };
            let (messages_tx, messages_rx) = mpsc::sync_channel(0);
            STATE.message_channel.set_if_none(Box::new(messages_tx));

            thread::spawn(move || {
                janus::log(LogLevel::Verb, "Message processing thread is alive.");
                for msg in messages_rx.iter() {
                    janus::log(LogLevel::Verb, &format!("Processing message: {:?}", msg));
                    handle_message_async(msg).err().map(|e| {
                        janus::log(LogLevel::Err, &format!("Error processing message: {}", e));
                    });
                }
            });

            janus::log(LogLevel::Info, "Janus SFU plugin initialized!");
            0
        }
        None => {
            janus::log(LogLevel::Err, "Invalid parameters for SFU plugin initialization!");
            -1
        }
    }
}

extern "C" fn destroy() {
    janus::log(LogLevel::Info, "Janus SFU plugin destroyed!");
}

extern "C" fn create_session(handle: *mut PluginSession, error: *mut c_int) {
    match Session::associate(handle, AtomSetOnce::empty()) {
        Ok(sess) => {
            janus::log(LogLevel::Info, &format!("Initializing SFU session {:?}...", sess));
            STATE.sessions.write().unwrap().push(sess);
        }
        Err(e) => {
            janus::log(LogLevel::Err, &format!("{}", e));
            unsafe { *error = -1 };
        }
    }
}

extern "C" fn destroy_session(handle: *mut PluginSession, error: *mut c_int) {
    match Session::from_ptr(handle) {
        Ok(sess) => {
            janus::log(LogLevel::Info, &format!("Destroying SFU session {:?}...", sess));
            STATE.sessions.write().unwrap().retain(|ref s| s.as_ptr() != handle);

            if let Some(state) = sess.get() {
                let user_exists = STATE.sessions.read().unwrap().iter().any(|ref s| {
                    match s.get() {
                        None => false,
                        Some(other_state) => state.user_id == other_state.user_id
                    }
                });
                if !user_exists {
                    let mut switchboard = STATE.switchboard.write().unwrap();
                    switchboard.remove_session(&sess);
                    let response = json!({ "event": "leave", "user_id": state.user_id, "room_id": state.room_id });
                    send_notification(state, response).unwrap_or_else(|e| {
                        janus::log(LogLevel::Err, &format!("Error notifying publishers on leave: {}", e));
                    });
                }
            }
        }
        Err(e) => {
            janus::log(LogLevel::Err, &format!("{}", e));
            unsafe { *error = -1 };
        }
    }
}

extern "C" fn query_session(_handle: *mut PluginSession) -> *mut RawJanssonValue {
    let output = json!({
        "switchboard": *STATE.switchboard.read().expect("Switchboard is poisoned :(")
    });
    from_serde_json(output).into_raw()
}

extern "C" fn setup_media(_handle: *mut PluginSession) {
    janus::log(LogLevel::Verb, "WebRTC media is now available.");
}

extern "C" fn incoming_rtp(handle: *mut PluginSession, video: c_int, buf: *mut c_char, len: c_int) {
    let sess = Session::from_ptr(handle).expect("Session can't be null!");
    let switchboard = STATE.switchboard.read().expect("Switchboard lock poisoned; can't continue.");
    let content_kind = if video == 1 { ContentKind::VIDEO } else { ContentKind::AUDIO };
    let subscribers = switchboard.subscribers_to(&sess, Some(content_kind));
    let relay_rtp = gateway_callbacks().relay_rtp;
    janus::log(LogLevel::Huge, &format!("RTP packet received over {:?}.", sess));
    for other in subscribers {
        relay_rtp(other.as_ptr(), video, buf, len);
    }
}

extern "C" fn incoming_rtcp(handle: *mut PluginSession, video: c_int, buf: *mut c_char, len: c_int) {
    let sess = Session::from_ptr(handle).expect("Session can't be null!");
    let switchboard = STATE.switchboard.read().expect("Subscriptions lock poisoned; can't continue.");
    let content_kind = if video == 1 { ContentKind::VIDEO } else { ContentKind::AUDIO };
    let relay_rtcp = gateway_callbacks().relay_rtcp;
    if let Some(state) = sess.get() {
        janus::log(LogLevel::Dbg, &format!("RTCP packet received in {:?} from {:?} over {:?}.", state.room_id, state.user_id, sess));
        if content_kind == ContentKind::AUDIO {
            let subscribers = switchboard.subscribers_to(&sess, Some(content_kind));
            for subscriber in subscribers {
                relay_rtcp(subscriber.as_ptr(), video, buf, len);
            }
        } else if content_kind == ContentKind::VIDEO {
            let publishers = switchboard.publishers_to(&sess, Some(content_kind));
            let packet = unsafe { slice::from_raw_parts(buf, len as usize) };
            if janus::rtcp::has_pli(packet) {
                let mut pli = janus::rtcp::gen_pli();
                for publisher in publishers {
                    janus::log(LogLevel::Info, &format!("Relaying PLI."));
                    relay_rtcp(publisher.as_ptr(), video, pli.as_mut_ptr(), pli.len() as i32);
                }
            } else if janus::rtcp::has_fir(packet) {
                let mut seq = state.fir_seq.fetch_add(1, Ordering::Relaxed) as i32;
                let mut fir = janus::rtcp::gen_fir(&mut seq);
                for publisher in publishers {
                    janus::log(LogLevel::Info, &format!("Relaying FIR."));
                    relay_rtcp(publisher.as_ptr(), video, fir.as_mut_ptr(), fir.len() as i32);
                }
            }
        }
    } else {
        janus::log(LogLevel::Huge, &format!("Discarding RTCP packet from not-yet-joined peer."));
    }
}

extern "C" fn incoming_data(handle: *mut PluginSession, buf: *mut c_char, len: c_int) {
    let sess = Session::from_ptr(handle).expect("Session can't be null!");
    let sessions = STATE.sessions.read().expect("Sessions lock poisoned; can't continue.");
    if let Some(state) = sess.get() {
        janus::log(LogLevel::Dbg, &format!("Data packet received in {:?} from {:?} over {:?}.", state.room_id, state.user_id, sess));
        let relay_data = gateway_callbacks().relay_data;
        for other in sessions.iter() {
            if let Some(other_state) = other.get() {
                if other_state.room_id == state.room_id && other_state.user_id != state.user_id {
                    relay_data(other.as_ptr(), buf, len);
                }
            }
        }
    } else {
        janus::log(LogLevel::Huge, &format!("Discarding data packet from not-yet-joined peer."));
    }
}

extern "C" fn slow_link(_handle: *mut PluginSession, _uplink: c_int, _video: c_int) {
    janus::log(LogLevel::Verb, "Slow link message received!");
}

extern "C" fn hangup_media(_handle: *mut PluginSession) {
    janus::log(LogLevel::Verb, "Hanging up WebRTC media.");
}

fn process_join(from: &Arc<Session>, room_id: RoomId, user_id: UserId, notify: Option<bool>, subscription_specs: Option<Vec<SubscriptionSpec>>) -> MessageProcessingResult {
    let state = Arc::new(SessionState::new(user_id, room_id, notify.unwrap_or(false)));
    if from.set_if_none(state).is_some() {
        return Err(From::from("Users may only join once!"))
    }

    if let Some(specs) = subscription_specs {
        let mut switchboard = STATE.switchboard.write()?;
        for subscription in specs {
            let publishers = get_sessions(subscription.publisher_id);
            switchboard.subscribe(from, &publishers, subscription.content_kind);
        }
    }

    if notify == Some(true) {
        let notification = json!({ "event": "join", "user_id": user_id, "room_id": room_id });
        if let Err(e) = send_notification(from.get().unwrap(), notification) {
            janus::log(LogLevel::Err, &format!("Error sending notification for user join: {:?}", e))
        }
    }

    let sessions = STATE.sessions.read()?;
    let mut user_ids = get_user_ids(&sessions, room_id);
    user_ids.remove(&user_id);
    Ok(json!({ "user_ids": user_ids }))
}

fn process_list_users(room_id: RoomId) -> MessageProcessingResult {
    let sessions = STATE.sessions.read()?;
    Ok(json!({ "user_ids": get_user_ids(&sessions, room_id) }))
}

fn process_list_rooms() -> MessageProcessingResult {
    let sessions = STATE.sessions.read()?;
    Ok(json!({ "room_ids": get_room_ids(&sessions) }))
}

fn process_subscribe(from: &Arc<Session>, specs: Vec<SubscriptionSpec>) -> MessageProcessingResult {
    let mut switchboard = STATE.switchboard.write()?;
    for subscription in specs {
        let publishers = get_sessions(subscription.publisher_id);
        switchboard.subscribe(from, &publishers, subscription.content_kind);
    }
    Ok(json!({}))
}

fn process_unsubscribe(from: &Arc<Session>, specs: Vec<SubscriptionSpec>) -> MessageProcessingResult {
    let mut switchboard = STATE.switchboard.write()?;
    for subscription in specs {
        let publishers = get_sessions(subscription.publisher_id);
        switchboard.unsubscribe(from, &publishers, subscription.content_kind);
    }
    Ok(json!({}))
}

fn process_message(from: &Arc<Session>, msg: JanssonValue) -> MessageProcessingResult {
    match to_serde_json::<OptionalField<MessageKind>>(msg) {
        Ok(OptionalField::None {}) => Ok(json!({})),
        Ok(OptionalField::Some(kind)) => {
            janus::log(LogLevel::Info, &format!("Processing {:?} on connection {:?}.", kind, from));
            match kind {
                MessageKind::ListRooms => process_list_rooms(),
                MessageKind::ListUsers { room_id } => process_list_users(room_id),
                MessageKind::Subscribe { specs } => process_subscribe(from, specs),
                MessageKind::Unsubscribe { specs } => process_unsubscribe(from, specs),
                MessageKind::Join { room_id, user_id, notify, subscription_specs } =>
                    process_join(from, room_id, user_id, notify, subscription_specs),
            }
        }
        Err(e) => Err(Box::new(e)),
    }
}

fn process_offer(sdp: String) -> JsepResult {
    let offer = Sdp::parse(CString::new(sdp)?)?;
    let answer = answer_sdp!(offer);
    let answer_str = Sdp::to_string(&answer);
    Ok(serde_json::to_value(JsepKind::Answer { sdp: answer_str.to_str()?.to_owned() })?)
}

fn process_jsep(from: &Arc<Session>, jsep: JanssonValue) -> JsepResult {
    match to_serde_json::<OptionalField<JsepKind>>(jsep) {
        Ok(OptionalField::None {}) => Ok(json!({})),
        Ok(OptionalField::Some(kind)) => {
            janus::log(LogLevel::Info, &format!("Processing {:?} from {:?}.", kind, from));
            match kind {
                JsepKind::Offer { sdp } => process_offer(sdp),
                JsepKind::Answer { .. } => Err(From::from("JSEP answers not yet supported.")),
            }
        }
        Err(e) => Err(Box::new(e)),
    }
}

fn push_response(from: &Session, txn: *mut c_char, msg: JsonValue, jsep: Option<JsonValue>) -> Result<(), Box<Error>> {
    let push_event = gateway_callbacks().push_event;
    let jsep = jsep.unwrap_or_else(|| json!({}));
    janus::log(LogLevel::Info, &format!("{:?} sending response to {:?}: msg = {}.", from.as_ptr(), txn, msg));
    janus::get_result(push_event(from.as_ptr(), &mut PLUGIN, txn, from_serde_json(msg).as_mut_ref(), from_serde_json(jsep).as_mut_ref()))
}

fn handle_message_async(RawMessage { jsep, msg, txn, from }: RawMessage) -> Result<(), Box<Error>> {
    if let Some(ref from) = from.upgrade() {
        // if we have a JSEP, handle it independently of whether or not we have a message
        let jsep_result = jsep.map(|x| process_jsep(from, x));
        let msg_result = msg.map(|x| process_message(from, x));
        if let Some(Err(msg_err)) = msg_result {
            let resp = json!({ "success": false, "error": format!("Error processing message: {}", msg_err)});
            return push_response(from, txn, resp, None)
        }
        if let Some(Err(jsep_err)) = jsep_result {
            let resp = json!({ "success": false, "error": format!("Error processing JSEP: {}", jsep_err)});
            return push_response(from, txn, resp, None);
        }
        let msg_resp = msg_result.map_or(json!({ "success": true }), |x| {
            json!({ "success": true, "response": x.ok().unwrap() })
        });
        push_response(from, txn, msg_resp, jsep_result.map(|x| x.ok().unwrap()))
    } else {
        // getting messages for destroyed connections is slightly concerning,
        // because messages shouldn't be backed up for that long, so warn if it happens
        Ok(janus::log(LogLevel::Warn, "Message received for destroyed session; discarding."))
    }
}

extern "C" fn handle_message(handle: *mut PluginSession, transaction: *mut c_char,
                             message: *mut RawJanssonValue, jsep: *mut RawJanssonValue) -> *mut RawPluginResult {
    janus::log(LogLevel::Verb, "Queueing signalling message.");
    let result = match Session::from_ptr(handle) {
        Ok(sess) => {
            let msg = RawMessage {
                from: Arc::downgrade(&sess),
                txn: transaction,
                msg: unsafe { JanssonValue::new(message) },
                jsep: unsafe { JanssonValue::new(jsep) }
            };
            STATE.message_channel.get().unwrap().send(msg).ok();
            PluginResult::new(PluginResultType::JANUS_PLUGIN_OK_WAIT, cstr!("Processing."), Some(from_serde_json(json!({}))))
        },
        Err(_) => PluginResult::new(PluginResultType::JANUS_PLUGIN_ERROR, cstr!("No handle associated with message!"), None)
    };
    result.into_raw()
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
