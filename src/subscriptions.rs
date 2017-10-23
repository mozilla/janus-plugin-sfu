/// Types and code related to managing session subscriptions to incoming data.
use messages::UserId;
use sessions::Session;
use messages::SubscriptionSpec;
use std::collections::HashMap;
use std::error::Error;
use std::sync::{Arc, Weak};

bitflags! {
    /// A particular kind of traffic transported over a connection.
    #[derive(Serialize, Deserialize)]
    pub struct ContentKind: u8 {
        /// Audio traffic.
        const AUDIO = 0b00000001;
        /// Video traffic.
        const VIDEO = 0b00000010;
        /// Video traffic.
        const ALL = 0b11111111;
    }
}

/// Indicates that traffic of a particular kind should be routed to a particular session,
/// i.e. the session "subscribes" to the traffic.
#[derive(Debug)]
pub struct Subscription {
    /// The subscriber to this traffic. Null if the subscriber has been destroyed since subscribing.
    pub sess: Weak<Session>,

    /// The kind or kinds of traffic subscribed to.
    pub kind: ContentKind,
}

impl Subscription {
    pub fn new(sess: &Arc<Session>, kind: ContentKind) -> Self {
        Self { sess: Arc::downgrade(sess), kind }
    }
}

/// A data structure mapping publishers to subscribers.
pub type SubscriptionMap = HashMap<UserId, Vec<Subscription>>;

pub fn subscribe(subscriptions: &mut SubscriptionMap, sess: &Arc<Session>, kind: ContentKind, publisher: UserId) {
    subscriptions.entry(publisher).or_insert_with(Vec::new).push(Subscription::new(sess, kind));
}

pub fn subscribe_all(subscriptions: &mut SubscriptionMap, sess: &Arc<Session>, specs: &Vec<SubscriptionSpec>) -> Result<(), Box<Error>> {
    for &SubscriptionSpec { publisher_id, content_kind } in specs {
        match ContentKind::from_bits(content_kind) {
            Some(kind) => {
                subscribe(subscriptions, sess, kind, publisher_id);
            }
            None => return Err(From::from("Invalid content kind.")),
        }
    }
    Ok(())
}

pub fn unsubscribe(subscriptions: &mut SubscriptionMap, sess: &Arc<Session>, kind: ContentKind, publisher: UserId) {
    subscriptions.entry(publisher).or_insert_with(Vec::new).retain(|ref sub| {
        let matches = if let Some(s) = sub.sess.upgrade() { s.handle == sess.handle && sub.kind == kind } else { false };
        !matches
    });
}

pub fn unsubscribe_all(subscriptions: &mut SubscriptionMap, sess: &Arc<Session>, specs: &Vec<SubscriptionSpec>) -> Result<(), Box<Error>> {
    for &SubscriptionSpec { publisher_id, content_kind } in specs {
        match ContentKind::from_bits(content_kind) {
            Some(kind) => {
                unsubscribe(subscriptions, sess, kind, publisher_id);
            }
            None => return Err(From::from("Invalid content kind.")),
        }
    }
    Ok(())
}

pub fn unpublish(subscriptions: &mut SubscriptionMap, publisher: UserId) {
    subscriptions.remove(&publisher);
}

pub fn subscribers_to(subscriptions: &SubscriptionMap, publisher: UserId, kind: Option<ContentKind>) -> Vec<&Subscription> {
    let all_subscriptions = subscriptions.get(&publisher).map(Vec::as_slice).unwrap_or(&[]).iter();
    match kind {
        None => all_subscriptions.collect(),
        Some(k) => all_subscriptions.filter(|s| { s.kind.contains(k) }).collect()
    }
}
