/// Types and code related to managing session subscriptions to incoming data.
use messages::UserId;
use sessions::Session;
use messages::SubscriptionSpec;
use std::collections::HashMap;
use std::error::Error;
use std::sync::{Arc, Weak};
use super::PluginSession;

bitflags! {
    /// A particular kind of traffic transported over a connection.
    #[derive(Serialize, Deserialize)]
    pub struct ContentKind: u8 {
        /// Audio traffic.
        const AUDIO = 0b00000001;
        /// Video traffic.
        const VIDEO = 0b00000010;
        /// All traffic.
        const ALL = 0b11111111;
    }
}

/// Indicates that content of a particular kind should be routed to a particular session,
/// i.e. the session "subscribes" to the content.
#[derive(Debug)]
pub struct SessionContent {
    /// The subscriber to this content. Null if the subscriber has been destroyed since subscribing.
    pub sess: Weak<Session>,

    /// The kind or kinds of content subscribed to.
    pub kind: ContentKind,
}

impl SessionContent {
    pub fn new(sess: &Arc<Session>, kind: ContentKind) -> Self {
        Self { sess: Arc::downgrade(sess), kind }
    }
}

/// A subscription to a particular kind of content from a particular publisher.
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub struct Subscription {
    /// The content publisher in question.
    pub publisher_id: UserId,

    /// The kind or kinds of content that are being subscribed to.
    pub kind: ContentKind,
}

impl Subscription {
    pub fn try_from(spec: &SubscriptionSpec) -> Result<Self, Box<Error>> {
        match ContentKind::from_bits(spec.content_kind) {
            Some(kind) => Ok(Self { publisher_id: spec.publisher_id, kind }),
            None => Err(From::from("Invalid content kind.")),
        }
    }
}

#[derive(Debug)]
pub struct SubscriptionMap {
    /// For a given publisher, which connections are subscribing to which content from them.
    pub publisher_to_subscribers: HashMap<Arc<Session>, Vec<SubscriberRecord>>,

    /// For a given subscriber, which connections are publishing which content to them.
    pub subscriber_to_publishers: HashMap<*mut PluginSession, Vec<Subscription>>,
}

// the pointer is opaque to Janus code, so this handle is threadsafe to the extent that the state is

unsafe impl Sync for SubscriptionMap {}
unsafe impl Send for SubscriptionMap {}

impl Switchboard {
    pub fn new() -> Self {
        Self {
            publisher_to_subscribers: HashMap::new(),
            subscriber_to_subscriptions: HashMap::new(),
        }
    }

    pub fn remove_session(&mut self, session: &Arc<Session>) {
        if let Some(state) = session.get() {
            self.publisher_to_subscribers.remove(&state.user_id);
        }
        self.subscriber_to_subscriptions.remove(&session.as_ptr());
    }

    pub fn subscribe(&mut self, subscriber: &Arc<Session>, subscription: Subscription) {
        let Subscription { publisher_id, kind } = subscription;
        self.subscriber_to_subscriptions.entry(subscriber.handle).or_insert_with(Vec::new).push(subscription);
        self.publisher_to_subscribers.entry(publisher_id).or_insert_with(Vec::new).push(SubscriberRecord::new(subscriber, kind));
    }

    pub fn subscribe_all<I>(&mut self, subscriber: &Arc<Session>, subscriptions: I) where I: Iterator<Item=Subscription> {
        for subscription in subscriptions {
            self.subscribe(subscriber, subscription);
        }
    }

    pub fn unsubscribe(&mut self, subscriber: &Session, subscription: Subscription) {
        let Subscription { publisher_id, kind } = subscription;
        self.subscriber_to_subscriptions.entry(subscriber.handle).or_insert_with(Vec::new).retain(|x| &subscription != x);
        self.publisher_to_subscribers.entry(publisher_id).or_insert_with(Vec::new).retain(|x| {
            let matches = if let Some(s) = x.sess.upgrade() { s.handle == subscriber.handle && x.kind == kind } else { false };
            !matches
        });
    }

    pub fn unsubscribe_all<I>(&mut self, subscriber: &Session, subscriptions: I) where I: Iterator<Item=Subscription> {
        for subscription in subscriptions {
            self.unsubscribe(subscriber, subscription);
        }
    }

    pub fn subscribers_to(&self, publisher: UserId, kind: Option<ContentKind>) -> Vec<&SubscriberRecord> {
        let all_subscribers = self.publisher_to_subscribers.get(&publisher).map(Vec::as_slice).unwrap_or(&[]).iter();
        match kind {
            None => all_subscribers.collect(),
            Some(k) => all_subscribers.filter(|s| { s.kind.contains(k) }).collect()
        }
    }

    pub fn publishers_to(&self, subscriber: &Session, kind: Option<ContentKind>) -> Vec<&Subscription> {
        let all_subscriptions = self.subscriber_to_subscriptions.get(&subscriber.handle).map(Vec::as_slice).unwrap_or(&[]).iter();
        match kind {
            None => all_subscriptions.collect(),
            Some(k) => all_subscriptions.filter(|s| { k.contains(s.kind) }).collect()
        }
    }
}
