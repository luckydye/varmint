//! `varmint screen`: shows a VM's screen live in the terminal with the Kitty
//! graphics protocol, which Ghostty, Kitty and WezTerm implement.

use crossterm::terminal;
use std::io::{self, Read, Write};

/// Image and placement ids; reusing them replaces the previous frame in place.
const IMAGE_ID: u32 = 1;
const PLACEMENT_ID: u32 = 1;
/// The protocol limits each escape sequence to 4096 bytes of payload.
const CHUNK: usize = 4096;

/// Puts the terminal in a state for drawing frames and restores it on drop.
pub struct Display;

impl Display {
    pub fn enter() -> io::Result<Self> {
        terminal::enable_raw_mode()?;
        // Alternate screen and hidden cursor, so the shell is untouched afterwards.
        io::stdout().write_all(b"\x1b[?1049h\x1b[?25l")?;
        io::stdout().flush()?;
        Ok(Self)
    }
}

impl Drop for Display {
    fn drop(&mut self) {
        leave();
    }
}

/// Removes the image and gives the terminal back as it was.
pub fn leave() {
    let mut stdout = io::stdout();
    let _ = write!(stdout, "\x1b_Ga=d,d=I,i={IMAGE_ID},q=2\x1b\\\x1b[?25h\x1b[?1049l");
    let _ = stdout.flush();
    let _ = terminal::disable_raw_mode();
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

/// Cells that fit an image of `width`x`height` pixels into the terminal
/// while keeping its aspect ratio.
fn fit(width: u32, height: u32) -> io::Result<(u32, u32)> {
    let window = terminal::window_size()?;
    if window.width == 0 || window.height == 0 {
        return Err(io::Error::other(
            "the terminal does not report its size in pixels; use Ghostty, Kitty or WezTerm",
        ));
    }
    let cell_width = f64::from(window.width) / f64::from(window.columns);
    let cell_height = f64::from(window.height) / f64::from(window.rows);
    let scale = (f64::from(window.width) / f64::from(width)).min(f64::from(window.height) / f64::from(height));
    let columns = ((f64::from(width) * scale / cell_width).floor() as u32).max(1);
    let rows = ((f64::from(height) * scale / cell_height).floor() as u32).max(1);
    Ok((columns, rows))
}

/// Draws one PNG frame at the top left, scaled to fit the terminal.
pub fn draw(png: &[u8]) -> io::Result<()> {
    let (width, height) = png_size(png)?;
    let (columns, rows) = fit(width, height)?;
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
    stdout.flush()
}

/// Reads one length-prefixed PNG frame of a screen stream.
pub fn read_frame(reader: &mut impl Read) -> io::Result<Vec<u8>> {
    let mut length = [0u8; 4];
    reader.read_exact(&mut length)?;
    let mut png = vec![0; u32::from_le_bytes(length) as usize];
    reader.read_exact(&mut png)?;
    Ok(png)
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
