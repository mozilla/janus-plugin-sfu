use crate::messages::{RoomId, Subscription, UserId};
use janus_plugin::sdp::Sdp;
use janus_plugin::session::SessionWrapper;
use once_cell::sync::OnceCell;
/// Types for representing Janus session state.
use std::sync::atomic::{AtomicBool, AtomicIsize};
use std::sync::{Arc, Mutex};

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
    pub destroyed: AtomicBool,

    /// Information pertaining to this session's user and room, if joined.
    pub join_state: OnceCell<JoinState>,

    /// The subscription this user has established, if any.
    pub subscription: OnceCell<Subscription>,

    /// If this is a publisher, the offer for subscribing to it.
    pub subscriber_offer: Arc<Mutex<Option<Sdp>>>,

    /// The current FIR sequence number for this session's video.
    pub fir_seq: AtomicIsize,
}

/// Rust representation of a single Janus session, i.e. a single `RTCPeerConnection`.
pub type Session = SessionWrapper<SessionState>;
