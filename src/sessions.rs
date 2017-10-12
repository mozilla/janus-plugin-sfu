/// Types for representing Janus session state.

use janus::session::SessionWrapper;
use userid::AtomicUserId;

/// The state associated with a single session.
#[derive(Debug)]
pub struct SessionState {

    /// A unique user ID for this session. Used to correlate multiple sessions that represent
    /// the same client, so that other code can refer to a client's packets consistently without
    /// regard to which session those packets are being transported on.
    ///
    /// By convention, this starts out empty during every session and is immutable once set.
    pub user_id: AtomicUserId
}

impl Default for SessionState {
    fn default() -> Self {
        Self { user_id: AtomicUserId::empty() }
    }
}

/// Rust representation of a single Janus session, i.e. a single RTCPeerConnection.
pub type Session = SessionWrapper<SessionState>;
