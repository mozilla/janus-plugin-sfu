/// Types for representing Janus session state.
use atom::AtomSetOnce;
use std::collections::VecDeque;
use std::sync::atomic::AtomicIsize;
use std::sync::{Arc, Mutex};
use messages::{RoomId, UserId, Subscription};
use janus::sdp::Sdp;
use janus::session::SessionWrapper;

/// All of the offers that a publisher has made, ordered chronologically.
pub type PublicationLog = VecDeque<Sdp>;

/// State pertaining to this session's join of a particular room as a particular user ID.
#[derive(Debug, Clone)]
pub struct JoinState {
    /// The room ID that this session is in.
    pub room_id: RoomId,

    /// An opaque ID uniquely identifying this user.
    pub user_id: UserId,
}

impl JoinState {
    pub fn new(room_id: RoomId, user_id: UserId) -> Self {
        Self { room_id, user_id }
    }
}

/// The state associated with a single session.
#[derive(Debug)]
pub struct SessionState {
    /// Whether this session has been destroyed.
    pub destroyed: Mutex<bool>,

    /// Information pertaining to this session's user and room, if joined.
    pub join_state: AtomSetOnce<Box<JoinState>>,

    /// The subscription this user has established, if any.
    pub subscription: AtomSetOnce<Box<Subscription>>,

    /// If this is a publisher, the offers it's made to subscribe to it.
    pub publications: Arc<Mutex<PublicationLog>>,

    /// The current FIR sequence number for this session's video.
    pub fir_seq: AtomicIsize,
}

/// Rust representation of a single Janus session, i.e. a single `RTCPeerConnection`.
pub type Session = SessionWrapper<SessionState>;
