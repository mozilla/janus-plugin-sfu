/// Types and code related to handling signalling messages.
use super::serde::de::{self, Deserialize, Deserializer, Unexpected, Visitor};
use super::serde::ser::{self, Serialize, Serializer};
use std::fmt;

bitflags! {
    /// A particular kind of traffic transported over a connection.
    pub struct ContentKind: u8 {
        /// Audio traffic.
        const AUDIO = 0b00000001;
        /// Video traffic.
        const VIDEO = 0b00000010;
        /// All traffic.
        const ALL = 0b11111111;
    }
}

impl Serialize for ContentKind {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error> where S: Serializer {
        let name = match *self {
            ContentKind::AUDIO => Ok("audio"),
            ContentKind::VIDEO => Ok("video"),
            ContentKind::ALL => Ok("all"),
            _ => Err(ser::Error::custom("Unexpected content kind."))
        }?;
        serializer.serialize_str(name)
    }
}

impl<'de> Deserialize<'de> for ContentKind {
    /// Deserializes a ContentKind value from the lowercase string naming the value (as if ContentKind were a C-style enum.)
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error> where D: Deserializer<'de> {
        struct ContentKindVisitor;
        impl<'de> Visitor<'de> for ContentKindVisitor {
            type Value = ContentKind;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("`audio`, `video`, or `all`")
            }

            fn visit_str<E>(self, value: &str) -> Result<ContentKind, E> where E: de::Error {
                match value {
                    "audio" => Ok(ContentKind::AUDIO),
                    "video" => Ok(ContentKind::VIDEO),
                    "all" => Ok(ContentKind::ALL),
                    _ => Err(de::Error::invalid_value(Unexpected::Str(value), &self))
                }
            }
        }
        deserializer.deserialize_identifier(ContentKindVisitor)
    }
}

/// A room ID representing a Janus multicast room.
#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct RoomId(u64);

/// A user ID representing a single Janus client. Used to correlate multiple Janus connections back to the same
/// conceptual user for managing subscriptions.
#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct UserId(u64);

/// Useful to represent a JSON message field which may or may not be present.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OptionalField<T> {
    Some(T),
    None {}
}

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
    pub content_kind: ContentKind,
}

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
            let json = r#"{"kind": "subscribe", "specs": [{"publisher_id": 100, "content_kind": "audio"}]}"#;
            let result: MessageKind = serde_json::from_str(json).unwrap();
            assert_eq!(result, MessageKind::Subscribe {
                specs: vec!(SubscriptionSpec {
                    publisher_id: UserId(100),
                    content_kind: ContentKind::AUDIO,
                })
            });
        }

        #[test]
        fn parse_unsubscribe() {
            let json = r#"{"kind": "unsubscribe", "specs": [{"publisher_id": 100, "content_kind": "video"}]}"#;
            let result: MessageKind = serde_json::from_str(json).unwrap();
            assert_eq!(result, MessageKind::Unsubscribe {
                specs: vec!(SubscriptionSpec {
                    publisher_id: UserId(100),
                    content_kind: ContentKind::VIDEO,
                })
            });
        }
    }
}
