//! Wire formats shared by the CLI, the VMM control socket and the guest agent.
//!
//! The control socket starts with one JSON request line. `open` requests then
//! switch the connection to frames, the same framing used on the guest agent
//! port: `[u32 stream][u8 kind][u32 len][payload]`, little endian.

use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io::{self, BufRead, Read, Write},
};

pub mod kind {
    pub const OPEN: u8 = 1;
    pub const DATA: u8 = 2;
    pub const STDERR: u8 = 3;
    pub const EOF: u8 = 4;
    pub const EXIT: u8 = 5;
    pub const RESIZE: u8 = 6;
    pub const HELLO: u8 = 7;
    pub const SETUP: u8 = 8;
    pub const CLOSE: u8 = 9;
    pub const SETUP_DONE: u8 = 10;
    pub const TELEMETRY: u8 = 11;
}

const HEADER_LEN: usize = 9;
const MAX_PAYLOAD: usize = 16 * 1024 * 1024;

pub struct Frame {
    pub stream: u32,
    pub kind: u8,
    pub payload: Vec<u8>,
}

impl Frame {
    pub fn new(stream: u32, kind: u8, payload: Vec<u8>) -> Self {
        Self { stream, kind, payload }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(HEADER_LEN + self.payload.len());
        bytes.extend_from_slice(&self.stream.to_le_bytes());
        bytes.push(self.kind);
        bytes.extend_from_slice(&(self.payload.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&self.payload);
        bytes
    }
}

pub struct Deframer {
    buf: Vec<u8>,
}

impl Deframer {
    pub fn new() -> Self {
        Self { buf: Vec::new() }
    }

    pub fn push(&mut self, bytes: &[u8], mut on_frame: impl FnMut(Frame)) {
        self.buf.extend_from_slice(bytes);

        while self.buf.len() >= HEADER_LEN {
            let len = u32::from_le_bytes(self.buf[5..9].try_into().unwrap()) as usize;
            assert!(len <= MAX_PAYLOAD, "agent frame of {len} bytes exceeds the protocol limit");
            if self.buf.len() < HEADER_LEN + len {
                return;
            }

            let frame = Frame {
                stream: u32::from_le_bytes(self.buf[0..4].try_into().unwrap()),
                kind: self.buf[4],
                payload: self.buf[HEADER_LEN..HEADER_LEN + len].to_vec(),
            };
            self.buf.drain(..HEADER_LEN + len);
            on_frame(frame);
        }
    }
}

pub fn read_frame(reader: &mut impl Read) -> io::Result<Option<Frame>> {
    let mut header = [0u8; HEADER_LEN];
    match reader.read_exact(&mut header) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error),
    }

    let len = u32::from_le_bytes(header[5..9].try_into().unwrap()) as usize;
    if len > MAX_PAYLOAD {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "frame exceeds the protocol limit"));
    }
    let mut payload = vec![0; len];
    reader.read_exact(&mut payload)?;

    Ok(Some(Frame {
        stream: u32::from_le_bytes(header[0..4].try_into().unwrap()),
        kind: header[4],
        payload,
    }))
}

pub fn write_frame(writer: &mut impl Write, frame: &Frame) -> io::Result<()> {
    writer.write_all(&frame.encode())?;
    writer.flush()
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
pub enum Request {
    Wait,
    Screenshot,
    /// PNG frames whenever the screen changes, at most `fps` per second, each
    /// as a little-endian u32 length followed by the PNG.
    Stream { fps: u32 },
    Input { events: Vec<InputEvent> },
    /// Input events as JSON lines until the client disconnects, for interactive use.
    InputStream,
    /// Resizes the guest display to the aspect ratio of `width` x `height`,
    /// scaled down to at most the VM's configured pixel count.
    Resize { width: u32, height: u32 },
    Open { session: Session },
    Stop,
}

#[derive(Serialize, Deserialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub enum InputEvent {
    Move { x: u32, y: u32 },
    Button { button: u16, down: bool },
    Key { code: u16, down: bool },
    Scroll { value: i32, horizontal: bool },
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Session {
    Pty {
        argv: Vec<String>,
        cols: u16,
        rows: u16,
        term: String,
    },
    Exec {
        argv: Vec<String>,
    },
    Tcp {
        port: u16,
    },
}

#[derive(Serialize, Deserialize)]
pub struct Setup {
    pub time_ms: u64,
    pub authorized_keys: String,
    pub mounts: Vec<GuestMount>,
    pub environment: BTreeMap<String, String>,
    pub seeds: Option<GuestSeeds>,
}

/// Where the staged seeds are mounted and where each item goes.
#[derive(Serialize, Deserialize, Clone)]
pub struct GuestSeeds {
    pub mount: String,
    pub items: Vec<SeedItem>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct SeedItem {
    /// Name inside the seed mount.
    pub source: String,
    pub target: String,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct GuestMount {
    pub tag: String,
    pub path: String,
    pub readonly: bool,
}

/// The agent's answer to [`Setup`]; `errors` is empty on success.
#[derive(Serialize, Deserialize)]
pub struct SetupDone {
    pub errors: Vec<String>,
}

#[derive(Serialize, Deserialize, Default)]
pub struct Reply {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub boot_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub png_len: Option<usize>,
}

pub fn write_json(writer: &mut impl Write, value: &impl Serialize) -> io::Result<()> {
    let mut line = serde_json::to_vec(value).map_err(io::Error::other)?;
    line.push(b'\n');
    writer.write_all(&line)?;
    writer.flush()
}

pub fn read_json<T: for<'de> Deserialize<'de>>(reader: &mut impl BufRead) -> io::Result<T> {
    let mut line = String::new();
    if reader.read_line(&mut line)? == 0 {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "connection closed"));
    }
    serde_json::from_str(&line).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}
