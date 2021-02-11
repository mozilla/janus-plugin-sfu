use crate::messages::{RoomId, UserId};
use crate::sessions::Session;
use janus_plugin::janus_err;
use std::borrow::Borrow;
/// Tools for managing the set of subscriptions between connections.
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::fmt::Debug;
use std::hash::Hash;
use std::sync::Arc;

#[derive(Debug)]
pub struct BidirectionalMultimap<K: Eq + Hash, V: Eq + Hash> {
    forward_mapping: HashMap<K, Vec<V>>,
    inverse_mapping: HashMap<V, Vec<K>>,
}

impl<K, V> BidirectionalMultimap<K, V>
where
    K: Eq + Hash + Clone + Debug,
    V: Eq + Hash + Clone + Debug,
{
    pub fn new() -> Self {
        Self {
            forward_mapping: HashMap::new(),
            inverse_mapping: HashMap::new(),
        }
    }

    pub fn associate(&mut self, k: K, v: V) {
        let kk = k.clone();
        let vv = v.clone();
        self.forward_mapping.entry(k).or_insert_with(Vec::new).push(vv);
        self.inverse_mapping.entry(v).or_insert_with(Vec::new).push(kk);
    }

    pub fn disassociate<T, U>(&mut self, k: &T, v: &U)
    where
        K: Borrow<T>,
        T: Hash + Eq,
        V: Borrow<U>,
        U: Hash + Eq,
    {
        if let Some(vals) = self.forward_mapping.get_mut(k) {
            vals.retain(|x| x.borrow() != v);
        }
        if let Some(keys) = self.inverse_mapping.get_mut(v) {
            keys.retain(|x| x.borrow() != k);
        }
    }

    pub fn remove_key<T>(&mut self, k: &T)
    where
        K: Borrow<T>,
        T: Hash + Eq + Debug,
    {
        if let Some(vs) = self.forward_mapping.remove(k) {
            for v in vs {
                if let Some(ks) = self.inverse_mapping.get_mut(&v) {
                    ks.retain(|x| x.borrow() != k);
                } else {
                    janus_err!("Map in inconsistent state: entry ({:?}, {:?}) has no corresponding entry.", k, v);
                }
            }
        }
    }

    pub fn remove_value<U>(&mut self, v: &U)
    where
        V: Borrow<U>,
        U: Hash + Eq + Debug,
    {
        if let Some(ks) = self.inverse_mapping.remove(v) {
            for k in ks {
                if let Some(vs) = self.forward_mapping.get_mut(&k) {
                    vs.retain(|x| x.borrow() != v);
                } else {
                    janus_err!("Map in inconsistent state: entry ({:?}, {:?}) has no corresponding entry.", k, v);
                }
            }
        }
    }

    pub fn get_values<T>(&self, k: &T) -> &[V]
    where
        K: Borrow<T>,
        T: Hash + Eq,
    {
        self.forward_mapping.get(k).map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn get_keys<U>(&self, v: &U) -> &[K]
    where
        V: Borrow<U>,
        U: Hash + Eq,
    {
        self.inverse_mapping.get(v).map(Vec::as_slice).unwrap_or(&[])
    }
}

/// A data structure for storing the state of all active connections and providing fast access to which
/// connections should be sending traffic to which other connections.
#[derive(Debug)]
pub struct Switchboard {
    /// All active connections, whether or not they have joined a room.
    sessions: Vec<Box<Arc<Session>>>,
    /// All joined publisher connections, by which room they have joined.
    publishers_by_room: HashMap<RoomId, Vec<Arc<Session>>>,
    /// All joined publisher connections, by which user they have joined as.
    publishers_by_user: HashMap<UserId, Arc<Session>>,
    /// All joined subscriber connections, by which user they have joined as.
    subscribers_by_user: HashMap<UserId, Vec<Arc<Session>>>,
    /// Which connections are subscribing to traffic from which other connections.
    publisher_to_subscribers: BidirectionalMultimap<Arc<Session>, Arc<Session>>,
    /// Which users have explicitly blocked traffic to and from other users.
    blockers_to_miscreants: BidirectionalMultimap<UserId, UserId>,
}

impl Switchboard {
    pub fn new() -> Self {
        Self {
            sessions: Vec::new(),
            publishers_by_room: HashMap::new(),
            publishers_by_user: HashMap::new(),
            subscribers_by_user: HashMap::new(),
            publisher_to_subscribers: BidirectionalMultimap::new(),
            blockers_to_miscreants: BidirectionalMultimap::new(),
        }
    }

    pub fn connect(&mut self, session: Box<Arc<Session>>) {
        self.sessions.push(session);
    }

    pub fn disconnect(&mut self, session: &Session) {
        self.sessions.retain(|s| s.handle != session.handle);
    }

    pub fn is_connected(&self, user: &UserId) -> bool {
        self.sessions.iter().any(|s| match s.join_state.get() {
            None => false,
            Some(other_state) => user == &other_state.user_id,
        })
    }

    pub fn establish_block(&mut self, from: UserId, target: UserId) {
        self.blockers_to_miscreants.associate(from, target);
    }

    pub fn lift_block(&mut self, from: &UserId, target: &UserId) {
        self.blockers_to_miscreants.disassociate(from, target);
    }

    pub fn join_publisher(&mut self, session: Arc<Session>, user: UserId, room: RoomId) {
        self.publishers_by_user.entry(user).or_insert(session.clone());
        self.publishers_by_room.entry(room).or_insert_with(Vec::new).push(session);
    }

    pub fn join_subscriber(&mut self, session: Arc<Session>, user: UserId, _room: RoomId) {
        self.subscribers_by_user.entry(user).or_insert_with(Vec::new).push(session);
    }

    pub fn leave_publisher(&mut self, session: &Session) {
        self.publisher_to_subscribers.remove_key(session);
        if let Some(joined) = session.join_state.get() {
            self.publishers_by_user.remove(&joined.user_id);
            if let Entry::Occupied(mut others) = self.publishers_by_room.entry(joined.room_id.clone()) {
                others.get_mut().retain(|x| x.as_ref() != session);
                if others.get().is_empty() {
                    others.remove_entry();
                }
            }
        }
    }

    pub fn leave_subscriber(&mut self, session: &Session) {
        self.publisher_to_subscribers.remove_value(session);
        if let Some(joined) = session.join_state.get() {
            if let Entry::Occupied(mut others) = self.subscribers_by_user.entry(joined.user_id.clone()) {
                others.get_mut().retain(|x| x.as_ref() != session);
                if others.get().is_empty() {
                    others.remove_entry();
                }
            }
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

    pub fn publishers_occupying(&self, room: &RoomId) -> &[Arc<Session>] {
        self.publishers_by_room.get(room).map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn media_recipients_for(&self, sender: &Session) -> impl Iterator<Item = &Arc<Session>> {
        let (forward_blocks, reverse_blocks) = match sender.join_state.get() {
            None => (&[] as &[_], &[] as &[_]),
            Some(joined) => (
                self.blockers_to_miscreants.get_keys(&joined.user_id),
                self.blockers_to_miscreants.get_values(&joined.user_id),
            ),
        };
        self.subscribers_to(sender).iter().filter(move |subscriber| match subscriber.join_state.get() {
            None => true,
            Some(other) => {
                let blocks = forward_blocks.contains(&other.user_id);
                let is_blocked = reverse_blocks.contains(&other.user_id);
                !blocks && !is_blocked
            }
        })
    }

    pub fn media_senders_to(&self, recipient: &Session) -> impl Iterator<Item = &Arc<Session>> {
        let (forward_blocks, reverse_blocks) = match recipient.join_state.get() {
            None => (&[] as &[_], &[] as &[_]),
            Some(joined) => (
                self.blockers_to_miscreants.get_values(&joined.user_id),
                self.blockers_to_miscreants.get_keys(&joined.user_id),
            ),
        };
        self.publishers_to(recipient).iter().filter(move |publisher| match publisher.join_state.get() {
            None => true,
            Some(other) => {
                let blocks = forward_blocks.contains(&other.user_id);
                let is_blocked = reverse_blocks.contains(&other.user_id);
                !blocks && !is_blocked
            }
        })
    }

    pub fn data_recipients_for<'s>(&'s self, session: &'s Session) -> impl Iterator<Item = &'s Arc<Session>> {
        let (forward_blocks, reverse_blocks, cohabitators) = match session.join_state.get() {
            None => (&[] as &[_], &[] as &[_], &[] as &[_]),
            Some(joined) => (
                self.blockers_to_miscreants.get_keys(&joined.user_id),
                self.blockers_to_miscreants.get_values(&joined.user_id),
                self.publishers_occupying(&joined.room_id),
            ),
        };
        cohabitators.iter().filter(move |cohabitator| {
            cohabitator.handle != session.handle
                && match cohabitator.join_state.get() {
                    None => true,
                    Some(other) => {
                        let blocks = forward_blocks.contains(&other.user_id);
                        let is_blocked = reverse_blocks.contains(&other.user_id);
                        !blocks && !is_blocked
                    }
                }
        })
    }

    pub fn get_room_users(&self, room: &RoomId) -> impl Iterator<Item = &UserId> {
        self.publishers_occupying(room).iter().filter_map(|s| {
            s.join_state.get().map(|j| &j.user_id)
        })
    }

    pub fn get_all_users(&self) -> impl Iterator<Item = &UserId> {
        self.publishers_by_user.keys()
    }

    pub fn get_publisher(&self, user: &UserId) -> Option<&Arc<Session>> {
        self.publishers_by_user.get(user)
    }

    pub fn get_subscribers(&self, user: &UserId) -> Option<&Vec<Arc<Session>>> {
        self.subscribers_by_user.get(user)
    }
}
