/// Tools for managing the set of subscriptions between connections.
use messages::{RoomId, UserId};
use sessions::Session;
use std::collections::{HashMap, HashSet};
use std::collections::hash_map::Entry;
use std::sync::{Arc, Weak};
use std::hash::Hash;

#[derive(Debug)]
pub struct BidirectionalMultimap<K: Eq + Hash, V: Eq + Hash> {
    forward_mapping: HashMap<Arc<K>, Vec<Weak<V>>>,
    inverse_mapping: HashMap<Arc<V>, Vec<Weak<K>>>,
}

impl<K, V> BidirectionalMultimap<K, V> where K: Eq + Hash, V: Eq + Hash {
    pub fn new() -> Self {
        Self {
            forward_mapping: HashMap::new(),
            inverse_mapping: HashMap::new(),
        }
    }

    pub fn associate(&mut self, k: Arc<K>, v: Arc<V>) {
        let weak_k = Arc::downgrade(&k);
        let weak_v = Arc::downgrade(&v);
        self.forward_mapping.entry(k).or_insert_with(Vec::new).push(weak_v);
        self.inverse_mapping.entry(v).or_insert_with(Vec::new).push(weak_k);
    }

    pub fn disassociate(&mut self, k: &K, v: &V) {
        if let Some(vals) = self.forward_mapping.get_mut(k) {
            vals.retain(|x| x.upgrade().map(|x| x.as_ref() != v).unwrap_or(false));
        }
        if let Some(keys) = self.inverse_mapping.get_mut(v) {
            keys.retain(|x| x.upgrade().map(|x| x.as_ref() != k).unwrap_or(false));
        }
    }

    pub fn remove_key(&mut self, k: &K) {
        self.forward_mapping.remove(k);
    }

    pub fn remove_value(&mut self, v: &V) {
        self.inverse_mapping.remove(v);
    }

    pub fn get_values(&self, k: &K) -> Vec<Arc<V>> {
        self.forward_mapping.get(k).map(Vec::as_slice).unwrap_or(&[]).iter().filter_map(|s| s.upgrade()).collect()
    }

    pub fn get_keys(&self, v: &V) -> Vec<Arc<K>> {
        self.inverse_mapping.get(v).map(Vec::as_slice).unwrap_or(&[]).iter().filter_map(|s| s.upgrade()).collect()
    }
}

/// A data structure for storing the state of all active connections and providing fast access to which
/// connections should be sending traffic to which other connections.#[derive(Debug)]
pub struct Switchboard {
    /// All active connections.
    sessions: Vec<Box<Arc<Session>>>,
    /// Connections which have joined a room, per room.
    occupants: HashMap<RoomId, HashSet<Arc<Session>>>,
    /// Which connections are subscribing to traffic from which other connections.
    publisher_to_subscribers: BidirectionalMultimap<Session, Session>,
    /// Which users have explicitly blocked traffic to and from other users.
    blockers_to_miscreants: BidirectionalMultimap<UserId, UserId>,
}

impl Switchboard {
    pub fn new() -> Self {
        Self {
            sessions: Vec::new(),
            occupants: HashMap::new(),
            publisher_to_subscribers: BidirectionalMultimap::new(),
            blockers_to_miscreants: BidirectionalMultimap::new(),
        }
    }

    pub fn connect(&mut self, session: Box<Arc<Session>>) {
        self.sessions.push(session);
    }

    pub fn is_connected(&self, user: &UserId) -> bool {
        self.sessions.iter().any(|s| {
            match s.join_state.get() {
                None => false,
                Some(other_state) => user == &other_state.user_id
            }
        })
    }

    pub fn establish_block(&mut self, from: Arc<UserId>, target: Arc<UserId>) {
        self.blockers_to_miscreants.associate(from, target);
    }

    pub fn lift_block(&mut self, from: &UserId, target: &UserId) {
        self.blockers_to_miscreants.disassociate(from, target);
    }

    pub fn join_room(&mut self, session: Arc<Session>, room: RoomId) {
        self.occupants.entry(room).or_insert_with(HashSet::new).insert(session);
    }

    pub fn leave_room(&mut self, session: &Session, room: RoomId) {
        if let Entry::Occupied(mut cohabitators) = self.occupants.entry(room) {
            cohabitators.get_mut().remove(session);
            if cohabitators.get().len() == 0 {
                cohabitators.remove_entry();
            }
        }
    }

    pub fn remove_session(&mut self, session: &Session) {
        self.publisher_to_subscribers.remove_key(session);
        self.publisher_to_subscribers.remove_value(session);
        self.sessions.retain(|s| s.handle != session.handle);
        if let Some(joined) = session.join_state.get() {
            self.leave_room(session, joined.room_id.clone());
        }
    }

    pub fn subscribe_to_user(&mut self, subscriber: Arc<Session>, publisher: Arc<Session>) {
        self.publisher_to_subscribers.associate(publisher, subscriber);
    }

    pub fn subscribers_to(&self, publisher: &Session) -> Vec<Arc<Session>> {
        self.publisher_to_subscribers.get_values(publisher)
    }

    pub fn publishers_to(&self, subscriber: &Session) -> Vec<Arc<Session>> {
        self.publisher_to_subscribers.get_keys(subscriber)
    }

    pub fn occupants_of(&self, room: &RoomId) -> HashSet<Arc<Session>> {
        self.occupants.get(room).map(|x| x.clone()).unwrap_or_else(HashSet::new)
    }

    pub fn media_recipients_for(&self, sender: &Session) -> Vec<Arc<Session>> {
        let mut subscribers = self.subscribers_to(sender);
        if let Some(joined) = sender.join_state.get() {
            let forward_blocks = self.blockers_to_miscreants.get_keys(&joined.user_id);
            let reverse_blocks = self.blockers_to_miscreants.get_values(&joined.user_id);
            let blocks_exist = !forward_blocks.is_empty() || !reverse_blocks.is_empty();
            if blocks_exist {
                subscribers.retain(|recipient| {
                    match recipient.join_state.get() {
                        None => true,
                        Some(other) => {
                            let blocks = forward_blocks.iter().any(|x| x.as_ref() == &other.user_id);
                            let is_blocked = reverse_blocks.iter().any(|x| x.as_ref() == &other.user_id);
                            return !blocks && !is_blocked;
                        }
                    }
                });
            }
        }
        subscribers
    }

    pub fn media_senders_to(&self, recipient: &Session) -> Vec<Arc<Session>> {
        let mut publishers = self.publishers_to(recipient);
        if let Some(joined) = recipient.join_state.get() {
            let forward_blocks = self.blockers_to_miscreants.get_values(&joined.user_id);
            let reverse_blocks = self.blockers_to_miscreants.get_keys(&joined.user_id);
            let blocks_exist = !forward_blocks.is_empty() || !reverse_blocks.is_empty();
            if blocks_exist {
                publishers.retain(|sender| {
                    match sender.join_state.get() {
                        None => true,
                        Some(other) => {
                            let blocks = forward_blocks.iter().any(|x| x.as_ref() == &other.user_id);
                            let is_blocked = reverse_blocks.iter().any(|x| x.as_ref() == &other.user_id);
                            return !blocks && !is_blocked;
                        }
                    }
                });
            }
        }
        publishers
    }

    pub fn data_recipients_for(&self, session: &Session) -> HashSet<Arc<Session>> {
        if let Some(joined) = session.join_state.get() {
            let mut cohabitators = self.occupants_of(&joined.room_id);
            let forward_blocks = self.blockers_to_miscreants.get_keys(&joined.user_id);
            let reverse_blocks = self.blockers_to_miscreants.get_values(&joined.user_id);
            let blocks_exist = !forward_blocks.is_empty() || !reverse_blocks.is_empty();
            cohabitators.retain(|cohabitator| cohabitator.handle != session.handle);
            if blocks_exist {
                cohabitators.retain(|cohabitator| {
                    match cohabitator.join_state.get() {
                        None => true,
                        Some(other) => {
                            let blocks = forward_blocks.iter().any(|x| x.as_ref() == &other.user_id);
                            let is_blocked = reverse_blocks.iter().any(|x| x.as_ref() == &other.user_id);
                            return !blocks && !is_blocked;
                        }
                    }
                });
            }
            cohabitators
        } else {
            HashSet::new()
        }
    }

    pub fn occupant_count(&self, room: &RoomId) -> usize {
        self.occupants.get(room).map(HashSet::len).unwrap_or(0)
    }

    pub fn get_users<'a, 'b>(&'a self, room: &'b RoomId) -> HashSet<&'a UserId> {
        let mut result = HashSet::new();
        if let Some(sessions) = self.occupants.get(room) {
            for session in sessions {
                if let Some(joined) = session.join_state.get() {
                    result.insert(&joined.user_id);
                }
            }
        }
        result
    }

    pub fn get_publisher<'a, 'b>(&'a self, user_id: &'b UserId) -> Option<&'a Arc<Session>> {
        self.sessions.iter()
            .find(|s| {
                let subscriber_offer = s.subscriber_offer.lock().unwrap();
                let join_state = s.join_state.get();
                match (subscriber_offer.as_ref(), join_state) {
                    (Some(_), Some(state)) if &state.user_id == user_id => true,
                    _ => false
                }
            })
            .map(Box::as_ref)
    }
}
