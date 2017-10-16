/// Types for representing Janus session state.
use entityids::{AtomicRoomId, AtomicUserId};
use janus::session::SessionWrapper;

/// The state associated with a single session.
#[derive(Debug)]
pub struct SessionState {
    /// The user ID associated with this session. Used to correlate multiple sessions that represent
    /// the same client, so that other code can refer to a client's packets consistently without
    /// regard to which session those packets are being transported on.
    ///
    /// By convention, this starts out empty during every session and is immutable once set.
    pub user_id: AtomicUserId,

    /// The room ID that this session is in. Only users in the same room can subscribe to each other.
    ///
    /// By convention, this starts out empty during every session and is immutable once set.
    pub room_id: AtomicRoomId,
}

impl Default for SessionState {
    fn default() -> Self {
        Self {
            room_id: AtomicRoomId::empty(),
            user_id: AtomicUserId::empty(),
        }
    }
}

/// Rust representation of a single Janus session, i.e. a single RTCPeerConnection.
pub type Session = SessionWrapper<SessionState>;
