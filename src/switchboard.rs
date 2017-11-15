/// Tools for managing the set of subscriptions between connections.
use super::serde::ser::{Serialize, Serializer, SerializeSeq};
use sessions::Session;
use std::collections::HashMap;
use std::sync::{Arc, Weak};

/// A data structure for expressing which connections should be sending data to which other connections.  Basically a
/// bidirectional map from subscriber to publisher and vice versa, optimized for fast lookups in both directions.
///
/// Note that internally, strong references are kept as keys for each subscriber and publisher in the switchboard, but
/// only weak references are kept as values. This turns the cost of removing a session from O(N) up front, where N is
/// the number of map entries, into O(1) up front and O(M) amortized over time as we encounter the dead entries, where M
/// is the number of actual subscriptions including that session (which should be much smaller.)
#[derive(Debug)]
pub struct Switchboard {
    /// For a given connection, which connections are subscribing to traffic from them.
    publisher_to_subscribers: HashMap<Arc<Session>, Vec<Weak<Session>>>,
    /// For a given connection, which connections are publishing traffic to them.
    subscriber_to_publishers: HashMap<Arc<Session>, Vec<Weak<Session>>>,
}

impl Serialize for Switchboard {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error> where S: Serializer {
        #[derive(Serialize)]
        struct Connection { publisher: String, subscriber: String };
        let mut connections = serializer.serialize_seq(None)?;
        for (publisher, subscriptions) in &self.publisher_to_subscribers {
            for subscription in subscriptions {
                if let Some(subscriber) = subscription.upgrade() {
                    let publisher_handle = format!("{:p}", publisher.as_ptr());
                    let subscriber_handle = format!("{:p}", subscriber.as_ptr());
                    let conn = Connection {
                        publisher: publisher_handle,
                        subscriber: subscriber_handle,
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

    pub fn subscribe_to_user(&mut self, subscriber: &Arc<Session>, publisher: &Arc<Session>) {
        self.subscriber_to_publishers.entry(Arc::clone(subscriber)).or_insert_with(Vec::new).push(Arc::downgrade(publisher));
        self.publisher_to_subscribers.entry(Arc::clone(publisher)).or_insert_with(Vec::new).push(Arc::downgrade(subscriber));
    }

    pub fn subscribers_to_user(&self, publisher: &Session) -> Vec<Arc<Session>> {
        let all_subscriptions = self.publisher_to_subscribers.get(publisher).map(Vec::as_slice).unwrap_or(&[]).iter();
        all_subscriptions.filter_map(|s| s.upgrade()).collect()
    }

    pub fn publishers_to_user(&self, subscriber: &Session) -> Vec<Arc<Session>> {
        let all_subscriptions = self.subscriber_to_publishers.get(subscriber).map(Vec::as_slice).unwrap_or(&[]).iter();
        all_subscriptions.filter_map(|s| s.upgrade()).collect()
    }
}
