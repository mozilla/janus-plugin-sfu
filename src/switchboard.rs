/// Tools for managing the set of subscriptions between connections.
use super::serde::ser::{Serialize, Serializer, SerializeSeq};
use messages::UserId;
use sessions::Session;
use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Weak};
use std::hash::Hash;

#[derive(Debug)]
pub struct BidirectionalMultimap<T> where T: Eq + Hash {
    forward_mapping: HashMap<Arc<T>, Vec<Weak<T>>>,
    inverse_mapping: HashMap<Arc<T>, Vec<Weak<T>>>,
}

//#[derive(Serialize)]
//struct Association<'a, T> where T: Serialize + 'a { from: &'a T, to: &'a T }

impl<T> Serialize for BidirectionalMultimap<T> where T: Eq + Hash {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error> where S: Serializer {
        let mut assocs = serializer.serialize_seq(None)?;
        // for (from, tos) in &self.forward_mapping {
        //     for to in tos {
        //         if let Some(to) = to.upgrade() {
        //             assocs.serialize_element(&Association { from: from.as_ref(), to: to.as_ref() })?;
        //         }
        //     }
        // }
        assocs.end()
    }
}

impl<T> BidirectionalMultimap<T> where T: Eq + Hash {
    pub fn new() -> Self {
        Self {
            forward_mapping: HashMap::new(),
            inverse_mapping: HashMap::new(),
        }
    }

    pub fn associate(&mut self, k: Arc<T>, v: Arc<T>) {
        let weak_k = Arc::downgrade(&k);
        let weak_v = Arc::downgrade(&v);
        self.forward_mapping.entry(k).or_insert_with(Vec::new).push(weak_v);
        self.inverse_mapping.entry(v).or_insert_with(Vec::new).push(weak_k);
    }

    pub fn disassociate(&mut self, k: &T, v: &T) {
        if let Some(vals) = self.forward_mapping.get_mut(k) {
            vals.retain(|x| x.upgrade().map(|x| x.as_ref() != v).unwrap_or(false));
        }
        if let Some(keys) = self.inverse_mapping.get_mut(v) {
            keys.retain(|x| x.upgrade().map(|x| x.as_ref() != k).unwrap_or(false));
        }
    }

    pub fn forget(&mut self, x: &T) {
        self.remove_key(x);
        self.remove_value(x);
    }

    pub fn remove_key(&mut self, k: &T) {
        self.forward_mapping.remove(k);
    }

    pub fn remove_value(&mut self, v: &T) {
        self.inverse_mapping.remove(v);
    }

    pub fn get_values(&self, k: &T) -> Vec<Arc<T>> {
        self.forward_mapping.get(k).map(Vec::as_slice).unwrap_or(&[]).iter().filter_map(|s| s.upgrade()).collect()
    }

    pub fn get_keys(&self, v: &T) -> Vec<Arc<T>> {
        self.inverse_mapping.get(v).map(Vec::as_slice).unwrap_or(&[]).iter().filter_map(|s| s.upgrade()).collect()
    }
}

/// A data structure for expressing which connections should be sending data to which other connections.
///
/// Note that internally, strong references are kept as keys for each subscriber and publisher in the switchboard, but
/// only weak references are kept as values. This turns the cost of removing a session from O(N) up front, where N is
/// the number of map entries, into O(1), at the cost of suffering a little bit occasionally as we encounter the dead
/// entries.
#[derive(Debug, Serialize)]
pub struct Switchboard {
    /// Which connections are subscribing to traffic from which other connections.
    publisher_to_subscribers: BidirectionalMultimap<Session>,
    /// Which users have explicitly blocked traffic to and from other users.
    blockers_to_miscreants: BidirectionalMultimap<UserId>,
}

impl Switchboard {
    pub fn new() -> Self {
        Self {
            publisher_to_subscribers: BidirectionalMultimap::new(),
            blockers_to_miscreants: BidirectionalMultimap::new(),
        }
    }

    pub fn establish_block(&mut self, from: Arc<UserId>, target: Arc<UserId>) {
        self.blockers_to_miscreants.associate(from, target);
    }

    pub fn lift_block(&mut self, from: &UserId, target: &UserId) {
        self.blockers_to_miscreants.disassociate(from, target);
    }

    pub fn remove_session(&mut self, session: &Session) {
        self.publisher_to_subscribers.forget(session);
    }

    pub fn subscribe_to_user(&mut self, subscriber: Arc<Session>, publisher: Arc<Session>) {
        self.publisher_to_subscribers.associate(subscriber, publisher);
    }

    pub fn subscribers_to(&self, publisher: &Session) -> Vec<Arc<Session>> {
        self.publisher_to_subscribers.get_values(publisher)
    }

    pub fn publishers_to(&self, subscriber: &Session) -> Vec<Arc<Session>> {
        self.publisher_to_subscribers.get_keys(subscriber)
    }

    pub fn recipients_for(&self, sender: &Session) -> Vec<Arc<Session>> {
        let mut subscribers = self.subscribers_to(sender);
        if let Some(joined) = sender.join_state.get() {
            let forward_blocks = self.blockers_to_miscreants.get_keys(&joined.user_id);
            let reverse_blocks = self.blockers_to_miscreants.get_values(&joined.user_id);
            let blocks_exist = !forward_blocks.is_empty() || !reverse_blocks.is_empty();
            if blocks_exist {
                subscribers.retain(|recipient| {
                    match recipient.join_state.get() {
                        None => true,
                        Some(other) => (
                            !forward_blocks.iter().any(|x| x.as_ref() == &other.user_id) && !reverse_blocks.iter().any(|x| x.as_ref() == &other.user_id)
                        )
                    }
                });
            }
        }
        subscribers
    }

    pub fn senders_to(&self, recipient: &Session) -> Vec<Arc<Session>> {
        let mut publishers = self.publishers_to(recipient);
        if let Some(joined) = recipient.join_state.get() {
            let forward_blocks = self.blockers_to_miscreants.get_values(&joined.user_id);
            let reverse_blocks = self.blockers_to_miscreants.get_keys(&joined.user_id);
            let blocks_exist = !forward_blocks.is_empty() || !reverse_blocks.is_empty();
            if blocks_exist {
                publishers.retain(|sender| {
                    match sender.join_state.get() {
                        None => true,
                        Some(other) => (
                            !forward_blocks.iter().any(|x| x.as_ref() == &other.user_id) && !reverse_blocks.iter().any(|x| x.as_ref() == &other.user_id)
                        )
                    }
                });
            }
        }
        publishers
    }
}
