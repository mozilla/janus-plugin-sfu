/// Types for representing Janus session state.
use atom::AtomSetOnce;
use std::sync::atomic::AtomicIsize;
use messages::{RoomId, UserId};
use janus::session::SessionWrapper;
use std::sync::Arc;

/// The state associated with a single session.
#[derive(Debug)]
pub struct SessionState {
    /// The room ID that this session is in. Only users in the same room can subscribe to each other.
    pub room_id: RoomId,

    /// An opaque ID uniquely identifying this user.
    pub user_id: UserId,

    /// Whether or not this session should receive notifications.
    pub notify: bool,

    /// The current FIR sequence number for this session's video.
    pub fir_seq: AtomicIsize,
}

impl SessionState {
    pub fn new(room_id: RoomId, user_id: UserId, notify: bool) -> Self {
        Self { room_id, user_id, notify, fir_seq: AtomicIsize::new(0) }
    }
}

/// Rust representation of a single Janus session, i.e. a single RTCPeerConnection.
pub type Session = SessionWrapper<AtomSetOnce<Arc<SessionState>>>;
