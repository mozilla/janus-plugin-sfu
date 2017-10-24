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
mod subscriptions;

use atom::AtomSetOnce;
use messages::{RoomId, UserId};
use janus::{JanssonDecodingFlags, JanssonEncodingFlags, JanssonValue, LogLevel, Plugin, PluginCallbacks, PluginMetadata, PluginResultInfo,
            PluginResultType, PluginSession, RawJanssonValue};
use janus::sdp::Sdp;
use messages::{JsepKind, MessageKind, OptionalField, RawMessage, SubscriptionSpec};
use serde_json::Result as JsonResult;
use serde_json::Value as JsonValue;
use sessions::{Session, SessionState};
use std::collections::HashSet;
use std::error::Error;
use std::ffi::CString;
use std::os::raw::{c_char, c_int};
use std::ptr;
use std::sync::{mpsc, Arc, Mutex, RwLock};
use std::thread;
use subscriptions::{ContentKind, SubscriptionMap};

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
    pub subscriptions: RwLock<SubscriptionMap>,
    pub message_channel: AtomSetOnce<Box<mpsc::SyncSender<RawMessage>>>,
}

lazy_static! {
    static ref STATE: State = State {
        sessions: RwLock::new(Vec::new()),
        subscriptions: RwLock::new(SubscriptionMap::new()),
        message_channel: AtomSetOnce::empty(),
    };
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
                janus::get_result(push_event(other.handle, &mut PLUGIN, ptr::null(), msg.as_mut_ref(), ptr::null_mut()))?
            }
        }
    }
    Ok(())
}

fn send_data<T>(from: *mut PluginSession, send: T) -> Result<(), Box<Error>> where T: Fn(&Session) {
    let sess = Session::from_ptr(from)?;
    if let Some(state) = sess.get() {
        janus::log(LogLevel::Dbg, &format!("Data packet received in room {:?} from {:?}.", state.room_id, state.user_id));
        for other in STATE.sessions.read()?.iter() {
            if let Some(other_state) = other.get() {
                if other_state.room_id == state.room_id && other_state.user_id != state.user_id {
                    send(&other)
                }
            }
        }
    }
    Ok(())
}

fn publish<T>(from: *mut PluginSession, kind: Option<ContentKind>, send: T) -> Result<(), Box<Error>> where T: Fn(&Session) {
    let sess = Session::from_ptr(from)?;
    if let Some(state) = sess.get() {
        janus::log(LogLevel::Dbg, &format!("Packet of kind {:?} received in room {:?} from {:?}.", kind, state.room_id, state.user_id));
        let subscriptions = STATE.subscriptions.read()?;
        for subscription in subscriptions::subscribers_to(&subscriptions, state.user_id, kind) {
            if let Some(subscriber) = subscription.sess.upgrade() {
                if let Some(subscriber_state) = subscriber.get() {
                    if state.user_id != subscriber_state.user_id {
                        // if there's a cross-room subscription, relay it -- presume the client knows what it's doing.
                        send(&subscriber);
                    }
                }
            }
        }
        Ok(())
    } else {
        Err(From::from("Session not yet configured; can't relay."))
    }
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
            STATE.sessions.write().unwrap().retain(|ref s| s.handle != handle);

            if let Some(state) = sess.get() {
                let user_exists = STATE.sessions.read().unwrap().iter().any(|ref s| {
                    match s.get() {
                        None => false,
                        Some(other_state) => state.user_id == other_state.user_id
                    }
                });
                if !user_exists {
                    let mut subscriptions = STATE.subscriptions.write().unwrap();
                    subscriptions::unpublish(&mut subscriptions, state.user_id);
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
    ptr::null_mut()
}

extern "C" fn setup_media(_handle: *mut PluginSession) {
    janus::log(LogLevel::Verb, "WebRTC media is now available.");
}

extern "C" fn incoming_rtp(handle: *mut PluginSession, video: c_int, buf: *mut c_char, len: c_int) {
    let content_kind = if video == 1 { ContentKind::VIDEO } else { ContentKind::AUDIO };
    let relay_rtp = gateway_callbacks().relay_rtp;
    if let Err(e) = publish(handle, Some(content_kind), |other| { relay_rtp(other.handle, video, buf, len); }) {
        janus::log(LogLevel::Huge, &format!("Discarding RTP packet: {}", e))
    }
}

extern "C" fn incoming_rtcp(handle: *mut PluginSession, video: c_int, buf: *mut c_char, len: c_int) {
    let content_kind = if video == 1 { ContentKind::VIDEO } else { ContentKind::AUDIO };
    let relay_rtcp = gateway_callbacks().relay_rtcp;
    if let Err(e) = publish(handle, Some(content_kind), |other| { relay_rtcp(other.handle, video, buf, len); }) {
        janus::log(LogLevel::Huge, &format!("Discarding RTCP packet: {}", e))
    }
}

extern "C" fn incoming_data(handle: *mut PluginSession, buf: *mut c_char, len: c_int) {
    let relay_data = gateway_callbacks().relay_data;
    if let Err(e) = send_data(handle, |other| { relay_data(other.handle, buf, len); }) {
        janus::log(LogLevel::Huge, &format!("Discarding data packet: {}", e))
    }
}

extern "C" fn slow_link(_handle: *mut PluginSession, _uplink: c_int, _video: c_int) {
    janus::log(LogLevel::Verb, "Slow link message received!");
}

extern "C" fn hangup_media(_handle: *mut PluginSession) {
    janus::log(LogLevel::Verb, "Hanging up WebRTC media.");
}

fn process_join(from: &Arc<Session>, room_id: RoomId, user_id: UserId, notify: Option<bool>, subscription_specs: Option<Vec<SubscriptionSpec>>) -> MessageProcessingResult {
    let state = Box::new(SessionState {
        user_id,
        room_id,
        notify: notify.unwrap_or(false)
    });

    if from.set_if_none(state).is_some() {
        return Err(From::from("Users may only join once!"))
    }

    let subscription_specs = subscription_specs.unwrap_or_else(Vec::new);
    let mut subscriptions = STATE.subscriptions.write()?;
    subscriptions::subscribe_all(&mut subscriptions, from, &subscription_specs)?;
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
    let mut subscriptions = STATE.subscriptions.write()?;
    subscriptions::subscribe_all(&mut subscriptions, from, &specs).map(|_| json!({}))
}

fn process_unsubscribe(from: &Arc<Session>, specs: Vec<SubscriptionSpec>) -> MessageProcessingResult {
    let mut subscriptions = STATE.subscriptions.write()?;
    subscriptions::unsubscribe_all(&mut subscriptions, from, &specs).map(|_| json!({}))
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
    let answer = answer_sdp!(offer, janus::sdp::OfferAnswerParameters::Video, 0);
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
    janus::log(LogLevel::Info, &format!("{:?} sending response to {:?}: msg = {}.", from.handle, txn, msg));
    janus::get_result(push_event(from.handle, &mut PLUGIN, txn, from_serde_json(msg).as_mut_ref(), from_serde_json(jsep).as_mut_ref()))
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
                             message: *mut RawJanssonValue, jsep: *mut RawJanssonValue) -> *mut PluginResultInfo {
    janus::log(LogLevel::Verb, "Queueing signalling message.");
    Box::into_raw(
        match Session::from_ptr(handle) {
            Ok(sess) => {
                let msg = RawMessage {
                    from: Arc::downgrade(&sess),
                    txn: transaction,
                    msg: unsafe { JanssonValue::new(message) },
                    jsep: unsafe { JanssonValue::new(jsep) }
                };
                STATE.message_channel.get().unwrap().send(msg).ok();
                janus::create_result(PluginResultType::JANUS_PLUGIN_OK_WAIT, cstr!("Processing."), None)
            },
            Err(_) => janus::create_result(PluginResultType::JANUS_PLUGIN_ERROR, cstr!("No handle associated with message!"), None)
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
