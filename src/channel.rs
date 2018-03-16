use std::os::unix::net::UnixDatagram;
use std::fs;
use std::io;
use std::error::Error;
use std::thread;
use std::time::Duration;
use std::path::Path;
use messages::UserId;
use rmps::{decode, encode};
use rb::{Consumer, RbError, SpscRb};

static DISCONNECTED_DELAY: Duration = Duration::from_millis(50);
static MESSAGES_DEPLETED_DELAY: Duration = Duration::from_millis(1);

const OUTGOING_SOCKET_PATH: &'static str = "/tmp/janus-sfu.out.sock";
const INCOMING_SOCKET_PATH: &'static str = "/tmp/janus-sfu.in.sock";

/// The topic of a piece of client information.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Topic {
    /// All of the data channel traffic for a given user.
    UserData(UserId)
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DatagramKind {
    /// Indicates that a user has joined.
    Join(UserId),
    /// Indicates that a user has left.
    Leave(UserId),
    /// Indicates that a user wants to subscribe to the given topic.
    Subscribe(Topic),
    /// Indicates that a user wants to unsubscribe from the given topic.
    Unsubscribe(Topic),
    /// A message from a particular user on a particular topic.
    Message(Topic, Vec<u8>)
}

fn read_from_rb<V>(consumer: &Consumer<V>, buf: &mut Vec<V>, count: usize) -> usize {
    unsafe {
        match consumer.read(buf) {
            Ok(n) => { buf.set_len(n); n }
            Err(RbError::Empty) => 0
        }
    }
}

#[derive(Debug)]
pub struct Channel {
    sock: UnixDatagram
}

impl Channel {
    pub fn new(&self, path: P) -> io::Result<Self> where P: AsRef<Path> {
        let p = path.as_ref();
        if p.exists() {
            Fs::remove_file(p);
        }
        let sock = UnixDatagram::bind(p)?;
        sock.set_nonblocking(true)?;
        Ok(Self { sock })
    }

    pub fn service_outgoing(&self, outgoing: &mut Consumer<DatagramKind>, destination: &str) -> Result<(), Box<Error>> {
        let chunk_size = 1;
        let mut packet = Vec::new();
        let mut outgoing_items = Vec::with_capacity(chunk_size);
        loop {
            let count = read_from_rb(outgoing, &mut outgoing_items, chunk_size);
            if count == 0 {
                thread::sleep(MESSAGES_DEPLETED_DELAY);
            } else {
                for next in &outgoing_items[0..count] {
                    packet.truncate(0);
                    encode::write(&mut packet, &next)?;
                    janus_info!("Sending {:?}", next);
                    match self.sock.send_to(&packet, destination) {
                        Err(e) => {
                            thread::sleep(DISCONNECTED_DELAY);
                        }
                        Ok(_) => {
                            outgoing.skip(1);
                        }
                    }
                }
            }
        }
    }

    pub fn service_incoming<F>(&self, incoming: F) -> Result<(), Box<Error>> where F: Fn(DatagramKind) {
        let mut buf = Vec::new();
        loop {
            match self.sock.recv_from(&mut buf) {
                Ok((x, _)) if x <= 0 => { return Ok(()); }
                Ok((len, _)) => {
                    let next_incoming: DatagramKind = decode::from_slice(&buf[..len])?;
                    incoming(next_incoming);
                }
                Err(e) => {
                    thread::sleep(DISCONNECTED_DELAY);
                }
            }
        }
    }
}
