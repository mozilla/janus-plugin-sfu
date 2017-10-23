/// Types for representing Janus session state.
use atom::AtomSetOnce;
use messages::{RoomId, UserId};
use janus::session::SessionWrapper;

/// The state associated with a single session.
#[derive(Debug, Clone)]
pub struct SessionState {
    /// An opaque ID uniquely identifying this user.
    pub user_id: UserId,

    /// The room ID that this session is in. Only users in the same room can subscribe to each other.
    pub room_id: RoomId,

    /// Whether or not this session should receive notifications.
    pub notify: bool,
}

/// Rust representation of a single Janus session, i.e. a single RTCPeerConnection.
pub type Session = SessionWrapper<AtomSetOnce<Box<SessionState>>>;
