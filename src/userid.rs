use std::error::Error;
use std::sync::atomic::{AtomicUsize, Ordering};

/// A user ID representing a single Janus client. Used to correlate multiple Janus connections back to the same
/// conceptual user for managing subscriptions.
///
/// User IDs are represented as usizes >= 1; 0 indicates an un-set user ID.
#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct UserId(usize);

impl UserId {
    /// Attempts to construct a user ID from a usize. Any usize >= 1 is valid.
    pub fn try_from(val: usize) -> Result<UserId, Box<Error+Send+Sync>> {
        match val {
            0 => Err(From::from("User IDs must be positive integers.")),
            _ => Ok(UserId(val))
        }
    }
}

/// An atomic container representing an optional user ID. A sequence of successive user IDs can be generated via
/// AtomicUserId::first() followed by repeated invocations of AtomicUserId::next().
#[derive(Debug)]
pub struct AtomicUserId {
    v: AtomicUsize
}

impl AtomicUserId {
    pub fn empty() -> Self {
        Self { v: AtomicUsize::new(0) }
    }

    pub fn first() -> Self {
        Self { v: AtomicUsize::new(1) }
    }

    pub fn next(&self, order: Ordering) -> Option<UserId> {
        match self.v.fetch_add(1, order) {
            0 => None,
            n => Some(UserId(n))
        }
    }

    pub fn load(&self, order: Ordering) -> Option<UserId> {
        match self.v.load(order) {
            0 => None,
            n => Some(UserId(n))
        }
    }

    pub fn store(&self, val: UserId, order: Ordering) {
        self.v.store(val.0, order);
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn basic_functionality() {
        let a = AtomicUserId::empty();
        let b = AtomicUserId::first();
        assert_eq!(None, a.next(Ordering::SeqCst));
        assert_eq!(a.load(Ordering::SeqCst), b.load(Ordering::SeqCst));
    }
}
