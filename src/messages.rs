/// Types and code related to handling signalling messages.
use std::fmt;
use super::Sdp;
use super::auth::UserToken;
use serde::de::{self, Deserialize, Deserializer, Visitor};

/// A room ID representing a Janus multicast room.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct RoomId(pub String);

/// A user ID representing a single Janus client. Used to correlate multiple Janus connections back to the same
/// conceptual user for managing subscriptions.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct UserId(pub String);

struct IdVisitor;

impl<'de> Visitor<'de> for IdVisitor {
    type Value = String;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("string or numeric identifier")
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<String, E>
    {
        Ok(value.into())
    }

    fn visit_string<E: de::Error>(self, value: String) -> Result<String, E>
    {
        Ok(value)
    }

    fn visit_u64<E: de::Error>(self, value: u64) -> Result<String, E>
    {
        Ok(value.to_string())
    }
}

impl<'de> Deserialize<'de> for RoomId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<RoomId, D::Error>
    {
        deserializer.deserialize_any(IdVisitor).map(|x| RoomId(x))
    }
}

impl<'de> Deserialize<'de> for UserId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<UserId, D::Error>
    {
        deserializer.deserialize_any(IdVisitor).map(|x| UserId(x))
    }
}

/// Useful to represent a JSON message field which may or may not be present.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
#[serde(deny_unknown_fields)]
pub enum OptionalField<T> {
    Some(T),
    None {}
}

/// A signalling message carrying a JSEP SDP offer or answer.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase", tag = "type")]
pub enum JsepKind {
    /// An offer to establish a connection.
    Offer { sdp: Sdp },

    /// An answer responding to an offer.
    Answer { sdp: Sdp },
}

/// The enumeration of all (non-JSEP) signalling messages which can be received from a client.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase", tag = "kind")]
pub enum MessageKind {
    /// Indicates that a client wishes to "join" a room on the server. Prior to this, no audio, video, or data
    /// received from the client will be forwarded to anyone.
    ///
    /// The "subscribe" field specifies which kind of traffic this client will receive. (Useful for saving a round
    /// trip if you wanted to both join and subscribe, as is typical.)
    Join {
        room_id: RoomId,
        user_id: UserId,
        subscribe: Option<Subscription>,
        token: Option<UserToken>,
    },

    /// Indicates that a client wishes to subscribe to traffic described by the given subscription specification.
    Subscribe { what: Subscription },

    /// Indicates that a given user should be blocked from receiving your traffic, and that you should not
    /// receive their traffic (superseding any subscriptions you have.)
    Block { whom: UserId },

    /// Undoes a block targeting the given user.
    Unblock { whom: UserId },

    /// Requests a list of connected users by room.
    ListUsers,
}

/// Information about which traffic a client will get pushed to them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Subscription {
    /// Whether to subscribe to server-wide notifications (e.g. user joins and leaves, room creates and destroys).
    pub notifications: bool,

    /// Whether to subscribe to data in the currently-joined room.
    pub data: bool,

    /// Whether to subscribe to media (audio and video) from a particular user.
    pub media: Option<UserId>,
}

#[cfg(test)]
mod tests {

    use super::*;

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
        fn parse_inner_error() {
            let json = r#"{"kind": "join"}"#;
            let result: serde_json::Result<OptionalField<MessageKind>> = serde_json::from_str(json);
            assert!(result.is_err());
        }

        #[test]
        fn parse_outer_error() {
            let json = r#"{"kind": "fiddle"}"#;
            let result: serde_json::Result<OptionalField<MessageKind>> = serde_json::from_str(json);
            assert!(result.is_err());
        }

        #[test]
        fn parse_list_users() {
            let json = r#"{"kind": "listusers"}"#;
            let result: MessageKind = serde_json::from_str(json).unwrap();
            assert_eq!(result, MessageKind::ListUsers);
        }

        #[test]
        fn parse_join_user_id() {
            let json = r#"{"kind": "join", "user_id": 10, "room_id": "alpha"}"#;
            let result: MessageKind = serde_json::from_str(json).unwrap();
            assert_eq!(result, MessageKind::Join {
                user_id: UserId("10".into()),
                room_id: RoomId("alpha".into()),
                subscribe: None,
                token: None,
            });
        }

        #[test]
        fn parse_join_subscriptions() {
            let json = r#"{"kind": "join", "user_id": 10, "room_id": 5, "subscribe": {"notifications": true, "data": false}}"#;
            let result: MessageKind = serde_json::from_str(json).unwrap();
            assert_eq!(result, MessageKind::Join {
                user_id: UserId("10".into()),
                room_id: RoomId("5".into()),
                subscribe: Some(Subscription {
                    notifications: true,
                    data: false,
                    media: None
                }),
                token: None,
            });
        }

        #[test]
        fn parse_subscribe() {
            let json = r#"{"kind": "subscribe", "what": {"notifications": false, "data": true, "media": "steve"}}"#;
            let result: MessageKind = serde_json::from_str(json).unwrap();
            assert_eq!(result, MessageKind::Subscribe {
                what: Subscription {
                    notifications: false,
                    data: true,
                    media: Some(UserId("steve".into()))
                }
            });
        }
    }
}
