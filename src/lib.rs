mod auth;
mod config;
mod messages;
mod sessions;
mod switchboard;
mod txid;

use auth::ValidatedToken;
use config::Config;
use janus_plugin::rtcp::{gen_fir, has_fir, has_pli};
use janus_plugin::sdp::{AudioCodec, MediaDirection, OfferAnswerParameters, Sdp, VideoCodec};
use janus_plugin::utils::LibcString;
use janus_plugin::{
    answer_sdp, build_plugin, export_plugin, janus_err, janus_huge, janus_info, janus_verb, janus_warn, offer_sdp, JanssonDecodingFlags, JanssonEncodingFlags,
    JanssonValue, JanusError, JanusResult, LibraryMetadata, Plugin, PluginCallbacks, PluginDataPacket, PluginResult, PluginRtcpPacket, PluginRtpPacket,
    PluginSession, RawJanssonValue, RawPluginResult,
};
use messages::{JsepKind, MessageKind, OptionalField, Subscription};
use messages::{RoomId, UserId};
use once_cell::sync::{Lazy, OnceCell};
use serde::de::DeserializeOwned;
use serde_json::json;
use serde_json::Value as JsonValue;
use sessions::{JoinKind, JoinState, Session, SessionState};
use std::collections::HashSet;
use std::error::Error;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int};
use std::path::Path;
use std::ptr;
use std::slice;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex, RwLock, Weak};
use std::thread;
use switchboard::Switchboard;
use txid::TransactionId;

// courtesy of c_string crate, which also has some other stuff we aren't interested in
// taking in as a dependency here.
macro_rules! c_str {
    ($lit:expr) => {
        unsafe { CStr::from_ptr(concat!($lit, "\0").as_ptr() as *const $crate::c_char) }
    };
}

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
fn serde_to_jansson(input: &JsonValue) -> JanssonValue {
    JanssonValue::from_str(&input.to_string(), JanssonDecodingFlags::empty()).unwrap()
}

fn jansson_to_str(json: &JanssonValue) -> Result<LibcString, Box<dyn Error>> {
    Ok(json.to_libcstring(JanssonEncodingFlags::empty()))
}

/// A response to a signalling message. May carry either a response body, a JSEP, or both.
struct MessageResponse {
    pub body: Option<JsonValue>,
    pub jsep: Option<JsonValue>, // todo: make this an Option<JsepKind>?
}

impl MessageResponse {
    fn new(body: JsonValue, jsep: JsonValue) -> Self {
        Self {
            body: Some(body),
            jsep: Some(jsep),
        }
    }
    fn msg(body: JsonValue) -> Self {
        Self { body: Some(body), jsep: None }
    }
}

/// A result which carries a signalling message response to send to a client.
type MessageResult = Result<MessageResponse, Box<dyn Error>>;

/// A result which carries a JSEP to send to a client.
type JsepResult = Result<JsonValue, Box<dyn Error>>;

/// The audio codec Janus will negotiate with all participants. Opus is cross-compatible with everything we care about.
static AUDIO_CODEC: AudioCodec = AudioCodec::Opus;

/// The video codec Janus will negotiate with all participants. H.264 is cross-compatible with modern Firefox, Chrome,
/// Safari, and Edge; VP8/9 unfortunately isn't compatible with Safari.
static VIDEO_CODEC: VideoCodec = VideoCodec::H264;

/// Function pointers to the Janus core functionality made available to our plugin.
static mut CALLBACKS: Option<&PluginCallbacks> = None;

/// Returns a ref to the callback struct provided by Janus containing function pointers to pass data back to the gateway.
fn gateway_callbacks() -> &'static PluginCallbacks {
    unsafe { CALLBACKS.expect("Callbacks not initialized -- did plugin init() succeed?") }
}

/// The data structure mapping plugin handles between publishers (people sending audio) and subscribers
/// (people in the same room who are supposed to hear the audio.)
static SWITCHBOARD: Lazy<RwLock<Switchboard>> = Lazy::new(|| RwLock::new(Switchboard::new()));

/// The producer/consumer queue storing incoming plugin messages to be processed.
static MESSAGE_SENDERS: OnceCell<Vec<mpsc::SyncSender<RawMessage>>> = OnceCell::new();

/// Counts the number of messages handled. Used for round-robin dispatching to handler threads.
static MESSAGE_COUNTER: AtomicUsize = AtomicUsize::new(0);

/// The plugin configuration, read from disk.
static CONFIG: OnceCell<Config> = OnceCell::new();

// todo: clean up duplication here

fn send_data_user<T: IntoIterator<Item = U>, U: AsRef<Session>>(json: &JsonValue, target: &UserId, everyone: T) {
    let receivers = everyone.into_iter().filter(|s| {
        let subscription_state = s.as_ref().subscription.get();
        let join_state = s.as_ref().join_state.get();
        match (subscription_state, join_state) {
            (Some(subscription), Some(joined)) => subscription.data && &joined.user_id == target,
            _ => false,
        }
    });
    send_message(json, receivers)
}

fn send_data_except<T: IntoIterator<Item = U>, U: AsRef<Session>>(json: &JsonValue, myself: &UserId, everyone: T) {
    let receivers = everyone.into_iter().filter(|s| {
        let subscription_state = s.as_ref().subscription.get();
        let join_state = s.as_ref().join_state.get();
        match (subscription_state, join_state) {
            (Some(subscription), Some(joined)) => subscription.data && &joined.user_id != myself,
            _ => false,
        }
    });
    send_message(json, receivers)
}

fn notify_user<T: IntoIterator<Item = U>, U: AsRef<Session>>(json: &JsonValue, target: &UserId, everyone: T) {
    let notifiees = everyone.into_iter().filter(|s| {
        let subscription_state = s.as_ref().subscription.get();
        let join_state = s.as_ref().join_state.get();
        match (subscription_state, join_state) {
            (Some(subscription), Some(joined)) => subscription.notifications && &joined.user_id == target,
            _ => false,
        }
    });
    send_message(json, notifiees)
}

fn notify_except<T: IntoIterator<Item = U>, U: AsRef<Session>>(json: &JsonValue, myself: &UserId, everyone: T) {
    let notifiees = everyone.into_iter().filter(|s| {
        let subscription_state = s.as_ref().subscription.get();
        let join_state = s.as_ref().join_state.get();
        match (subscription_state, join_state) {
            (Some(subscription), Some(joined)) => subscription.notifications && &joined.user_id != myself,
            _ => false,
        }
    });
    send_message(json, notifiees)
}

fn send_message<T: IntoIterator<Item = U>, U: AsRef<Session>>(body: &JsonValue, sessions: T) {
    let mut msg = serde_to_jansson(body);
    let push_event = gateway_callbacks().push_event;
    for session in sessions {
        let handle = session.as_ref().handle;
        janus_huge!("Signalling message going to {:p}: {}.", handle, body);
        let result = JanusError::from(push_event(
            handle,
            &PLUGIN as *const Plugin as *mut Plugin,
            ptr::null(),
            msg.as_mut_ref(),
            ptr::null_mut(),
        ));
        match result {
            Ok(_) => (),
            Err(JanusError { code: 458 }) => {
                // session not found -- should be unusual but not problematic
                janus_warn!("Attempted to send signalling message to missing session {:p}: {}", handle, body);
            }
            Err(e) => janus_err!("Error sending signalling message to {:p}: {}", handle, e),
        }
    }
}

fn send_offer<T: IntoIterator<Item = U>, U: AsRef<Session>>(offer: &JsonValue, sessions: T) {
    let mut msg = serde_to_jansson(&json!({}));
    let mut jsep = serde_to_jansson(offer);
    let push_event = gateway_callbacks().push_event;
    for session in sessions {
        let handle = session.as_ref().handle;
        janus_huge!("Offer going to {:p}: {}.", handle, offer);
        let result = JanusError::from(push_event(
            handle,
            &PLUGIN as *const Plugin as *mut Plugin,
            ptr::null(),
            msg.as_mut_ref(),
            jsep.as_mut_ref(),
        ));
        match result {
            Ok(_) => (),
            Err(JanusError { code: 458 }) => {
                // session not found -- should be unusual but not problematic
                janus_warn!("Attempted to send signalling message to missing session {:p}: {}", handle, offer);
            }
            Err(e) => janus_err!("Error sending signalling message to {:p}: {}", handle, e),
        }
    }
}

fn send_fir<T: IntoIterator<Item = U>, U: AsRef<Session>>(publishers: T) {
    let relay_rtcp = gateway_callbacks().relay_rtcp;
    for publisher in publishers {
        let mut seq = publisher.as_ref().fir_seq.fetch_add(1, Ordering::Relaxed) as i32;
        let mut fir = gen_fir(&mut seq);
        let mut packet = PluginRtcpPacket {
            video: 1,
            buffer: fir.as_mut_ptr(),
            length: fir.len() as i16,
        };
        relay_rtcp(publisher.as_ref().as_ptr(), &mut packet);
    }
}

fn get_config(config_root: *const c_char) -> Result<Config, Box<dyn Error>> {
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
    let message_threads = config.message_threads;
    CONFIG.set(config).expect("Big problem: config already initialized!");
    match unsafe { callbacks.as_ref() } {
        Some(c) => {
            unsafe { CALLBACKS = Some(c) };
            let mut senders = Vec::new();
            let num_threads = if message_threads == 0 {
                let cpus = num_cpus::get();
                janus_info!("message_threads is set to 0, setting it to {} (number of cpus)", cpus);
                cpus
            } else {
                message_threads
            };

            for i in 0..num_threads {
                let (messages_tx, messages_rx) = mpsc::sync_channel(0);
                senders.push(messages_tx.clone());

                thread::Builder::new()
                    .name(format!("sfu msg {}", i))
                    .spawn(move || {
                        for msg in messages_rx.iter() {
                            if let Err(e) = handle_message_async(msg) {
                                janus_err!("Error processing message: {}", e);
                            }
                        }
                    })
                    .expect("Failed to spawn message thread.");

                janus_verb!("Message processing thread {} is alive.", i);
            }

            let _ = MESSAGE_SENDERS.set(senders);

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
        destroyed: AtomicBool::new(false),
        join_state: OnceCell::new(),
        subscriber_offer: Arc::new(Mutex::new(None)),
        subscription: OnceCell::new(),
        fir_seq: AtomicIsize::new(0),
    };

    match unsafe { Session::associate(handle, initial_state) } {
        Ok(sess) => {
            janus_info!("Initializing SFU session {:p}...", sess.handle);
            SWITCHBOARD.write().expect("Switchboard is poisoned :(").connect(sess);
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
            janus_info!("Destroying SFU session {:p}...", sess.handle);
            let mut switchboard = SWITCHBOARD.write().expect("Switchboard is poisoned :(");
            switchboard.disconnect(&sess);
            if let Some(joined) = sess.join_state.get() {
                match joined.kind {
                    JoinKind::Publisher => switchboard.leave_publisher(&sess),
                    JoinKind::Subscriber => switchboard.leave_subscriber(&sess),
                }
                // if this user is entirely disconnected, notify their roommates.
                // todo: is it better if this is instead when their publisher disconnects?
                if !switchboard.is_connected(&joined.user_id) {
                    let response = json!({ "event": "leave", "user_id": &joined.user_id, "room_id": &joined.room_id });
                    let occupants = switchboard.publishers_occupying(&joined.room_id);
                    notify_except(&response, &joined.user_id, occupants);
                }
            }
            sess.destroyed.store(true, Ordering::Relaxed);
        }
        Err(e) => {
            janus_err!("{}", e);
            unsafe { *error = -1 };
        }
    }
}

extern "C" fn query_session(_handle: *mut PluginSession) -> *mut RawJanssonValue {
    let output = json!({});
    serde_to_jansson(&output).into_raw()
}

extern "C" fn setup_media(handle: *mut PluginSession) {
    let sess = unsafe { Session::from_ptr(handle).expect("Session can't be null!") };
    let switchboard = SWITCHBOARD.read().expect("Switchboard is poisoned :(");
    send_fir(switchboard.media_senders_to(&sess));
    janus_info!("WebRTC media is now available on {:p}.", sess.handle);
}

extern "C" fn incoming_rtp(handle: *mut PluginSession, packet: *mut PluginRtpPacket) {
    let sess = unsafe { Session::from_ptr(handle).expect("Session can't be null!") };
    let switchboard = SWITCHBOARD.read().expect("Switchboard lock poisoned; can't continue.");
    let relay_rtp = gateway_callbacks().relay_rtp;
    for other in switchboard.media_recipients_for(&sess) {
        relay_rtp(other.as_ptr(), packet);
    }
}

extern "C" fn incoming_rtcp(handle: *mut PluginSession, packet: *mut PluginRtcpPacket) {
    let sess = unsafe { Session::from_ptr(handle).expect("Session can't be null!") };
    let switchboard = SWITCHBOARD.read().expect("Switchboard lock poisoned; can't continue.");
    let data = unsafe { slice::from_raw_parts((*packet).buffer, (*packet).length as usize) };
    let video = unsafe { (*packet).video };
    match video {
        1 if has_pli(data) => {
            let send_pli = gateway_callbacks().send_pli;
            for publisher in switchboard.media_senders_to(&sess) {
                send_pli(publisher.as_ptr());
            }
        }
        1 if has_fir(data) => {
            send_fir(switchboard.media_senders_to(&sess));
        }
        _ => {
            let relay_rtcp = gateway_callbacks().relay_rtcp;
            for subscriber in switchboard.media_recipients_for(&sess) {
                relay_rtcp(subscriber.as_ptr(), packet);
            }
        }
    }
}

extern "C" fn incoming_data(handle: *mut PluginSession, packet: *mut PluginDataPacket) {
    let sess = unsafe { Session::from_ptr(handle).expect("Session can't be null!") };
    let switchboard = SWITCHBOARD.read().expect("Switchboard lock poisoned; can't continue.");
    let relay_data = gateway_callbacks().relay_data;
    for other in switchboard.data_recipients_for(&sess) {
        // we presume that clients have matching labels on their channels -- in our case we have one
        // reliable one called "reliable" and one unreliable one called "unreliable"
        relay_data(other.as_ptr(), packet);
    }
}

extern "C" fn data_ready(_handle: *mut PluginSession) {
    // Skip data channels.
}

extern "C" fn slow_link(handle: *mut PluginSession, _uplink: c_int, _video: c_int) {
    let sess = unsafe { Session::from_ptr(handle).expect("Session can't be null!") };
    janus_info!("Slow link message received on {:p}.", sess.handle);
}

extern "C" fn hangup_media(handle: *mut PluginSession) {
    let sess = unsafe { Session::from_ptr(handle).expect("Session can't be null!") };
    janus_info!("Hanging up WebRTC media on {:p}.", sess.handle);
}

fn process_join(from: &Arc<Session>, room_id: RoomId, user_id: UserId, subscribe: Option<Subscription>, token: Option<String>) -> MessageResult {
    // todo: holy shit clean this function up somehow
    let config = CONFIG.get().unwrap();
    match (&config.auth_key, token) {
        (None, _) => {
            janus_verb!(
                "No auth_key configured. Allowing join from {:p} to room {} as user {}.",
                from.handle,
                room_id,
                user_id
            );
        }
        (Some(_), None) => {
            janus_warn!("Rejecting anonymous join from {:p} to room {} as user {}.", from.handle, room_id, user_id);
            return Err(From::from("Rejecting anonymous join!"));
        }
        (Some(key), Some(ref token)) => match ValidatedToken::from_str(token, key) {
            Ok(ref claims) => {
                if claims.may_join(&room_id) {
                    janus_verb!("Allowing join from {:p} to room {} as user {}.", from.handle, room_id, user_id);
                } else {
                    janus_warn!("Rejecting join from {:p} to room {} as user {}.", from.handle, room_id, user_id);
                    return Err(From::from("Rejecting join without permission!"));
                }
            }
            Err(e) => {
                janus_warn!("Rejecting join from {:p} to room {} as user {}. Error: {}", from.handle, room_id, user_id, e);
                return Err(From::from("Rejecting join with invalid token!"));
            }
        },
    }

    let mut switchboard = SWITCHBOARD.write()?;
    let room_users = switchboard.get_room_users(&room_id).collect::<HashSet<_>>();
    let body = json!({ "users": { room_id.as_str(): room_users }});

    // hack -- use data channel subscription to infer this, it would probably be nicer if
    // connections announced explicitly whether they were a publisher or subscriber
    let gets_data_channel = subscribe.as_ref().map(|s| s.data).unwrap_or(false);
    let join_kind = if gets_data_channel { JoinKind::Publisher } else { JoinKind::Subscriber };

    if join_kind == JoinKind::Publisher {
        if config.max_room_size > 0 && room_users.len() >= config.max_room_size {
            return Err(From::from("Room is full."));
        }
        if config.max_ccu > 0 && switchboard.get_all_users().count() >= config.max_ccu {
            return Err(From::from("Server is full."));
        }
    }

    if let Err(_existing) = from.join_state.set(JoinState::new(join_kind, room_id.clone(), user_id.clone())) {
        return Err(From::from("Handles may only join once!"));
    }

    if join_kind == JoinKind::Publisher {
        let notification = json!({ "event": "join", "user_id": user_id, "room_id": room_id });
        switchboard.join_publisher(Arc::clone(from), user_id.clone(), room_id.clone());
        notify_except(&notification, &user_id, switchboard.publishers_occupying(&room_id));
    } else {
        switchboard.join_subscriber(Arc::clone(from), user_id.clone(), room_id.clone());
    }

    if let Some(subscription) = subscribe {
        janus_info!("Processing join-time subscription from {:p}: {:?}.", from.handle, subscription);
        if let Err(_existing) = from.subscription.set(subscription.clone()) {
            return Err(From::from("Handles may only subscribe once!"));
        };
        if let Some(ref publisher_id) = subscription.media {
            let publisher = switchboard
                .get_publisher(publisher_id)
                .ok_or("Can't subscribe to a nonexistent publisher.")?
                .clone();
            let jsep = json!({
                "type": "offer",
                "sdp": publisher.subscriber_offer.lock().unwrap().as_ref().unwrap()
            });
            switchboard.subscribe_to_user(Arc::clone(from), publisher);
            return Ok(MessageResponse::new(body, jsep));
        }
    }

    Ok(MessageResponse::msg(body))
}

fn process_kick(from: &Arc<Session>, room_id: RoomId, user_id: UserId, token: String) -> MessageResult {
    let config = CONFIG.get().unwrap();
    if let Some(ref key) = config.auth_key {
        match ValidatedToken::from_str(&token, key) {
            Ok(tok) => {
                if tok.kick_users {
                    janus_info!("Processing kick from {:p} targeting user ID {} in room ID {}.", from.handle, user_id, room_id);
                    let end_session = gateway_callbacks().end_session;
                    let switchboard = SWITCHBOARD.read()?;
                    if let Some(publisher) = switchboard.get_publisher(&user_id) {
                        janus_info!("Kicking session {:p}.", publisher.handle);
                        end_session(publisher.as_ptr());
                    }
                    if let Some(subscribers) = switchboard.get_subscribers(&user_id) {
                        for subscriber in subscribers {
                            janus_info!("Kicking session {:p}.", subscriber.handle);
                            end_session(subscriber.as_ptr());
                        }
                    }
                } else {
                    janus_warn!("Ignoring kick from {:p} because they didn't have kick permissions.", from.handle);
                }
            }
            Err(e) => {
                janus_warn!("Ignoring kick from {:p} due to invalid token: {}.", from.handle, e);
            }
        }
    } else {
        janus_warn!("Ignoring kick from {:p} because no secret was configured.", from.handle);
    }
    Ok(MessageResponse::msg(json!({})))
}

fn process_block(from: &Arc<Session>, whom: UserId) -> MessageResult {
    janus_info!("Processing block from {:p} to {}", from.handle, whom);
    if let Some(joined) = from.join_state.get() {
        let mut switchboard = SWITCHBOARD.write()?;
        if let Some(publisher) = switchboard.get_publisher(&whom) {
            let event = json!({ "event": "blocked", "by": &joined.user_id });
            notify_user(&event, &whom, &[publisher]);
        }
        switchboard.establish_block(joined.user_id.clone(), whom);
        Ok(MessageResponse::msg(json!({})))
    } else {
        Err(From::from("Cannot block when not in a room."))
    }
}

fn process_unblock(from: &Arc<Session>, whom: UserId) -> MessageResult {
    janus_info!("Processing unblock from {:p} to {}", from.handle, whom);
    if let Some(joined) = from.join_state.get() {
        let mut switchboard = SWITCHBOARD.write()?;
        switchboard.lift_block(&joined.user_id, &whom);
        if let Some(publisher) = switchboard.get_publisher(&whom) {
            send_fir(&[publisher]);
            let event = json!({ "event": "unblocked", "by": &joined.user_id });
            notify_user(&event, &whom, &[publisher]);
        }
        Ok(MessageResponse::msg(json!({})))
    } else {
        Err(From::from("Cannot unblock when not in a room."))
    }
}

fn process_subscribe(from: &Arc<Session>, what: &Subscription) -> MessageResult {
    janus_info!("Processing subscription from {:p}: {:?}", from.handle, what);
    if let Err(_existing) = from.subscription.set(what.clone()) {
        return Err(From::from("Users may only subscribe once!"));
    }

    let mut switchboard = SWITCHBOARD.write()?;
    if let Some(ref publisher_id) = what.media {
        let publisher = switchboard
            .get_publisher(publisher_id)
            .ok_or("Can't subscribe to a nonexistent publisher.")?
            .clone();
        let jsep = json!({
            "type": "offer",
            "sdp": publisher.subscriber_offer.lock().unwrap().as_ref().unwrap()
        });
        switchboard.subscribe_to_user(from.clone(), publisher);
        return Ok(MessageResponse::new(json!({}), jsep));
    }
    Ok(MessageResponse::msg(json!({})))
}

fn process_data(from: &Arc<Session>, whom: Option<UserId>, body: &str) -> MessageResult {
    janus_huge!("Processing data message from {:p}: {:?}", from.handle, body);
    let payload = json!({ "event": "data", "body": body });
    let switchboard = SWITCHBOARD.read().expect("Switchboard lock poisoned; can't continue.");
    if let Some(joined) = from.join_state.get() {
        let occupants = switchboard.publishers_occupying(&joined.room_id);
        if let Some(user_id) = whom {
            send_data_user(&payload, &user_id, occupants);
        } else {
            send_data_except(&payload, &joined.user_id, occupants);
        }
        Ok(MessageResponse::msg(json!({})))
    } else {
        Err(From::from("Cannot send data when not in a room."))
    }
}

fn process_message(from: &Arc<Session>, msg: MessageKind) -> MessageResult {
    match msg {
        MessageKind::Join {
            room_id,
            user_id,
            subscribe,
            token,
        } => process_join(from, room_id, user_id, subscribe, token),
        MessageKind::Kick { room_id, user_id, token } => process_kick(from, room_id, user_id, token),
        MessageKind::Subscribe { what } => process_subscribe(from, &what),
        MessageKind::Block { whom } => process_block(from, whom),
        MessageKind::Unblock { whom } => process_unblock(from, whom),
        MessageKind::Data { whom, body } => process_data(from, whom, &body),
    }
}

fn process_offer(from: &Session, offer: &Sdp) -> JsepResult {
    // enforce publication of the codecs that we know our client base will be compatible with
    janus_info!("Processing JSEP offer from {:p}: {:?}", from.handle, offer);
    let h264_profile = c_str!("42e01f");
    let mut answer = answer_sdp!(
        offer,
        OfferAnswerParameters::AudioCodec,
        AUDIO_CODEC.to_cstr().as_ptr(),
        OfferAnswerParameters::AudioDirection,
        MediaDirection::JANUS_SDP_RECVONLY,
        OfferAnswerParameters::VideoCodec,
        VIDEO_CODEC.to_cstr().as_ptr(),
        OfferAnswerParameters::H264Profile,
        h264_profile.as_ptr(),
        OfferAnswerParameters::VideoDirection,
        MediaDirection::JANUS_SDP_RECVONLY,
    );
    let audio_payload_type = answer.get_payload_type(AUDIO_CODEC.to_cstr());
    let video_payload_type = answer.get_payload_type_full(VIDEO_CODEC.to_cstr(), h264_profile);
    if let Some(pt) = audio_payload_type {
        // todo: figure out some more principled way to keep track of this stuff per room
        let settings = CString::new(format!("{} stereo=0; sprop-stereo=0; usedtx=1;", pt))?;
        answer.add_attribute(pt, c_str!("fmtp"), &settings);
    }

    janus_verb!("Providing answer to {:p}: {:?}", from.handle, answer);

    // it's fishy, but we provide audio and video streams to subscribers regardless of whether the client is sending
    // audio and video right now or not -- this is basically working around pains in renegotiation to do with
    // reordering/removing media streams on an existing connection. to improve this, we'll want to keep the same offer
    // around and mutate it, instead of generating a new one every time the publisher changes something.

    let mut subscriber_offer = offer_sdp!(
        ptr::null(),
        answer.c_addr as *const _,
        OfferAnswerParameters::Data,
        1,
        OfferAnswerParameters::Audio,
        1,
        OfferAnswerParameters::AudioCodec,
        AUDIO_CODEC.to_cstr().as_ptr(),
        OfferAnswerParameters::AudioPayloadType,
        audio_payload_type.unwrap_or(100),
        OfferAnswerParameters::AudioDirection,
        MediaDirection::JANUS_SDP_SENDONLY,
        OfferAnswerParameters::Video,
        1,
        OfferAnswerParameters::VideoCodec,
        VIDEO_CODEC.to_cstr().as_ptr(),
        OfferAnswerParameters::VideoPayloadType,
        video_payload_type.unwrap_or(100),
        OfferAnswerParameters::H264Profile,
        h264_profile.as_ptr(),
        OfferAnswerParameters::VideoDirection,
        MediaDirection::JANUS_SDP_SENDONLY,
    );
    if let Some(pt) = audio_payload_type {
        // todo: figure out some more principled way to keep track of this stuff per room
        let settings = CString::new(format!("{} stereo=0; sprop-stereo=0; usedtx=1;", pt))?;
        subscriber_offer.add_attribute(pt, c_str!("fmtp"), &settings);
    }
    janus_verb!("Storing subscriber offer for {:p}: {:?}", from.handle, subscriber_offer);

    let switchboard = SWITCHBOARD.read().expect("Switchboard lock poisoned; can't continue.");
    let jsep = json!({ "type": "offer", "sdp": subscriber_offer });
    send_offer(&jsep, switchboard.subscribers_to(from));
    *from.subscriber_offer.lock().unwrap() = Some(subscriber_offer);
    Ok(json!({ "type": "answer", "sdp": answer }))
}

fn process_answer(from: &Session, answer: &Sdp) -> JsepResult {
    janus_info!("Processing JSEP answer from {:p}: {:?}", from.handle, answer);
    Ok(json!({})) // todo: check that this guy should actually be sending us an answer?
}

fn process_jsep(from: &Session, jsep: JsepKind) -> JsepResult {
    match jsep {
        JsepKind::Offer { sdp } => process_offer(from, &sdp),
        JsepKind::Answer { sdp } => process_answer(from, &sdp),
    }
}

fn push_response(from: &Session, txn: &TransactionId, body: &JsonValue, jsep: Option<JsonValue>) -> JanusResult {
    let push_event = gateway_callbacks().push_event;
    let jsep = jsep.unwrap_or_else(|| json!({}));
    janus_huge!("Responding to {:p} for txid {}: body={}, jsep={}", from.handle, txn, body, jsep);
    JanusError::from(push_event(
        from.as_ptr(),
        &PLUGIN as *const Plugin as *mut Plugin,
        txn.0,
        serde_to_jansson(body).as_mut_ref(),
        serde_to_jansson(&jsep).as_mut_ref(),
    ))
}

fn try_parse_jansson<T: DeserializeOwned>(json: &JanssonValue) -> Result<Option<T>, Box<dyn Error>> {
    jansson_to_str(json).and_then(|x| OptionalField::try_parse(x.to_string_lossy()))
}

fn handle_message_async(RawMessage { jsep, msg, txn, from }: RawMessage) -> JanusResult {
    if let Some(ref from) = from.upgrade() {
        janus_huge!("Processing txid {} from {:p}: msg={:?}, jsep={:?}", txn, from.handle, msg, jsep);
        if !from.destroyed.load(Ordering::Relaxed) {
            let parsed_msg = msg.and_then(|x| try_parse_jansson(&x).transpose());
            let parsed_jsep = jsep.and_then(|x| try_parse_jansson(&x).transpose());
            let msg_result = parsed_msg.map(|x| x.and_then(|msg| process_message(from, msg)));
            let jsep_result = parsed_jsep.map(|x| x.and_then(|jsep| process_jsep(from, jsep)));
            return match (msg_result, jsep_result) {
                (Some(Err(msg_err)), _) => {
                    let resp = json!({ "success": false, "error": { "msg": format!("{}", msg_err) }});
                    push_response(from, &txn, &resp, None)
                }
                (_, Some(Err(jsep_err))) => {
                    let resp = json!({ "success": false, "error": { "msg": format!("{}", jsep_err) }});
                    push_response(from, &txn, &resp, None)
                }
                (Some(Ok(msg_resp)), None) => {
                    let msg_body = msg_resp.body.map_or(json!({ "success": true }), |x| json!({ "success": true, "response": x }));
                    push_response(from, &txn, &msg_body, msg_resp.jsep)
                }
                (None, Some(Ok(jsep_resp))) => push_response(from, &txn, &json!({ "success": true }), Some(jsep_resp)),
                (Some(Ok(msg_resp)), Some(Ok(jsep_resp))) => {
                    let msg_body = msg_resp.body.map_or(json!({ "success": true }), |x| json!({ "success": true, "response": x }));
                    push_response(from, &txn, &msg_body, Some(jsep_resp))
                }
                (None, None) => push_response(from, &txn, &json!({ "success": true }), None),
            };
        }
    }

    // getting messages for destroyed connections is slightly concerning,
    // because messages shouldn't be backed up for that long, so warn if it happens
    janus_warn!("Message with txid {} received for destroyed session; discarding.", txn);
    Ok(())
}

extern "C" fn handle_message(
    handle: *mut PluginSession,
    transaction: *mut c_char,
    message: *mut RawJanssonValue,
    jsep: *mut RawJanssonValue,
) -> *mut RawPluginResult {
    let result = match unsafe { Session::from_ptr(handle) } {
        Ok(sess) => {
            let msg = RawMessage {
                from: Arc::downgrade(&sess),
                txn: TransactionId(transaction),
                msg: unsafe { JanssonValue::from_raw(message) },
                jsep: unsafe { JanssonValue::from_raw(jsep) },
            };
            janus_verb!("Queueing signalling message on {:p}.", sess.handle);
            let message_count = MESSAGE_COUNTER.fetch_add(1, Ordering::Relaxed);
            let senders = MESSAGE_SENDERS.get().unwrap();
            let sender = &senders[message_count % senders.len()];
            sender.send(msg).ok();

            PluginResult::ok_wait(Some(c_str!("Processing.")))
        }
        Err(_) => PluginResult::error(c_str!("No handle associated with message!")),
    };
    result.into_raw()
}

extern "C" fn handle_admin_message(_message: *mut RawJanssonValue) -> *mut RawJanssonValue {
    // for now we don't use this for anything
    let output = json!({});
    serde_to_jansson(&output).into_raw()
}

static PLUGIN: Plugin = build_plugin!(
    LibraryMetadata {
        api_version: 15,
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
    handle_admin_message,
    setup_media,
    incoming_rtp,
    incoming_rtcp,
    incoming_data,
    data_ready,
    slow_link,
    hangup_media,
    destroy_session,
    query_session
);

export_plugin!(&PLUGIN);
