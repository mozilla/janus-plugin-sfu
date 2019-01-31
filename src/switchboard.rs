/// Tools for managing the set of subscriptions between connections.
use messages::{RoomId, UserId};
use sessions::Session;
use std::collections::{HashMap, HashSet};
use std::collections::hash_map::Entry;
use std::sync::Arc;
use std::hash::Hash;
use std::fmt::Debug;
use std::borrow::Borrow;
use multimap::MultiMap;

#[derive(Debug)]
pub struct BidirectionalMultimap<K: Eq + Hash, V: Eq + Hash> {
    forward_mapping: MultiMap<K, V>,
    inverse_mapping: MultiMap<V, K>,
}

impl<K, V> BidirectionalMultimap<K, V> where K: Eq + Hash + Clone + Debug, V: Eq + Hash + Clone + Debug {
    pub fn new() -> Self {
        Self {
            forward_mapping: MultiMap::new(),
            inverse_mapping: MultiMap::new(),
        }
    }

    pub fn associate(&mut self, k: K, v: V) {
        let kk = k.clone();
        let vv = v.clone();
        self.forward_mapping.insert(k, vv);
        self.inverse_mapping.insert(v, kk);
    }

    pub fn disassociate<T, U>(&mut self, k: &T, v: &U)
        where K: Borrow<T>,
              T: Hash + Eq,
              V: Borrow<U>,
              U: Hash + Eq
    {
        if let Some(vals) = self.forward_mapping.get_vec_mut(k) {
            vals.retain(|x| x.borrow() != v);
        }
        if let Some(keys) = self.inverse_mapping.get_vec_mut(v) {
            keys.retain(|x| x.borrow() != k);
        }
    }

    pub fn remove_key<T>(&mut self, k: &T)
        where K: Borrow<T>,
              T: Hash + Eq + Debug
    {
        if let Some(vs) = self.forward_mapping.remove(k) {
            for v in vs {
                if let Some(ks) = self.inverse_mapping.get_vec_mut(&v) {
                    ks.retain(|x| x.borrow() != k);
                } else {
                    janus_err!("Map in inconsistent state: entry ({:?}, {:?}) has no corresponding entry.", k, v);
                }
            }
        }
    }

    pub fn remove_value<U>(&mut self, v: &U)
        where V: Borrow<U>,
              U: Hash + Eq + Debug
    {
        if let Some(ks) = self.inverse_mapping.remove(v) {
            for k in ks {
                if let Some(vs) = self.forward_mapping.get_vec_mut(&k) {
                    vs.retain(|x| x.borrow() != v);
                } else {
                    janus_err!("Map in inconsistent state: entry ({:?}, {:?}) has no corresponding entry.", k, v);
                }
            }
        }
    }

    pub fn get_values<T>(&self, k: &T) -> &[V]
        where K: Borrow<T>,
              T: Hash + Eq
    {
        self.forward_mapping.get_vec(k).map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn get_keys<U>(&self, v: &U) -> &[K]
        where V: Borrow<U>,
              U: Hash + Eq
    {
        self.inverse_mapping.get_vec(v).map(Vec::as_slice).unwrap_or(&[])
    }
}

/// A data structure for storing the state of all active connections and providing fast access to which
/// connections should be sending traffic to which other connections.
#[derive(Debug)]
pub struct Switchboard {
    /// All active connections.
    sessions: Vec<Box<Arc<Session>>>,
    /// Connections which have joined a room, per room.
    occupants: HashMap<RoomId, Vec<Arc<Session>>>,
    /// Which connections are subscribing to traffic from which other connections.
    publisher_to_subscribers: BidirectionalMultimap<Arc<Session>, Arc<Session>>,
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

    pub fn establish_block(&mut self, from: UserId, target: UserId) {
        self.blockers_to_miscreants.associate(from, target);
    }

    pub fn lift_block(&mut self, from: &UserId, target: &UserId) {
        self.blockers_to_miscreants.disassociate(from, target);
    }

    pub fn join_room(&mut self, session: Arc<Session>, room: RoomId) {
        self.occupants.entry(room).or_insert_with(Vec::new).push(session);
    }

    pub fn leave_room(&mut self, session: &Session, room: RoomId) {
        if let Entry::Occupied(mut cohabitators) = self.occupants.entry(room) {
            cohabitators.get_mut().retain(|x| x.as_ref() != session);
            if cohabitators.get().is_empty() {
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

    pub fn subscribers_to(&self, publisher: &Session) -> &[Arc<Session>] {
        self.publisher_to_subscribers.get_values(publisher)
    }

    pub fn publishers_to(&self, subscriber: &Session) -> &[Arc<Session>] {
        self.publisher_to_subscribers.get_keys(subscriber)
    }

    pub fn sessions(&self) -> &Vec<Box<Arc<Session>>> {
        &self.sessions
    }

    pub fn occupants_of(&self, room: &RoomId) -> &[Arc<Session>] {
        self.occupants.get(room).map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn media_recipients_for(&self, sender: &Session) -> impl Iterator<Item=&Arc<Session>> {
        let (forward_blocks, reverse_blocks) = match sender.join_state.get() {
            None => (&[] as &[_], &[] as &[_]),
            Some(joined) => (
                self.blockers_to_miscreants.get_keys(&joined.user_id),
                self.blockers_to_miscreants.get_values(&joined.user_id)
            )
        };
        self.subscribers_to(sender).iter().filter(move |subscriber| {
            match subscriber.join_state.get() {
                None => true,
                Some(other) => {
                    let blocks = forward_blocks.contains(&other.user_id);
                    let is_blocked = reverse_blocks.contains(&other.user_id);
                    !blocks && !is_blocked
                }
            }
        })
    }

    pub fn media_senders_to(&self, recipient: &Session) -> impl Iterator<Item=&Arc<Session>> {
        let (forward_blocks, reverse_blocks) = match recipient.join_state.get() {
            None => (&[] as &[_], &[] as &[_]),
            Some(joined) => (
                self.blockers_to_miscreants.get_values(&joined.user_id),
                self.blockers_to_miscreants.get_keys(&joined.user_id)
            )
        };
        self.publishers_to(recipient).iter().filter(move |publisher| {
            match publisher.join_state.get() {
                None => true,
                Some(other) => {
                    let blocks = forward_blocks.contains(&other.user_id);
                    let is_blocked = reverse_blocks.contains(&other.user_id);
                    !blocks && !is_blocked
                }
            }
        })
    }

    pub fn data_recipients_for<'s>(&'s self, session: &'s Session) -> impl Iterator<Item=&'s Arc<Session>> {
        let (forward_blocks, reverse_blocks, cohabitators) = match session.join_state.get() {
            None => (&[] as &[_], &[] as &[_], &[] as &[_]),
            Some(joined) => (
                self.blockers_to_miscreants.get_keys(&joined.user_id),
                self.blockers_to_miscreants.get_values(&joined.user_id),
                self.occupants_of(&joined.room_id)
            )
        };
        cohabitators.iter().filter(move |cohabitator| {
            cohabitator.handle != session.handle && match cohabitator.join_state.get() {
                None => true,
                Some(other) => {
                    let blocks = forward_blocks.contains(&other.user_id);
                    let is_blocked = reverse_blocks.contains(&other.user_id);
                    !blocks && !is_blocked
                }
            }
        })
    }

    pub fn get_users(&self, room: &RoomId) -> HashSet<&UserId> {
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

    pub fn get_publisher(&self, user_id: &UserId) -> Option<&Arc<Session>> {
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

    pub fn get_sessions(&self, user_id: &UserId) -> Vec<&Box<Arc<Session>>> {
        self.sessions.iter()
            .filter(|s| {
                let join_state = s.join_state.get();
                match join_state {
                    Some(state) if &state.user_id == user_id => true,
                    _ => false
                }
            }).collect::<_>()
    }

}
