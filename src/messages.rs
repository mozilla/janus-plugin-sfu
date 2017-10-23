/// Types and code related to handling signalling messages.
use super::JanssonValue;
use sessions::Session;
use std::os::raw::c_char;
use std::sync::Weak;

/// Useful to represent a JSON message field which may or may not be present.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OptionalField<T> {
    Some(T),
    None {}
}

/// A room ID representing a Janus multicast room.
#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct RoomId(u64);

/// A user ID representing a single Janus client. Used to correlate multiple Janus connections back to the same
/// conceptual user for managing subscriptions.
#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct UserId(u64);

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
    /// The "notify" option controls whether room notifications (e.g. join, leave) should be sent to this session.
    ///
    /// If subscriptions are specified, some initial subscriptions for this session will be configured. This is
    /// useful to save a round trip and to make sure that subscriptions are established before other clients
    /// get a join event for this user.
    Join {
        room_id: RoomId,
        user_id: UserId,
        notify: Option<bool>,
        subscription_specs: Option<Vec<SubscriptionSpec>>
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

#[cfg(test)]
mod tests {

    use super::*;

    mod jsep_parsing {

        use super::*;
        use ::serde_json;

        #[test]
        fn parse_offer() {
            let jsep = r#"{"type": "offer", "sdp": "..."}"#;
            let result: JsepKind = serde_json::from_str(jsep).unwrap();
            assert_eq!(result, JsepKind::Offer { sdp: "...".to_owned() });
        }

        #[test]
        fn parse_answer() {
            let jsep = r#"{"type": "answer", "sdp": "..."}"#;
            let result: JsepKind = serde_json::from_str(jsep).unwrap();
            assert_eq!(result, JsepKind::Answer { sdp: "...".to_owned() });
        }
    }

    mod message_parsing {

        use super::*;
        use ::serde_json;

        #[test]
        fn parse_empty() {
            let json = r#"{}"#;
            let result: OptionalField<MessageKind> = serde_json::from_str(json).unwrap();
            assert_eq!(result, OptionalField::None {});
        }

        #[test]
        fn parse_list_rooms() {
            let json = r#"{"kind": "listrooms"}"#;
            let result: MessageKind = serde_json::from_str(json).unwrap();
            assert_eq!(result, MessageKind::ListRooms);
        }

        #[test]
        fn parse_list_users() {
            let json = r#"{"kind": "listusers", "room_id": 5}"#;
            let result: MessageKind = serde_json::from_str(json).unwrap();
            assert_eq!(result, MessageKind::ListUsers { room_id: RoomId(5) });
        }

        #[test]
        fn parse_join_subscriptions() {
            let json = r#"{"kind": "join", "user_id": 10, "room_id": 5, "notify": true, "subscription_specs": []}"#;
            let result: MessageKind = serde_json::from_str(json).unwrap();
            assert_eq!(result, MessageKind::Join {
                user_id: UserId(10),
                room_id: RoomId(5),
                notify: Some(true),
                subscription_specs: Some(vec!()),
            });
        }

        #[test]
        fn parse_join_user_id() {
            let json = r#"{"kind": "join", "user_id": 10, "room_id": 5}"#;
            let result: MessageKind = serde_json::from_str(json).unwrap();
            assert_eq!(result, MessageKind::Join {
                user_id: UserId(10),
                room_id: RoomId(5),
                notify: None,
                subscription_specs: None,
            });
        }

        #[test]
        fn parse_subscribe() {
            let json = r#"{"kind": "subscribe", "specs": [{"publisher_id": 100, "content_kind": 1}]}"#;
            let result: MessageKind = serde_json::from_str(json).unwrap();
            assert_eq!(result, MessageKind::Subscribe {
                specs: vec!(SubscriptionSpec {
                    publisher_id: UserId(100),
                    content_kind: 1
                })
            });
        }

        #[test]
        fn parse_unsubscribe() {
            let json = r#"{"kind": "unsubscribe", "specs": [{"publisher_id": 100, "content_kind": 2}]}"#;
            let result: MessageKind = serde_json::from_str(json).unwrap();
            assert_eq!(result, MessageKind::Unsubscribe {
                specs: vec!(SubscriptionSpec {
                    publisher_id: UserId(100),
                    content_kind: 2
                })
            });
        }
    }
}
