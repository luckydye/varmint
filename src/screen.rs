//! `varmint screen`: an interactive view of a VM's screen in the terminal.
//!
//! Frames are drawn with the Kitty graphics protocol. Input uses SGR mouse
//! reporting in pixel mode and the Kitty keyboard protocol, which reports
//! every key with press and release. Ghostty, Kitty and WezTerm implement
//! all three.

use crate::{
    keymap,
    protocol::{self, InputEvent},
    virtio::input::keys::{BTN_LEFT, BTN_MIDDLE, BTN_RIGHT},
};
use crossterm::terminal;
use std::{
    collections::BTreeSet,
    fs::File,
    io::{self, Read, Write},
    os::fd::FromRawFd,
    sync::{
        Arc, Mutex,
        atomic::{AtomicI32, Ordering},
        mpsc::channel,
    },
    thread,
};

/// Image and placement ids; reusing them replaces the previous frame in place.
const IMAGE_ID: u32 = 1;
const PLACEMENT_ID: u32 = 1;
/// The protocol limits each escape sequence to 4096 bytes of payload.
const CHUNK: usize = 4096;
/// Kitty keyboard flags: disambiguate keys, report releases, report all keys.
const KEYBOARD_FLAGS: u32 = 1 | 2 | 8;

/// Where the last frame is on screen, for mapping mouse positions.
#[derive(Clone, Copy)]
struct Geometry {
    image_width: u32,
    image_height: u32,
    /// Size of the drawn frame in terminal pixels.
    width: f64,
    height: f64,
}

enum Event {
    Frame(Vec<u8>),
    Resized,
    Closed,
}

/// Shows frames from `frames` until the user quits with Ctrl+] or the VM
/// stops; keyboard and mouse input goes to `input` as JSON lines.
pub fn run(mut frames: impl Read + Send + 'static, mut input: impl Write + Send + 'static) -> io::Result<()> {
    enter()?;
    let geometry = Arc::new(Mutex::new(None::<Geometry>));
    let (events, receiver) = channel();

    let frame_events = events.clone();
    thread::spawn(move || {
        loop {
            match read_frame(&mut frames) {
                Ok(frame) => frame_events.send(Event::Frame(frame)).unwrap(),
                Err(_) => break frame_events.send(Event::Closed).unwrap(),
            }
        }
    });

    let resize_events = events.clone();
    on_window_change(move || {
        let _ = resize_events.send(Event::Resized);
    });

    let input_geometry = geometry.clone();
    thread::spawn(move || {
        forward_input(&mut input, &input_geometry);
        leave();
        std::process::exit(0);
    });
    drop(events);

    let mut last = None;
    while let Ok(event) = receiver.recv() {
        let frame = match event {
            Event::Frame(frame) => &*last.insert(frame),
            Event::Resized => {
                io::stdout().write_all(b"\x1b[2J")?;
                match &last {
                    Some(frame) => frame,
                    None => continue,
                }
            }
            Event::Closed => break,
        };
        match draw(frame) {
            Ok(drawn) => *geometry.lock().unwrap() = Some(drawn),
            Err(error) => {
                leave();
                return Err(error);
            }
        }
    }
    leave();
    Err(io::Error::other("the VM stopped"))
}

fn enter() -> io::Result<()> {
    terminal::enable_raw_mode()?;
    let mut stdout = io::stdout();
    // Alternate screen, hidden cursor, all mouse events in pixels, every key
    // as an escape sequence with releases.
    write!(stdout, "\x1b[?1049h\x1b[?25l\x1b[?1003h\x1b[?1006h\x1b[?1016h\x1b[>{KEYBOARD_FLAGS}u")?;
    stdout.flush()
}

/// Gives the terminal back as it was.
pub fn leave() {
    let mut stdout = io::stdout();
    let _ = write!(
        stdout,
        "\x1b[<u\x1b[?1016l\x1b[?1006l\x1b[?1003l\x1b_Ga=d,d=I,i={IMAGE_ID},q=2\x1b\\\x1b[?25h\x1b[?1049l"
    );
    let _ = stdout.flush();
    let _ = terminal::disable_raw_mode();
}

/// Write end of the self-pipe the SIGWINCH handler signals through.
static WINDOW_CHANGE_PIPE: AtomicI32 = AtomicI32::new(-1);

extern "C" fn on_sigwinch(_: libc::c_int) {
    let byte = 0u8;
    // Async-signal-safe; a full pipe already has a change pending.
    unsafe { libc::write(WINDOW_CHANGE_PIPE.load(Ordering::Relaxed), (&raw const byte).cast(), 1) };
}

/// Calls `changed` on a thread whenever the terminal window changes size.
pub fn on_window_change(changed: impl Fn() + Send + 'static) {
    let mut fds = [0; 2];
    assert!(unsafe { libc::pipe(fds.as_mut_ptr()) } == 0, "pipe failed: {}", io::Error::last_os_error());
    let [read_fd, write_fd] = fds;
    assert!(unsafe { libc::fcntl(write_fd, libc::F_SETFL, libc::O_NONBLOCK) } == 0);
    assert!(
        WINDOW_CHANGE_PIPE.swap(write_fd, Ordering::Relaxed) == -1,
        "on_window_change called twice"
    );
    let handler = on_sigwinch as extern "C" fn(libc::c_int);
    assert!(
        unsafe { libc::signal(libc::SIGWINCH, handler as libc::sighandler_t) } != libc::SIG_ERR,
        "failed to install the SIGWINCH handler"
    );

    thread::spawn(move || {
        let mut pipe = unsafe { File::from_raw_fd(read_fd) };
        let mut signals = [0u8; 64];
        while matches!(pipe.read(&mut signals), Ok(n) if n > 0) {
            changed();
        }
    });
}

/// Width and height from a PNG's IHDR chunk.
fn png_size(png: &[u8]) -> io::Result<(u32, u32)> {
    if png.len() < 24 || &png[1..4] != b"PNG" {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "frame is not a PNG"));
    }
    let width = u32::from_be_bytes(png[16..20].try_into().unwrap());
    let height = u32::from_be_bytes(png[20..24].try_into().unwrap());
    Ok((width, height))
}

/// Draws one PNG frame at the top left, scaled to fit the terminal.
fn draw(png: &[u8]) -> io::Result<Geometry> {
    let (image_width, image_height) = png_size(png)?;
    let window = terminal::window_size()?;
    if window.width == 0 || window.height == 0 {
        return Err(io::Error::other(
            "the terminal does not report its size in pixels; use Ghostty, Kitty or WezTerm",
        ));
    }
    let cell_width = f64::from(window.width) / f64::from(window.columns);
    let cell_height = f64::from(window.height) / f64::from(window.rows);
    let scale = (f64::from(window.width) / f64::from(image_width))
        .min(f64::from(window.height) / f64::from(image_height));
    let columns = ((f64::from(image_width) * scale / cell_width).floor() as u32).max(1);
    let rows = ((f64::from(image_height) * scale / cell_height).floor() as u32).max(1);

    let encoded = base64(png);
    let chunks: Vec<&[u8]> = encoded.as_bytes().chunks(CHUNK).collect();
    let mut out = Vec::with_capacity(encoded.len() + chunks.len() * 32);
    out.extend_from_slice(b"\x1b[H");
    for (index, chunk) in chunks.iter().enumerate() {
        let more = u8::from(index + 1 < chunks.len());
        if index == 0 {
            write!(
                out,
                "\x1b_Ga=T,f=100,i={IMAGE_ID},p={PLACEMENT_ID},c={columns},r={rows},C=1,q=2,m={more};"
            )?;
        } else {
            write!(out, "\x1b_Gm={more};")?;
        }
        out.extend_from_slice(chunk);
        out.extend_from_slice(b"\x1b\\");
    }
    let mut stdout = io::stdout().lock();
    stdout.write_all(&out)?;
    stdout.flush()?;

    // The terminal scales the image to exactly these cells.
    Ok(Geometry {
        image_width,
        image_height,
        width: f64::from(columns) * cell_width,
        height: f64::from(rows) * cell_height,
    })
}

/// Reads one length-prefixed PNG frame of a screen stream.
fn read_frame(reader: &mut impl Read) -> io::Result<Vec<u8>> {
    let mut length = [0u8; 4];
    reader.read_exact(&mut length)?;
    let mut png = vec![0; u32::from_le_bytes(length) as usize];
    reader.read_exact(&mut png)?;
    Ok(png)
}

/// A control sequence from the terminal: `CSI params final`.
struct Sequence {
    params: String,
    final_byte: u8,
}

/// Splits raw terminal input into control sequences; other bytes are dropped,
/// since every key arrives as a sequence.
fn parse(buffer: &mut Vec<u8>) -> Vec<Sequence> {
    let mut sequences = Vec::new();
    loop {
        let Some(start) = buffer.iter().position(|&byte| byte == 0x1b) else {
            buffer.clear();
            return sequences;
        };
        buffer.drain(..start);
        if buffer.len() < 2 {
            return sequences;
        }
        if buffer[1] != b'[' {
            buffer.drain(..1);
            continue;
        }
        let Some(end) = buffer[2..].iter().position(|byte| (0x40..=0x7e).contains(byte)) else {
            return sequences;
        };
        let end = end + 2;
        sequences.push(Sequence {
            params: String::from_utf8_lossy(&buffer[2..end]).into_owned(),
            final_byte: buffer[end],
        });
        buffer.drain(..=end);
    }
}

/// Reads keyboard and mouse input until Ctrl+], then releases held keys.
fn forward_input(input: &mut impl Write, geometry: &Mutex<Option<Geometry>>) {
    let mut held = BTreeSet::new();
    let mut buffer = Vec::new();
    let mut bytes = [0u8; 4096];
    let mut stdin = io::stdin();

    'reading: while let Ok(count @ 1..) = stdin.read(&mut bytes) {
        buffer.extend_from_slice(&bytes[..count]);
        for sequence in parse(&mut buffer) {
            let events = match sequence.params.strip_prefix('<') {
                Some(mouse) => mouse_events(mouse, sequence.final_byte, *geometry.lock().unwrap()),
                None => match key_event(&sequence, &mut held) {
                    KeyAction::Quit => break 'reading,
                    KeyAction::Send(event) => vec![event],
                    KeyAction::Ignore => Vec::new(),
                },
            };
            for event in events {
                if protocol::write_json(input, &event).is_err() {
                    return;
                }
            }
        }
    }

    for code in held {
        let _ = protocol::write_json(input, &InputEvent::Key { code, down: false });
    }
}

enum KeyAction {
    Send(InputEvent),
    Quit,
    Ignore,
}

/// `CSI number[:alternates] ; modifiers[:event] final`, from the Kitty keyboard protocol.
fn key_event(sequence: &Sequence, held: &mut BTreeSet<u16>) -> KeyAction {
    let mut fields = sequence.params.split(';');
    let number = fields
        .next()
        .and_then(|field| field.split(':').next())
        .and_then(|number| number.parse().ok())
        .unwrap_or(1);
    let mut modifier_fields = fields.next().unwrap_or("1").split(':');
    let modifiers: u32 = modifier_fields.next().and_then(|value| value.parse().ok()).unwrap_or(1);
    let event: u32 = modifier_fields.next().and_then(|value| value.parse().ok()).unwrap_or(1);

    let ctrl = (modifiers - 1) & 4 != 0;
    if sequence.final_byte == b'u' && number == u32::from(b']') && ctrl && event == 1 {
        return KeyAction::Quit;
    }
    // Repeats come from the guest's own key repeat instead.
    if event == 2 {
        return KeyAction::Ignore;
    }
    let Some(code) = keymap::kitty_key(number, sequence.final_byte) else {
        return KeyAction::Ignore;
    };
    let down = event != 3;
    if down {
        held.insert(code);
    } else {
        held.remove(&code);
    }
    KeyAction::Send(InputEvent::Key { code, down })
}

/// `CSI < button ; x ; y M|m` with x and y in terminal pixels.
fn mouse_events(params: &str, final_byte: u8, geometry: Option<Geometry>) -> Vec<InputEvent> {
    let fields: Vec<&str> = params.split(';').collect();
    let (Some(geometry), [button, x, y]) = (geometry, fields.as_slice()) else {
        return Vec::new();
    };
    let (Ok(button), Ok(x), Ok(y)) = (button.parse::<u32>(), x.parse::<f64>(), y.parse::<f64>()) else {
        return Vec::new();
    };
    // Pixel coordinates are 1-based and relative to the top left, where the frame is.
    let (x, y) = (x - 1.0, y - 1.0);
    if x >= geometry.width || y >= geometry.height {
        return Vec::new();
    }
    let position = InputEvent::Move {
        x: (x / geometry.width * f64::from(geometry.image_width)) as u32,
        y: (y / geometry.height * f64::from(geometry.image_height)) as u32,
    };

    let button_bits = button & 0b11;
    if button & 64 != 0 {
        let (value, horizontal) = match button_bits {
            0 => (1, false),
            1 => (-1, false),
            2 => (-1, true),
            _ => (1, true),
        };
        return vec![position, InputEvent::Scroll { value, horizontal }];
    }
    if button & 32 != 0 || button_bits == 3 {
        return vec![position];
    }
    let button = match button_bits {
        0 => BTN_LEFT,
        1 => BTN_MIDDLE,
        _ => BTN_RIGHT,
    };
    vec![
        position,
        InputEvent::Button {
            button,
            down: final_byte == b'M',
        },
    ]
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let value = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for (index, shift) in [18, 12, 6, 0].into_iter().enumerate() {
            if index <= chunk.len() {
                out.push(ALPHABET[(value >> shift & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}
