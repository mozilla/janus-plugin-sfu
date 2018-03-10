use std::os::unix::net::UnixDatagram;
use std::io;
use std::error::Error;
use std::thread;
use std::time::Duration;
use std::path::Path;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use messages::UserId;
use rmps::{decode, encode};

static SOCKET_ERROR_DELAY: Duration = Duration::from_millis(50);
static CHANNEL_DEPLETED_DELAY: Duration = Duration::from_millis(1);

/// The topic of a piece of client information.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Topic {
    /// All of the data channel traffic for a given user.
    UserData(UserId)
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
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

#[derive(Debug)]
pub struct Channel {
    sock: UnixDatagram,
    outgoing: Receiver<DatagramKind>,
    incoming: Sender<DatagramKind>,
    outgoing_buf: Vec<u8>,
    incoming_buf: Vec<u8>,
}

fn is_transient_error(kind: io::ErrorKind) -> bool {
    match kind {
        io::ErrorKind::ConnectionRefused | io::ErrorKind::ConnectionReset | io::ErrorKind::ConnectionAborted |
        io::ErrorKind::NotConnected | io::ErrorKind::BrokenPipe => true,
        _ => false
    }
}

impl Channel {
    pub fn new<P>(path: P, outgoing: Receiver<DatagramKind>, incoming: Sender<DatagramKind>) -> io::Result<Self> where P: AsRef<Path> {
        let sock = UnixDatagram::bind(path)?;
        sock.set_nonblocking(true)?;
        Ok(Self {
            sock: sock,
            outgoing: outgoing,
            incoming: incoming,
            outgoing_buf: Vec::new(),
            incoming_buf: Vec::new(),
        })
    }

    fn send_outgoing(&mut self) -> Result<(), Box<Error>> {
        loop {
            match self.outgoing.try_recv() {
                Err(TryRecvError::Empty) => { return Ok(()); }
                Err(TryRecvError::Disconnected) => { return Err(From::from("Channel was disconnected.")); }
                Ok(next_outgoing) => {
                    encode::write(&mut self.outgoing_buf, &next_outgoing)?;
                    while let Err(e) = self.sock.send(&self.outgoing_buf) {
                        if is_transient_error(e.kind()) {
                            janus_info!("Error sending message; retrying... ({})", e);
                        } else {
                            janus_info!("Outgoing connection broken; retrying... ({})", e);
                        }
                        thread::sleep(SOCKET_ERROR_DELAY);
                    }
                }
            }
        }
    }

    fn receive_incoming(&mut self) -> Result<(), Box<Error>> {
        loop {
            match self.sock.recv(self.incoming_buf.as_mut()) {
                Ok(x) if x <= 0 => { return Ok(()); }
                Ok(len) => {
                    let next_incoming: DatagramKind = decode::from_slice(&self.incoming_buf[..len])?;
                    self.incoming.send(next_incoming)?;
                }
                Err(e) => {
                    if is_transient_error(e.kind()) {
                        janus_info!("Error receiving message; retrying... ({})", e);
                    } else {
                        janus_info!("Incoming connection broken; retrying... ({})", e);
                    }
                    thread::sleep(SOCKET_ERROR_DELAY);
                }
            }
        }
    }

    pub fn service(&mut self) -> Result<(), Box<Error>> {
        loop {
            self.send_outgoing()?;
            self.receive_incoming()?;
            thread::sleep(CHANNEL_DEPLETED_DELAY);
        }
    }
}
