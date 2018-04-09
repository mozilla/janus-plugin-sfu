extern crate atom;
extern crate ini;
#[macro_use]
extern crate janus_plugin as janus;
extern crate jsonwebtoken as jwt;
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
mod auth;
mod config;

use atom::AtomSetOnce;
use messages::{RoomId, UserId};
use auth::UserToken;
use config::Config;
use janus::{JanusError, JanssonDecodingFlags, JanssonEncodingFlags, JanssonValue, Plugin, PluginCallbacks, LibraryMetadata, PluginResult, PluginSession, RawPluginResult, RawJanssonValue};
use janus::sdp::{AudioCodec, MediaDirection, OfferAnswerParameters, Sdp, VideoCodec};
use messages::{JsepKind, MessageKind, OptionalField, Subscription};
use serde_json::Value as JsonValue;
use sessions::{JoinState, Session, SessionState};
use std::collections::{HashMap, HashSet};
use std::collections::hash_map::Entry;
use std::error::Error;
use std::ffi::CStr;
use std::os::raw::{c_char, c_int};
use std::path::Path;
use std::ptr;
use std::slice;
use std::sync::{mpsc, Arc, Mutex, RwLock, Weak};
use std::sync::atomic::{Ordering, AtomicIsize};
use std::thread;
use switchboard::Switchboard;

// courtesy of c_string crate, which also has some other stuff we aren't interested in
// taking in as a dependency here.
macro_rules! c_str {
    ($lit:expr) => {
        unsafe {
            CStr::from_ptr(concat!($lit, "\0").as_ptr() as *const $crate::c_char)
        }
    }
}

/// A Janus transaction ID. Used to correlate signalling requests and responses.
#[derive(Debug)]
struct TransactionId(pub *mut c_char);

unsafe impl Send for TransactionId {}

/// A single signalling message that came in off the wire, associated with one session.
///
/// These will be queued up asynchronously and processed in order later.
#[derive(Debug)]
struct RawMessage {
    /// A reference to the sender's session. Possibly null if the session has been destroyed
    /// in between receiving and processing this message.
    pub from: Weak<Session>,

    /// The transaction ID used to mark any responses to this message.
    pub txn: TransactionId,

    /// An arbitrary message from the client. Will be deserialized as a MessageKind.
    pub msg: Option<JanssonValue>,

    /// A JSEP message (SDP offer or answer) from the client. Will be deserialized as a JsepKind.
    pub jsep: Option<JanssonValue>,
}

/// Inefficiently converts a serde JSON value to a Jansson JSON value.
fn from_serde_json(input: &JsonValue) -> JanssonValue {
    JanssonValue::from_str(&input.to_string(), JanssonDecodingFlags::empty()).unwrap()
}

/// A response to a signalling message. May carry either a response body, a JSEP, or both.
struct MessageResponse {
    pub body: Option<JsonValue>,
    pub jsep: Option<JsonValue>, // todo: make this an Option<JsepKind>?
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

static mut CALLBACKS: Option<&PluginCallbacks> = None;

/// Returns a ref to the callback struct provided by Janus containing function pointers to pass data back to the gateway.
fn gateway_callbacks() -> &'static PluginCallbacks {
    unsafe { CALLBACKS.expect("Callbacks not initialized -- did plugin init() succeed?") }
}

#[derive(Debug)]
struct State {
    // we always lock sessions, switchboard, occupants -- todo: consider making a state tuple with one RwLock containing all 3
    // to eliminate the possibility of screwing this up
    pub sessions: RwLock<Vec<Box<Arc<Session>>>>,
    pub switchboard: RwLock<Switchboard>,
    pub occupants: RwLock<HashMap<RoomId, HashSet<Arc<Session>>>>,
    pub message_channel: AtomSetOnce<Box<mpsc::SyncSender<RawMessage>>>,
    pub config: AtomSetOnce<Box<Config>>,
}

lazy_static! {
    static ref STATE: State = State {
        sessions: RwLock::new(Vec::new()),
        occupants: RwLock::new(HashMap::new()),
        switchboard: RwLock::new(Switchboard::new()),
        message_channel: AtomSetOnce::empty(),
        config: AtomSetOnce::empty(),
    };
}

// todo: this should probably be a serialize implementation on an `OccupancyMap` struct wrapping a hashmap, or something.
fn get_users(occupants: &HashMap<RoomId, HashSet<Arc<Session>>>) -> HashMap<RoomId, HashSet<UserId>> {
    let mut result = HashMap::new();
    for (room_id, sessions) in occupants {
        for session in sessions {
            if let Some(joined) = session.join_state.get() {
                result.entry(*room_id).or_insert_with(HashSet::new).insert(joined.user_id);
            }
        }
    }
    result
}

fn get_publisher<'a, T>(user_id: UserId, sessions: T) -> Option<Arc<Session>> where T: IntoIterator<Item=&'a Box<Arc<Session>>> {
    sessions.into_iter()
        .find(|s| {
            let subscriber_offer = s.subscriber_offer.lock().unwrap();
            let join_state = s.join_state.get();
            match (subscriber_offer.as_ref(), join_state) {
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

fn send_notification<'a, T>(body: &JsonValue, sessions: T) -> Result<(), JanusError> where T: IntoIterator<Item=&'a Box<Arc<Session>>> {
    let mut msg = from_serde_json(body);
    let push_event = gateway_callbacks().push_event;
    for session in sessions {
        janus_info!("Notification going to {:?}: {:?}.", session, msg);
        // probably a hack -- we shouldn't stop notifying if we fail one
        janus::get_result(push_event(session.as_ptr(), &mut PLUGIN, ptr::null(), msg.as_mut_ref(), ptr::null_mut()))?
    }
    Ok(())
}

fn send_offer<'a, T>(offer: &JsonValue, sessions: T) -> Result<(), JanusError> where T: IntoIterator<Item=&'a Arc<Session>> {
    let mut msg = from_serde_json(&json!({}));
    let mut jsep = from_serde_json(offer);
    let push_event = gateway_callbacks().push_event;
    for session in sessions {
        janus_info!("Offer going to {:?}: {:?}.", session, jsep);
        // probably a hack -- we shouldn't stop notifying if we fail one
        janus::get_result(push_event(session.as_ptr(), &mut PLUGIN, ptr::null(), msg.as_mut_ref(), jsep.as_mut_ref()))?
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
        let mut seq = publisher.fir_seq.fetch_add(1, Ordering::Relaxed) as i32;
        let mut fir = janus::rtcp::gen_fir(&mut seq);
        relay_rtcp(publisher.as_ptr(), 1, fir.as_mut_ptr(), fir.len() as i32);
    }
}

fn get_config(config_root: *const c_char) -> Result<Config, Box<Error>> {
    let config_path = unsafe { Path::new(CStr::from_ptr(config_root).to_str()?) };
    let config_file = config_path.join("janus.plugin.sfu.cfg");
    Config::from_path(config_file)
}

extern "C" fn init(callbacks: *mut PluginCallbacks, config_path: *const c_char) -> c_int {
    let config = match get_config(config_path) {
        Ok(c) => {
            janus_info!("Loaded SFU plugin configuration: {:?}", c);
            c
        }
        Err(e) => {
            janus_warn!("Error loading configuration for SFU plugin: {}", e);
            Config::default()
        }
    };
    STATE.config.set_if_none(Box::new(config));
    match unsafe { callbacks.as_ref() } {
        Some(c) => {
            unsafe { CALLBACKS = Some(c) };
            let (messages_tx, messages_rx) = mpsc::sync_channel(0);
            STATE.message_channel.set_if_none(Box::new(messages_tx));

            thread::spawn(move || {
                janus_verb!("Message processing thread is alive.");
                for msg in messages_rx.iter() {
                    janus_verb!("Processing message: {:?}", msg);
                    handle_message_async(msg).err().map(|e| {
                        janus_err!("Error processing message: {}", e);
                    });
                }
            });

            janus_info!("Janus SFU plugin initialized!");
            0
        }
        None => {
            janus_err!("Invalid parameters for SFU plugin initialization!");
            -1
        }
    }
}

extern "C" fn destroy() {
    janus_info!("Janus SFU plugin destroyed!");
}

extern "C" fn create_session(handle: *mut PluginSession, error: *mut c_int) {
    let initial_state = SessionState {
        destroyed: Mutex::new(false),
        join_state: AtomSetOnce::empty(),
        subscriber_offer: Arc::new(Mutex::new(None)),
        subscription: AtomSetOnce::empty(),
        fir_seq: AtomicIsize::new(0),
    };

    match unsafe { Session::associate(handle, initial_state) } {
        Ok(sess) => {
            janus_info!("Initializing SFU session {:?}...", sess);
            STATE.sessions.write().expect("Sessions table is poisoned :(").push(sess);
        }
        Err(e) => {
            janus_err!("{}", e);
            unsafe { *error = -1 };
        }
    }
}

extern "C" fn destroy_session(handle: *mut PluginSession, error: *mut c_int) {
    match unsafe { Session::from_ptr(handle) } {
        Ok(sess) => {
            janus_info!("Destroying SFU session {:?}...", sess);
            let mut destroyed = sess.destroyed.lock().expect("Session destruction mutex is poisoned :(");
            let mut sessions = STATE.sessions.write().expect("Sessions table is poisoned :(");
            let mut switchboard = STATE.switchboard.write().expect("Switchboard is poisoned :(");
            if let Some(joined) = sess.join_state.get() {
                let mut occupants = STATE.occupants.write().expect("Occupants are poisoned :(");
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
                        Err(e) => janus_err!("Error notifying publishers on leave: {}", e)
                    };
                }
                if let Entry::Occupied(mut cohabitators) = occupants.entry(joined.room_id) {
                    cohabitators.get_mut().remove(&sess);
                    if cohabitators.get().len() == 0 {
                        cohabitators.remove_entry();
                    }
                }
            }
            switchboard.remove_session(&sess);
            sessions.retain(|s| s.as_ptr() != handle);
            *destroyed = true;
        }
        Err(e) => {
            janus_err!("{}", e);
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
    janus_verb!("WebRTC media is now available on {:?}.", sess);
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
    let switchboard = STATE.switchboard.read().expect("Switchboard lock poisoned; can't continue.");
    let packet = unsafe { slice::from_raw_parts(buf, len as usize) };
    match video {
        1 if janus::rtcp::has_pli(packet) => {
            send_pli(&switchboard.publishers_to_user(&sess));
        }
        1 if janus::rtcp::has_fir(packet) => {
            send_fir(&switchboard.publishers_to_user(&sess));
        }
        _ => {
            let relay_rtcp = gateway_callbacks().relay_rtcp;
            for subscriber in switchboard.subscribers_to_user(&sess) {
                relay_rtcp(subscriber.as_ptr(), video, buf, len);
            }
        }
    }
}

extern "C" fn incoming_data(handle: *mut PluginSession, buf: *mut c_char, len: c_int) {
    let sess = unsafe { Session::from_ptr(handle).expect("Session can't be null!") };
    let occupants = STATE.occupants.read().expect("Occupants lock poisoned; can't continue.");
    if let Some(joined) = sess.join_state.get() {
        let relay_data = gateway_callbacks().relay_data;
        if let Some(cohabitators) = occupants.get(&joined.room_id) {
            for cohabitator in cohabitators {
                if sess != *cohabitator {
                    relay_data(cohabitator.as_ptr(), buf, len);
                }
            }
        }
    } else {
        janus_huge!("Discarding data packet from not-yet-joined peer.");
    }
}

extern "C" fn slow_link(_handle: *mut PluginSession, _uplink: c_int, _video: c_int) {
    janus_verb!("Slow link message received!");
}

extern "C" fn hangup_media(_handle: *mut PluginSession) {
    janus_verb!("Hanging up WebRTC media.");
}

fn process_join(from: &Arc<Session>, room_id: RoomId, user_id: UserId, subscribe: Option<Subscription>, _token: Option<UserToken>) -> MessageResult {
    // todo: holy shit clean this function up somehow
    let sessions = STATE.sessions.read()?;
    let mut switchboard = STATE.switchboard.write()?;
    let mut occupants = STATE.occupants.write()?;
    let body = json!({ "users": get_users(&occupants) });
    let cohabitators = occupants.entry(room_id).or_insert_with(HashSet::new);

    let already_joined = !from.join_state.is_none();
    let already_subscribed = !from.subscription.is_none();
    if already_joined {
        return Err(From::from("Handles may only join once!"))
    }
    if already_subscribed && subscribe.is_some() {
        return Err(From::from("Handles may only subscribe once!"))
    }

    let mut is_master_handle = false;
    if let Some(subscription) = subscribe {
        let max_room_size = STATE.config.get().unwrap().max_room_size;
        let room_is_full = cohabitators.len() >= max_room_size;
        is_master_handle = subscription.data; // hack -- assume there is only one "master" data connection per user
        if is_master_handle && room_is_full {
            return Err(From::from("Room is full."))
        }
    }

    from.join_state.set_if_none(Box::new(JoinState::new(room_id, user_id)));
    if let Some(subscription) = subscribe {
        from.subscription.set_if_none(Box::new(subscription));
        if is_master_handle {
            cohabitators.insert(Arc::clone(from));
            let notification = json!({ "event": "join", "user_id": user_id, "room_id": room_id });
            if let Err(e) = notify_except(&notification, user_id, &*sessions) {
                janus_err!("Error sending notification for user join: {:?}", e)
            }
        }
        if let Some(publisher_id) = subscription.media {
            let publisher = get_publisher(publisher_id, &*sessions).ok_or("Can't subscribe to a nonexistent publisher.")?;
            switchboard.subscribe_to_user(from, &publisher);
            let subscriber_offer = publisher.subscriber_offer.lock().unwrap();
            return Ok(MessageResponse::new(body, json!({
                "type": "offer",
                "sdp": subscriber_offer.as_ref().unwrap()
            })));
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
    let mut switchboard = STATE.switchboard.write()?;
    let occupants = STATE.occupants.read()?;
    let body = json!({ "users": get_users(&occupants) });

    if let Some(publisher_id) = what.media {
        let publisher = get_publisher(publisher_id, &*sessions).ok_or("Can't subscribe to a nonexistent publisher.")?;
        switchboard.subscribe_to_user(from, &publisher);
        let subscriber_offer = publisher.subscriber_offer.lock().unwrap();
        return Ok(MessageResponse::new(body, json!({
            "type": "offer",
            "sdp": subscriber_offer.as_ref().unwrap()
        })));
    }
    Ok(MessageResponse::msg(body))
}

fn process_list_users() -> MessageResult {
    let occupants = STATE.occupants.read()?;
    let body = json!({ "users": get_users(&occupants) });
    Ok(MessageResponse::msg(body))
}

fn process_message(from: &Arc<Session>, msg: &JanssonValue) -> MessageResult {
    let msg_str = msg.to_libcstring(JanssonEncodingFlags::empty());
    let msg_contents: OptionalField<MessageKind> = serde_json::from_str(msg_str.to_str()?)?;
    match msg_contents {
        OptionalField::None {} => Ok(MessageResponse::msg(json!({}))),
        OptionalField::Some(kind) => {
            janus_info!("Processing {:?} on connection {:?}.", kind, from);
            match kind {
                MessageKind::ListUsers => process_list_users(),
                MessageKind::Subscribe { what } => process_subscribe(from, what),
                MessageKind::Join { room_id, user_id, subscribe, token } => process_join(from, room_id, user_id, subscribe, token),
            }
        }
    }
}

fn process_offer(from: &Session, offer: &Sdp) -> JsepResult {
    // enforce publication of the codecs that we know our client base will be compatible with
    let answer = answer_sdp!(
        offer,
        OfferAnswerParameters::AudioCodec, AUDIO_CODEC.to_cstr().as_ptr(),
        OfferAnswerParameters::AudioDirection, MediaDirection::JANUS_SDP_RECVONLY,
        OfferAnswerParameters::VideoCodec, VIDEO_CODEC.to_cstr().as_ptr(),
        OfferAnswerParameters::VideoDirection, MediaDirection::JANUS_SDP_RECVONLY,
    );
    janus_huge!("Providing answer to {:?}: {}", from, answer.to_string().to_str().unwrap());

    // it's fishy, but we provide audio and video streams to subscribers regardless of whether the client is sending
    // audio and video right now or not -- this is basically working around pains in renegotiation to do with
    // reordering/removing media streams on an existing connection. to improve this, we'll want to keep the same offer
    // around and mutate it, instead of generating a new one every time the publisher changes something.

    let audio_payload_type = answer.get_payload_type(AUDIO_CODEC.to_cstr());
    let video_payload_type = answer.get_payload_type(VIDEO_CODEC.to_cstr());
    let subscriber_offer = offer_sdp!(
        ptr::null(),
        answer.c_addr as *const _,
        OfferAnswerParameters::Data, 1,
        OfferAnswerParameters::Audio, 1,
        OfferAnswerParameters::AudioCodec, AUDIO_CODEC.to_cstr().as_ptr(),
        OfferAnswerParameters::AudioPayloadType, audio_payload_type.unwrap_or(100),
        OfferAnswerParameters::AudioDirection, MediaDirection::JANUS_SDP_SENDONLY,
        OfferAnswerParameters::Video, 1,
        OfferAnswerParameters::VideoCodec, VIDEO_CODEC.to_cstr().as_ptr(),
        OfferAnswerParameters::VideoPayloadType, video_payload_type.unwrap_or(100),
        OfferAnswerParameters::VideoDirection, MediaDirection::JANUS_SDP_SENDONLY,
    );
    janus_huge!("Storing subscriber offer for {:?}: {}", from, subscriber_offer.to_string().to_str().unwrap());

    let switchboard = STATE.switchboard.read().expect("Switchboard lock poisoned; can't continue.");
    let jsep = json!({ "type": "offer", "sdp": subscriber_offer });
    match send_offer(&jsep, switchboard.subscribers_to_user(from).iter()) {
        Ok(_) | Err(JanusError(458 /* session not found */)) => (),
        Err(e) => janus_err!("Error notifying subscribers about new offer: {}", e)
    };
    *from.subscriber_offer.lock().unwrap() = Some(subscriber_offer);
    Ok(json!({ "type": "answer", "sdp": answer }))
}

fn process_answer(_from: &Arc<Session>, _answer: &Sdp) -> JsepResult {
    Ok(json!({})) // todo: check that this guy should actually be sending us an answer?
}

fn process_jsep(from: &Arc<Session>, jsep: &JanssonValue) -> JsepResult {
    let jsep_str = jsep.to_libcstring(JanssonEncodingFlags::empty());
    let jsep_contents: OptionalField<JsepKind> = serde_json::from_str(jsep_str.to_str()?)?;
    match jsep_contents {
        OptionalField::None {} => Ok(json!({})),
        OptionalField::Some(kind) => {
            janus_info!("Processing {:?} from {:?}.", kind, from);
            match kind {
                JsepKind::Offer { sdp } => process_offer(from, &sdp),
                JsepKind::Answer { sdp } => process_answer(from, &sdp),
            }
        }
    }
}

fn push_response(from: &Session, txn: TransactionId, body: &JsonValue, jsep: Option<JsonValue>) -> Result<(), Box<Error>> {
    let push_event = gateway_callbacks().push_event;
    let jsep = jsep.unwrap_or_else(|| json!({}));
    janus_info!("{:?} sending response to {:?}: body = {}.", from.as_ptr(), txn, body);
    Ok(janus::get_result(push_event(from.as_ptr(), &mut PLUGIN, txn.0, from_serde_json(body).as_mut_ref(), from_serde_json(&jsep).as_mut_ref()))?)
}

fn handle_message_async(RawMessage { jsep, msg, txn, from }: RawMessage) -> Result<(), Box<Error>> {
    if let Some(ref from) = from.upgrade() {
        let destroyed = from.destroyed.lock().expect("Session destruction mutex is poisoned :(");
        if !*destroyed {
            // handle the message first, because handling a JSEP can cause us to want to send an RTCP
            // FIR to our subscribers, which may have been established in the message
            let msg_result = msg.map(|x| process_message(from, &x));
            let jsep_result = jsep.map(|x| process_jsep(from, &x));
            return match (msg_result, jsep_result) {
                (Some(Err(msg_err)), _) => {
                    let resp = json!({ "success": false, "error": { "msg": format!("{}", msg_err) }});
                    push_response(from, txn, &resp, None)
                }
                (_, Some(Err(jsep_err))) => {
                    let resp = json!({ "success": false, "error": { "msg": format!("{}", jsep_err) }});
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
        }
    }

    // getting messages for destroyed connections is slightly concerning,
    // because messages shouldn't be backed up for that long, so warn if it happens
    Ok(janus_warn!("Message received for destroyed session; discarding."))
}

extern "C" fn handle_message(handle: *mut PluginSession, transaction: *mut c_char,
                             message: *mut RawJanssonValue, jsep: *mut RawJanssonValue) -> *mut RawPluginResult {
    janus_verb!("Queueing signalling message.");
    let result = match unsafe { Session::from_ptr(handle) } {
        Ok(sess) => {
            let msg = RawMessage {
                from: Arc::downgrade(&sess),
                txn: TransactionId(transaction),
                msg: unsafe { JanssonValue::from_raw(message) },
                jsep: unsafe { JanssonValue::from_raw(jsep) }
            };
            STATE.message_channel.get().unwrap().send(msg).ok();
            PluginResult::ok_wait(Some(c_str!("Processing.")))
        },
        Err(_) => PluginResult::error(c_str!("No handle associated with message!"))
    };
    result.into_raw()
}

const PLUGIN: Plugin = build_plugin!(
    LibraryMetadata {
        api_version: 9,
        version: 1,
        name: c_str!("Janus SFU plugin"),
        package: c_str!("janus.plugin.sfu"),
        version_str: c_str!(env!("CARGO_PKG_VERSION")),
        description: c_str!(env!("CARGO_PKG_DESCRIPTION")),
        author: c_str!(env!("CARGO_PKG_AUTHORS")),
    },
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
