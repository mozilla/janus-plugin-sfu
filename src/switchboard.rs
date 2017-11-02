/// Tools for managing the set of subscriptions between connections.
use super::serde::ser::{Serialize, Serializer, SerializeSeq};
use sessions::Session;
use messages::ContentKind;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Weak};

#[derive(Debug)]
struct SessionContent {
    /// The session.
    pub sess: Weak<Session>,

    /// The content.
    pub kind: ContentKind,
}

impl SessionContent {
    pub fn new(sess: &Arc<Session>, kind: ContentKind) -> Self {
        Self { sess: Arc::downgrade(sess), kind }
    }
}

/// A data structure for expressing which connections should be sending data to which other connections.  Basically a
/// bidirectional map from subscriber (a connection) to publication (a (connection, content_kind) pair), and vice versa,
/// optimized for fast lookups in both directions.
///
/// Note that internally, strong references are kept as keys for each subscriber and publisher in the switchboard, but
/// only weak references are kept as values. This turns the cost of removing a session from O(N) up front, where N is
/// the number of map entries, into O(1) up front and O(M) amortized over time as we encounter the dead entries, where M
/// is the number of actual subscriptions including that session (which should be much smaller.)
#[derive(Debug)]
pub struct Switchboard {
    /// For a given connection, which connections are subscribing to which content from them.
    publisher_to_subscribers: HashMap<Arc<Session>, Vec<SessionContent>>,
    /// For a given connection, which connections are publishing which content to them.
    subscriber_to_publishers: HashMap<Arc<Session>, Vec<SessionContent>>,
}

impl Serialize for Switchboard {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error> where S: Serializer {
        #[derive(Serialize)]
        struct Connection { publisher: String, subscriber: String, kind: ContentKind };
        let mut connections = serializer.serialize_seq(None)?;
        for (publisher, subscriptions) in self.publisher_to_subscribers.iter() {
            for subscription in subscriptions {
                if let Some(subscriber) = subscription.sess.upgrade() {
                    let publisher_handle = format!("{:p}", publisher.as_ptr());
                    let subscriber_handle = format!("{:p}", subscriber.as_ptr());
                    let conn = Connection {
                        publisher: publisher_handle,
                        subscriber: subscriber_handle,
                        kind: subscription.kind
                    };
                    connections.serialize_element(&conn)?;
                }
            }
        }
        connections.end()
    }
}

impl Switchboard {
    pub fn new() -> Self {
        Self {
            publisher_to_subscribers: HashMap::new(),
            subscriber_to_publishers: HashMap::new(),
        }
    }

    pub fn remove_session(&mut self, session: &Arc<Session>) {
        self.publisher_to_subscribers.remove(session);
        self.subscriber_to_publishers.remove(session);
    }

    pub fn subscribe(&mut self, subscriber: &Arc<Session>, publishers: &HashSet<Arc<Session>>, kind: ContentKind) {
        let publisher_records = publishers.iter().map(|p| SessionContent::new(p, kind));
        self.subscriber_to_publishers.entry(Arc::clone(subscriber)).or_insert_with(Vec::new).extend(publisher_records);
        for publisher in publishers {
            let entry = self.publisher_to_subscribers.entry(Arc::clone(publisher));
            let subscriber_record = SessionContent::new(subscriber, kind);
            entry.or_insert_with(Vec::new).push(subscriber_record);
        }
    }

    pub fn unsubscribe(&mut self, subscriber: &Arc<Session>, publishers: &HashSet<Arc<Session>>, kind: ContentKind) {
        self.subscriber_to_publishers.entry(Arc::clone(subscriber)).or_insert_with(Vec::new).retain(|record| {
            record.kind != kind || match record.sess.upgrade() {
                Some(s) => !publishers.contains(&s),
                None => false // if the publisher is dead, now's a fine time to trim it
            }
        });
        for publisher in publishers {
            self.publisher_to_subscribers.entry(Arc::clone(publisher)).or_insert_with(Vec::new).retain(|record| {
                record.kind != kind || match record.sess.upgrade() {
                    Some(s) => &s != subscriber,
                    None => false  // if the subscriber is dead, now's a fine time to trim it
                }
            });
        }
    }

    pub fn subscribers_to(&self, publisher: &Session, kind: Option<ContentKind>) -> Vec<Arc<Session>> {
        let all_subscribers = self.publisher_to_subscribers.get(publisher).map(Vec::as_slice).unwrap_or(&[]).iter();
        match kind {
            None => all_subscribers.filter_map(|record| record.sess.upgrade()).collect(),
            Some(k) => all_subscribers.filter(|s| { s.kind.contains(k) }).filter_map(|record| record.sess.upgrade()).collect()
        }
    }

    pub fn publishers_to(&self, subscriber: &Session, kind: Option<ContentKind>) -> Vec<Arc<Session>> {
        let all_subscriptions = self.subscriber_to_publishers.get(subscriber).map(Vec::as_slice).unwrap_or(&[]).iter();
        match kind {
            None => all_subscriptions.filter_map(|record| record.sess.upgrade()).collect(),
            Some(k) => all_subscriptions.filter(|s| { k.contains(s.kind) }).filter_map(|record| record.sess.upgrade()).collect()
        }
    }
}
