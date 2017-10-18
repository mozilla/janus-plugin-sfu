/// Types and code related to handling signalling messages.
use super::JanssonValue;
use entityids::{RoomId, UserId};
use sessions::Session;
use std::os::raw::c_char;
use std::sync::Weak;

/// A signalling message carrying a JSEP SDP offer or answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase", tag = "type")]
pub enum JsepKind {
    /// A client offer to establish a connection.
    Offer { sdp: String },

    /// A client answer responding to one of our offers.
    Answer { sdp: String },
}

/// The enumeration of all (non-JSEP) signalling messages which can be received from a client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase", tag = "kind")]
pub enum MessageKind {
    /// Indicates that a client wishes to "join" a room on the server. Prior to this, no audio, video, or data
    /// received from the client will be forwarded to anyone.
    ///
    /// The first session associated with a client should pass no user ID; the server will generate
    /// an ID and return it. Subsequent sessions associated with the same client should pass the same ID.
    ///
    /// If a role is specified, subscriptions for this session will be configured apropos the given role;
    /// otherwise, this session won't be subscribed to anything.
    Join {
        room_id: RoomId,
        user_id: Option<UserId>,
    },

    /// Indicates that a client wishes to subscribe to traffic described by the given subscription specifications.
    Subscribe { specs: Vec<SubscriptionSpec> },

    /// Indicates that a client wishes to remove some previously established subscriptions.
    Unsubscribe { specs: Vec<SubscriptionSpec> },

    /// Requests a list of connected user IDs in the given room.
    ListUsers { room_id: RoomId },

    /// Requests a list of room IDs that any user is in.
    ListRooms,
}

/// Indicates that a client wishes to subscribe to all traffic coming from the given publisher of the given kind.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubscriptionSpec {
    /// The user ID to subscribe to traffic from.
    pub publisher_id: UserId,

    /// The kind or kinds of content to subscribe to.
    pub content_kind: u8, // todo: parse ContentKind directly
}

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
