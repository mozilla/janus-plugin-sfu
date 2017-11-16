extern crate atom;
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
use janus::{JanusError, JanssonDecodingFlags, JanssonEncodingFlags, JanssonValue, LogLevel, Plugin, PluginCallbacks, PluginMetadata,
            PluginResult, PluginResultType, PluginSession, RawPluginResult, RawJanssonValue};
use janus::sdp::{AudioCodec, MediaDirection, OfferAnswerParameters, Sdp, VideoCodec};
use messages::{JsepKind, MessageKind, OptionalField, Subscription};
use serde_json::Value as JsonValue;
use sessions::{JoinState, Session, SessionState};
use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::ffi::CString;
use std::os::raw::{c_char, c_int};
use std::ptr;
use std::slice;
use std::sync::{mpsc, Arc, RwLock, Weak};
use std::sync::atomic::{Ordering, AtomicIsize};
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
fn from_serde_json(input: &JsonValue) -> JanssonValue {
    JanssonValue::from_str(&input.to_string(), JanssonDecodingFlags::empty()).unwrap()
}

/// A response to a signalling message. May carry either a response body, a JSEP, or both.
pub struct MessageResponse {
    pub body: Option<JsonValue>,
    pub jsep: Option<JsonValue>, // todo: make this an Option<JsepKind>
}

impl MessageResponse {
    fn new(body: JsonValue, jsep: JsonValue) -> Self {
        Self { body: Some(body), jsep: Some(jsep) }
    }
    fn msg(body: JsonValue) -> Self {
        Self { body: Some(body), jsep: None }
    }
}

/// A result which carries a signalling message response to send to a client.
type MessageResult = Result<MessageResponse, Box<Error>>;

/// A result which carries a JSEP to send to a client.
type JsepResult = Result<JsonValue, Box<Error>>;

/// The audio codec Janus will negotiate with all participants. Opus is cross-compatible with everything we care about.
static AUDIO_CODEC: AudioCodec = AudioCodec::Opus;

/// The video codec Janus will negotiate with all participants. H.264 is cross-compatible with modern Firefox, Chrome,
/// Safari, and Edge; VP8/9 unfortunately isn't compatible with Safari.
static VIDEO_CODEC: VideoCodec = VideoCodec::H264;

/// The payload type identifier we use for audio. Basically arbitrary, but 111 is common for Opus in Chrome.
static AUDIO_PAYLOAD_TYPE: i32 = 111;

/// The payload type identifier we use for video. Basically arbitrary, but 107 is common for H.264 in Chrome.
static VIDEO_PAYLOAD_TYPE: i32 = 107;

const METADATA: PluginMetadata = PluginMetadata {
    version: 1,
    name: cstr!("Janus SFU plugin"),
    package: cstr!("janus.plugin.sfu"),
    version_str: cstr!(env!("CARGO_PKG_VERSION")),
    description: cstr!(env!("CARGO_PKG_DESCRIPTION")),
    author: cstr!(env!("CARGO_PKG_AUTHORS")),
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

fn get_users(sessions: &[Box<Arc<Session>>]) -> HashMap<RoomId, HashSet<UserId>> {
    let mut result = HashMap::new();
    for session in sessions {
        if let Some(joined) = session.join_state.get() {
            result.entry(joined.room_id).or_insert_with(HashSet::new).insert(joined.user_id);
        }
    }
    result
}

fn get_publisher<'a, T>(user_id: UserId, sessions: T) -> Option<Arc<Session>> where T: IntoIterator<Item=&'a Box<Arc<Session>>> {
    sessions.into_iter()
        .find(|s| {
            let subscriber_offer = s.subscriber_offer.get();
            let join_state = s.join_state.get();
            match (subscriber_offer, join_state) {
                (Some(_), Some(state)) if state.user_id == user_id => true,
                _ => false
            }
        })
        .map(|s| Arc::clone(s))
}

fn notify_except<'a, T>(json: &JsonValue, myself: UserId, everyone: T) -> Result<(), JanusError> where T: IntoIterator<Item=&'a Box<Arc<Session>>> {
    let notifiees = everyone.into_iter().filter(|s| {
        let subscription_state = s.subscription.get();
        let join_state = s.join_state.get();
        match (subscription_state, join_state) {
            (Some(subscription), Some(joined)) => {
                subscription.notifications && joined.user_id != myself
            }
            _ => false
        }
    });
    send_notification(json, notifiees)
}

fn send_notification<'a, T>(json: &JsonValue, sessions: T) -> Result<(), JanusError> where T: IntoIterator<Item=&'a Box<Arc<Session>>> {
    let mut msg = from_serde_json(json);
    let push_event = gateway_callbacks().push_event;
    for session in sessions {
        janus::log(LogLevel::Info, &format!("Notification going to {:?}: {:?}.", session, json));
        // probably a hack -- we shouldn't stop notifying if we fail one
        janus::get_result(push_event(session.as_ptr(), &mut PLUGIN, ptr::null(), msg.as_mut_ref(), ptr::null_mut()))?
    }
    Ok(())
}

fn send_pli<'a, T>(publishers: T) where T: IntoIterator<Item=&'a Arc<Session>> {
    let relay_rtcp = gateway_callbacks().relay_rtcp;
    for publisher in publishers {
        let mut pli = janus::rtcp::gen_pli();
        relay_rtcp(publisher.as_ptr(), 1, pli.as_mut_ptr(), pli.len() as i32);
    }
}

fn send_fir<'a, T>(publishers: T) where T: IntoIterator<Item=&'a Arc<Session>> {
    let relay_rtcp = gateway_callbacks().relay_rtcp;
    for publisher in publishers {
        janus::log(LogLevel::Warn, &format!("Publishing FIR to {:?}", publisher));
        let mut seq = publisher.fir_seq.fetch_add(1, Ordering::Relaxed) as i32;
        let mut fir = janus::rtcp::gen_fir(&mut seq);
        relay_rtcp(publisher.as_ptr(), 1, fir.as_mut_ptr(), fir.len() as i32);
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
    let initial_state = SessionState {
        join_state: AtomSetOnce::empty(),
        subscriber_offer: AtomSetOnce::empty(),
        subscription: AtomSetOnce::empty(),
        fir_seq: AtomicIsize::new(0),
    };

    match unsafe { Session::associate(handle, initial_state) } {
        Ok(sess) => {
            janus::log(LogLevel::Info, &format!("Initializing SFU session {:?}...", sess));
            STATE.sessions.write().expect("Sessions table is poisoned :(").push(sess);
        }
        Err(e) => {
            janus::log(LogLevel::Err, &format!("{}", e));
            unsafe { *error = -1 };
        }
    }
}

extern "C" fn destroy_session(handle: *mut PluginSession, error: *mut c_int) {
    match unsafe { Session::from_ptr(handle) } {
        Ok(sess) => {
            janus::log(LogLevel::Info, &format!("Destroying SFU session {:?}...", sess));
            let mut sessions = STATE.sessions.write().expect("Sessions table is poisoned :(");
            if let Some(joined) = sess.join_state.get() {
                let user_exists = sessions.iter().any(|s| {
                    let user_matches = match s.join_state.get() {
                        None => false,
                        Some(other_state) => joined.user_id == other_state.user_id
                    };
                    s.handle != sess.handle && user_matches
                });
                if !user_exists {
                    let response = json!({ "event": "leave", "user_id": joined.user_id, "room_id": joined.room_id });
                    match notify_except(&response, joined.user_id, &*sessions) {
                        Ok(_) | Err(JanusError(458 /* session not found */)) => (),
                        Err(e) => janus::log(LogLevel::Err, &format!("Error notifying publishers on leave: {}", e))
                    };
                }
            }
            STATE.switchboard.write().expect("Switchboard is poisoned :(").remove_session(&sess);
            sessions.retain(|s| s.as_ptr() != handle);
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
    from_serde_json(&output).into_raw()
}

extern "C" fn setup_media(handle: *mut PluginSession) {
    let sess = unsafe { Session::from_ptr(handle).expect("Session can't be null!") };
    let switchboard = STATE.switchboard.read().expect("Switchboard is poisoned :(");
    send_fir(&switchboard.publishers_to_user(&sess));
    janus::log(LogLevel::Verb, &format!("WebRTC media is now available on {:?}.", sess));
}

extern "C" fn incoming_rtp(handle: *mut PluginSession, video: c_int, buf: *mut c_char, len: c_int) {
    let sess = unsafe { Session::from_ptr(handle).expect("Session can't be null!") };
    let switchboard = STATE.switchboard.read().expect("Switchboard lock poisoned; can't continue.");
    let subscribers = switchboard.subscribers_to_user(&sess);
    let relay_rtp = gateway_callbacks().relay_rtp;
    for other in subscribers {
        relay_rtp(other.as_ptr(), video, buf, len);
    }
}

extern "C" fn incoming_rtcp(handle: *mut PluginSession, video: c_int, buf: *mut c_char, len: c_int) {
    let sess = unsafe { Session::from_ptr(handle).expect("Session can't be null!") };
    let switchboard = STATE.switchboard.read().expect("Subscriptions lock poisoned; can't continue.");
    let relay_rtcp = gateway_callbacks().relay_rtcp;
    let packet = unsafe { slice::from_raw_parts(buf, len as usize) };
    match video {
        1 if janus::rtcp::has_pli(packet) => {
            send_pli(&switchboard.publishers_to_user(&sess));
        }
        1 if janus::rtcp::has_fir(packet) => {
            send_fir(&switchboard.publishers_to_user(&sess));
        }
        _ => {
            for subscriber in switchboard.subscribers_to_user(&sess) {
                relay_rtcp(subscriber.as_ptr(), video, buf, len);
            }
        }
    }
}

extern "C" fn incoming_data(handle: *mut PluginSession, buf: *mut c_char, len: c_int) {
    let sess = unsafe { Session::from_ptr(handle).expect("Session can't be null!") };
    let sessions = STATE.sessions.read().expect("Sessions lock poisoned; can't continue.");
    // todo: we need a switchboard-like data structure to make this one not O(N)
    if let Some(joined) = sess.join_state.get() {
        let relay_data = gateway_callbacks().relay_data;
        for other in sessions.iter() {
            if let Some(subscription) = other.subscription.get() {
                if let Some(other_state) = other.join_state.get() {
                    if subscription.data && other_state.room_id == joined.room_id && other_state.user_id != joined.user_id {
                        relay_data(other.as_ptr(), buf, len);
                    }
                }
            }
        }
    } else {
        janus::log(LogLevel::Huge, "Discarding data packet from not-yet-joined peer.");
    }
}

extern "C" fn slow_link(_handle: *mut PluginSession, _uplink: c_int, _video: c_int) {
    janus::log(LogLevel::Verb, "Slow link message received!");
}

extern "C" fn hangup_media(_handle: *mut PluginSession) {
    janus::log(LogLevel::Verb, "Hanging up WebRTC media.");
}

fn process_join(from: &Arc<Session>, room_id: RoomId, user_id: UserId, subscribe: Option<Subscription>) -> MessageResult {
    let state = Box::new(JoinState::new(room_id, user_id));
    if from.join_state.set_if_none(state).is_some() {
        return Err(From::from("Users may only join once!"))
    }

    let sessions = STATE.sessions.read()?;
    let body = json!({ "users": get_users(sessions.as_slice()) });

    if let Some(subscription) = subscribe {
        let subscription_state = Box::new(subscription);
        if from.subscription.set_if_none(subscription_state).is_some() {
            return Err(From::from("Users may only subscribe once!"))
        }
        if subscription.data { // hack -- assume there is only one "master" data connection per user
            let notification = json!({ "event": "join", "user_id": user_id, "room_id": room_id });
            if let Err(e) = notify_except(&notification, user_id, &*sessions) {
                janus::log(LogLevel::Err, &format!("Error sending notification for user join: {:?}", e))
            }
        }
        if let Some(publisher_id) = subscription.media {
            let publisher = get_publisher(publisher_id, &*sessions).ok_or("Can't subscribe to a nonexistent publisher.")?;
            let subscriber_offer = publisher.subscriber_offer.get().ok_or("Can't subscribe to a publisher with no media.")?;
            STATE.switchboard.write()?.subscribe_to_user(from, &publisher);
            let jsep = JsepKind::Offer { sdp: subscriber_offer.to_string().to_str()?.to_owned().into() };
            let json = serde_json::from_str(&serde_json::to_string(&jsep)?)?;
            return Ok(MessageResponse::new(body, json));
        }
    }
    Ok(MessageResponse::msg(body))
}

fn process_subscribe(from: &Arc<Session>, what: Subscription) -> MessageResult {
    let subscription_state = Box::new(what);
    if from.subscription.set_if_none(subscription_state).is_some() {
        return Err(From::from("Users may only subscribe once!"))
    }

    let sessions = STATE.sessions.read()?;
    let body = json!({ "users": get_users(sessions.as_slice()) });

    if let Some(publisher_id) = what.media {
        let publisher = get_publisher(publisher_id, &*sessions).ok_or("Can't subscribe to a nonexistent publisher.")?;
        let subscriber_offer = publisher.subscriber_offer.get().ok_or("Can't subscribe to a publisher with no media.")?;
        STATE.switchboard.write()?.subscribe_to_user(from, &publisher);
        let jsep = JsepKind::Offer { sdp: subscriber_offer.to_string().to_str()?.to_owned().into() };
        let json = serde_json::from_str(&serde_json::to_string(&jsep)?)?;
        return Ok(MessageResponse::new(body, json))
    }
    Ok(MessageResponse::msg(body))
}

fn process_list_users() -> MessageResult {
    let sessions = &STATE.sessions.read()?;
    let body = json!({ "users": get_users(sessions.as_slice()) });
    Ok(MessageResponse::msg(body))
}

fn process_message(from: &Arc<Session>, msg: &JanssonValue) -> MessageResult {
    let msg_str = msg.to_libcstring(JanssonEncodingFlags::empty());
    let msg_contents: OptionalField<MessageKind> = serde_json::from_str(msg_str.to_str()?)?;
    match msg_contents {
        OptionalField::None {} => Ok(MessageResponse::msg(json!({}))),
        OptionalField::Some(kind) => {
            janus::log(LogLevel::Info, &format!("Processing {:?} on connection {:?}.", kind, from));
            match kind {
                MessageKind::ListUsers => process_list_users(),
                MessageKind::Subscribe { what } => process_subscribe(from, what),
                MessageKind::Join { room_id, user_id, subscribe } => process_join(from, room_id, user_id, subscribe),
            }
        }
    }
}

fn process_offer(from: &Session, sdp: &str) -> JsepResult {
    let offer = Sdp::parse(&CString::new(sdp)?)?;
    let mut answer = answer_sdp!(
        offer,
        OfferAnswerParameters::AudioCodec, AUDIO_CODEC.to_cstr().as_ptr(),
        OfferAnswerParameters::VideoCodec, VIDEO_CODEC.to_cstr().as_ptr(),
    );
    // todo: do we still want to rewrite payload types now that we have plugin-directed offers?
    if let Some(offer_audio_pt) = answer.get_payload_type(AUDIO_CODEC.to_cstr()) {
        answer.rewrite_payload_type(offer_audio_pt, AUDIO_PAYLOAD_TYPE);
    }
    if let Some(offer_video_pt) = answer.get_payload_type(VIDEO_CODEC.to_cstr()) {
        answer.rewrite_payload_type(offer_video_pt, VIDEO_PAYLOAD_TYPE);
    }
    let answer_str = answer.to_string();
    let subscriber_offer = Box::new(offer_sdp!(
        ptr::null(),
        answer.c_addr as *const _,
        OfferAnswerParameters::Audio, 1,
        OfferAnswerParameters::AudioCodec, AUDIO_CODEC.to_cstr().as_ptr(),
        OfferAnswerParameters::AudioPayloadType, AUDIO_PAYLOAD_TYPE,
        OfferAnswerParameters::AudioDirection, MediaDirection::JANUS_SDP_SENDONLY,
        OfferAnswerParameters::Video, 1,
        OfferAnswerParameters::VideoCodec, VIDEO_CODEC.to_cstr().as_ptr(),
        OfferAnswerParameters::VideoPayloadType, VIDEO_PAYLOAD_TYPE,
        OfferAnswerParameters::VideoDirection, MediaDirection::JANUS_SDP_SENDONLY,
        OfferAnswerParameters::Data, 0,
    ));
    if from.subscriber_offer.set_if_none(subscriber_offer).is_some() {
        return Err(From::from("Renegotiations not yet supported."))
    }
    Ok(serde_json::to_value(JsepKind::Answer { sdp: answer_str.to_str()?.into() })?)
}

fn process_answer() -> JsepResult {
    Ok(json!({})) // todo: check that this guy should actually be sending us an answer?
}

fn process_jsep(from: &Arc<Session>, jsep: &JanssonValue) -> JsepResult {
    let jsep_str = jsep.to_libcstring(JanssonEncodingFlags::empty());
    let jsep_contents: OptionalField<JsepKind> = serde_json::from_str(jsep_str.to_str()?)?;
    match jsep_contents {
        OptionalField::None {} => Ok(json!({})),
        OptionalField::Some(kind) => {
            janus::log(LogLevel::Info, &format!("Processing {:?} from {:?}.", kind, from));
            match kind {
                JsepKind::Offer { sdp } => process_offer(from, &sdp),
                JsepKind::Answer { .. } => process_answer(),
            }
        }
    }
}

fn push_response(from: &Session, txn: *mut c_char, body: &JsonValue, jsep: Option<JsonValue>) -> Result<(), Box<Error>> {
    let push_event = gateway_callbacks().push_event;
    let jsep = jsep.unwrap_or_else(|| json!({}));
    janus::log(LogLevel::Info, &format!("{:?} sending response to {:?}: body = {}.", from.as_ptr(), txn, body));
    Ok(janus::get_result(push_event(from.as_ptr(), &mut PLUGIN, txn, from_serde_json(body).as_mut_ref(), from_serde_json(&jsep).as_mut_ref()))?)
}

fn handle_message_async(RawMessage { jsep, msg, txn, from }: RawMessage) -> Result<(), Box<Error>> {
    if let Some(ref from) = from.upgrade() {
        // handle the message first, because handling a JSEP can cause us to want to send an RTCP
        // FIR to our subscribers, which may have been established in the message
        let msg_result = msg.map(|x| process_message(from, &x));
        let jsep_result = jsep.map(|x| process_jsep(from, &x));
        match (msg_result, jsep_result) {
            (Some(Err(msg_err)), _) => {
                let resp = json!({ "success": false, "error": format!("Error processing message: {}", msg_err)});
                push_response(from, txn, &resp, None)
            }
            (_, Some(Err(jsep_err))) => {
                let resp = json!({ "success": false, "error": format!("Error processing JSEP: {}", jsep_err)});
                push_response(from, txn, &resp, None)
            }
            (Some(Ok(msg_resp)), None) => {
                let msg_body = msg_resp.body.map_or(json!({ "success": true }), |x| {
                    json!({ "success": true, "response": x })
                });
                push_response(from, txn, &msg_body, msg_resp.jsep)
            }
            (None, Some(Ok(jsep_resp))) => {
                push_response(from, txn, &json!({ "success": true }), Some(jsep_resp))
            }
            (Some(Ok(msg_resp)), Some(Ok(jsep_resp))) => {
                let msg_body = msg_resp.body.map_or(json!({ "success": true }), |x| {
                    json!({ "success": true, "response": x })
                });
                push_response(from, txn, &msg_body, Some(jsep_resp))
            }
            (None, None) => {
                push_response(from, txn, &json!({ "success": true }), None)
            }
        }
    } else {
        // getting messages for destroyed connections is slightly concerning,
        // because messages shouldn't be backed up for that long, so warn if it happens
        Ok(janus::log(LogLevel::Warn, "Message received for destroyed session; discarding."))
    }
}

extern "C" fn handle_message(handle: *mut PluginSession, transaction: *mut c_char,
                             message: *mut RawJanssonValue, jsep: *mut RawJanssonValue) -> *mut RawPluginResult {
    janus::log(LogLevel::Verb, "Queueing signalling message.");
    let result = match unsafe { Session::from_ptr(handle) } {
        Ok(sess) => {
            let msg = RawMessage {
                from: Arc::downgrade(&sess),
                txn: transaction,
                msg: unsafe { JanssonValue::new(message) },
                jsep: unsafe { JanssonValue::new(jsep) }
            };
            STATE.message_channel.get().unwrap().send(msg).ok();
            PluginResult::new(PluginResultType::JANUS_PLUGIN_OK_WAIT, cstr!("Processing."), Some(from_serde_json(&json!({}))))
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
