use crate::messages::{RoomId, Subscription, UserId};
use janus_plugin::sdp::Sdp;
use janus_plugin::session::SessionWrapper;
use once_cell::sync::OnceCell;
/// Types for representing Janus session state.
use std::sync::atomic::{AtomicBool, AtomicIsize};
use std::sync::{Arc, Mutex};

/// Once they join a room, all sessions are classified as either subscribers or publishers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinKind {
    Publisher,
    Subscriber,
}

/// State pertaining to all sessions that have joined a room.
#[derive(Debug, Clone)]
pub struct JoinState {
    /// Whether this session is a subscriber or a publisher.
    pub kind: JoinKind,

    /// The room ID that this session is in.
    pub room_id: RoomId,

    /// An opaque ID uniquely identifying this user.
    pub user_id: UserId,
}

impl JoinState {
    pub fn new(kind: JoinKind, room_id: RoomId, user_id: UserId) -> Self {
        Self { kind, room_id, user_id }
    }
}

/// The state associated with a single session.
#[derive(Debug)]
pub struct SessionState {
    /// Whether this session has been destroyed.
    pub destroyed: AtomicBool,

    /// The current FIR sequence number for this session's video.
    pub fir_seq: AtomicIsize,

    /// Information pertaining to this session's user and room, if joined.
    pub join_state: OnceCell<JoinState>,

    // todo: these following fields should be unified with the JoinState, but it's
    // annoying in practice because they are established during JSEP negotiation
    // rather than during the join flow
    /// If this is a subscriber, the subscription this user has established, if any.
    pub subscription: OnceCell<Subscription>,

    /// If this is a publisher, the offer for subscribing to it.
    pub subscriber_offer: Arc<Mutex<Option<Sdp>>>,
}

/// Rust representation of a single Janus session, i.e. a single `RTCPeerConnection`.
pub type Session = SessionWrapper<SessionState>;
