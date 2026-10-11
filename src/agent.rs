//! Host side of the guest agent link on the `agent` virtio-console port.
//! Routes frames between guest streams and control socket clients.

use crate::{
    devices::RuntimeEvent,
    protocol::{Deframer, Frame, Session, Setup, kind},
};
use std::{
    collections::HashMap,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicU32, Ordering},
        mpsc::{Receiver, Sender, channel},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

pub struct Agent {
    to_guest: Sender<RuntimeEvent>,
    streams: Mutex<HashMap<u32, Sender<Frame>>>,
    next_stream: AtomicU32,
    started: Instant,
    boot_time: Mutex<Option<Duration>>,
    booted: Condvar,
    authorized_keys: String,
}

/// Receives raw bytes the guest wrote to the agent port.
pub struct Sink {
    tx: Sender<Vec<u8>>,
}

impl Sink {
    pub fn push(&self, bytes: Vec<u8>) {
        self.tx.send(bytes).expect("agent reader thread exited");
    }
}

pub fn start(to_guest: Sender<RuntimeEvent>, authorized_keys: String) -> (Arc<Agent>, Sink) {
    let agent = Arc::new(Agent {
        to_guest,
        streams: Mutex::new(HashMap::new()),
        next_stream: AtomicU32::new(1),
        started: Instant::now(),
        boot_time: Mutex::new(None),
        booted: Condvar::new(),
        authorized_keys,
    });

    let (tx, rx) = channel::<Vec<u8>>();
    let reader = agent.clone();
    thread::Builder::new()
        .name("varmint-agent".into())
        .spawn(move || {
            let mut deframer = Deframer::new();
            while let Ok(bytes) = rx.recv() {
                deframer.push(&bytes, |frame| reader.receive(frame));
            }
        })
        .unwrap_or_else(|error| panic!("failed to spawn agent thread: {error}"));

    (agent, Sink { tx })
}

impl Agent {
    pub fn send(&self, frame: &Frame) {
        self.to_guest
            .send(RuntimeEvent::AgentRx(frame.encode()))
            .expect("runtime event pump exited");
    }

    pub fn open(&self, session: &Session) -> (u32, Receiver<Frame>) {
        let stream = self.next_stream.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = channel();
        self.streams.lock().unwrap().insert(stream, tx);
        self.send(&Frame::new(stream, kind::OPEN, serde_json::to_vec(session).unwrap()));
        (stream, rx)
    }

    pub fn close(&self, stream: u32) {
        if self.streams.lock().unwrap().remove(&stream).is_some() {
            self.send(&Frame::new(stream, kind::CLOSE, Vec::new()));
        }
    }

    /// Blocks until the guest agent has said hello and returns the boot time.
    pub fn wait_booted(&self) -> Duration {
        let guard = self
            .booted
            .wait_while(self.boot_time.lock().unwrap(), |boot_time| boot_time.is_none())
            .unwrap();
        guard.unwrap()
    }

    fn receive(&self, frame: Frame) {
        if frame.kind == kind::HELLO {
            self.hello();
            return;
        }

        let mut streams = self.streams.lock().unwrap();
        let finished = frame.kind == kind::EXIT;
        let stream = frame.stream;
        if let Some(client) = streams.get(&stream) {
            // A disconnected client is cleaned up by its own connection thread.
            let _ = client.send(frame);
        }
        if finished {
            streams.remove(&stream);
        }
    }

    fn hello(&self) {
        // A restarted agent has lost every stream it served before.
        self.streams.lock().unwrap().clear();

        let time_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("host clock is before the unix epoch")
            .as_millis() as u64;
        let setup = Setup {
            time_ms,
            authorized_keys: self.authorized_keys.clone(),
        };
        self.send(&Frame::new(0, kind::SETUP, serde_json::to_vec(&setup).unwrap()));

        let mut boot_time = self.boot_time.lock().unwrap();
        if boot_time.is_none() {
            *boot_time = Some(self.started.elapsed());
            self.booted.notify_all();
        }
    }
}
