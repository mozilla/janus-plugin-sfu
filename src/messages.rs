/// Types and code related to handling signalling messages.

use userid::UserId;
use sessions::Session;
use std::os::raw::c_char;
use std::sync::Weak;
use super::JanssonValue;

/// A signalling message carrying a JSEP SDP offer or answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase", tag = "type")]
pub enum JsepKind {

    /// A client offer to establish a connection.
    Offer { sdp: String },

    /// A client answer responding to one of our offers.
    Answer { sdp: String }
}

/// The enumeration of all (non-JSEP) signalling messages which can be received from a client.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase", tag = "kind")]
pub enum MessageKind {

    /// Indicates that a client wishes to "join" the server. Prior to this, no audio, video, or data
    /// received from the client will be forwarded to anyone.
    ///
    /// The first session associated with a client should pass no user ID; the server will generate
    /// an ID and return it. Subsequent sessions associated with the same client should pass the same ID.
    ///
    /// If a role is specified, subscriptions for this session will be configured apropos the given role;
    /// otherwise, this session won't be subscribed to anything.
    Join { user_id: Option<UserId>, role: SessionRole },

    /// Indicates that a client wishes to subscribe to all traffic coming from the publisher_id of the given kind.
    /// If publisher_id is not specified, then the subscription is for all traffic of this kind from all users.
    Subscribe { publisher_id: Option<UserId>, content_kind: u8 }, // todo: parse ContentKind directly

    /// Indicates that a client wishes to remove a previously established subscription.
    Unsubscribe { publisher_id: Option<UserId>, content_kind: u8 }, // todo: parse ContentKind directly

    /// Requests a list of currently connected user IDs from the server.
    List,
}

/// Shorthands for establishing a default set of subscriptions associated with a session.
///
/// These are designed to suit the current common case for clients, where one peer connection
/// (a "publisher") has all data traffic between Janus and the client and the client's outgoing A/V,
/// and N additional connections ("subscribers") which carry audio and voice for one other client each.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase", tag = "kind")]
pub enum SessionRole {

    /// Subscribe to data from all users.
    Publisher,

    /// Subscribe to the audio and video of the target user.
    Subscriber { publisher_id: UserId }
}

/// A single signalling message that came in off the wire, associated with one session.
///
/// These will be queued up asynchronously and processed in order later.
#[derive(Debug)]
pub struct RawMessage {

    /// A reference to the session state. Possibly null if the session has been destroyed
    /// in between receiving and processing this message.
    pub sess: Weak<Session>,

    /// The transaction ID used to mark any responses to this message.
    pub txn: *mut c_char,

    /// An arbitrary message from the client. Will be deserialized as a MessageKind.
    pub msg: Option<JanssonValue>,

    /// A JSEP message (SDP offer or answer) from the client. Will be deserialized as a JsepKind.
    pub jsep: Option<JanssonValue>,
}

unsafe impl Send for RawMessage {}
