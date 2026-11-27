//! Private Unix-socket protocol. Numbers are big-endian; PTY bytes are opaque.

use std::io::{self, Read, Write};

use portable_pty::PtySize;

pub(super) const ATTACH: u8 = b'A';
pub(super) const STATUS: u8 = b'S';
pub(super) const KILL: u8 = b'K';
pub(super) const INPUT: u8 = b'I';
pub(super) const RESIZE: u8 = b'R';
pub(super) const OUTPUT: u8 = b'O';
pub(super) const EXIT: u8 = b'E';
pub(super) const CHUNK: usize = 16 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Size {
    pub cols: u16,
    pub rows: u16,
}

impl Size {
    pub fn new(cols: u16, rows: u16) -> io::Result<Self> {
        if cols == 0 || rows == 0 {
            return Err(invalid("terminal size must be positive"));
        }
        Ok(Self { cols, rows })
    }

    pub fn read(reader: &mut impl Read) -> io::Result<Self> {
        let mut bytes = [0; 4];
        reader.read_exact(&mut bytes)?;
        let [a, b, c, d] = bytes;
        Self::new(u16::from_be_bytes([a, b]), u16::from_be_bytes([c, d]))
    }

    pub fn send(self, tag: u8, writer: &mut impl Write) -> io::Result<()> {
        let [a, b] = self.cols.to_be_bytes();
        let [c, d] = self.rows.to_be_bytes();
        writer.write_all(&[tag, a, b, c, d])
    }

    pub fn pty(self) -> PtySize {
        PtySize {
            cols: self.cols,
            rows: self.rows,
            pixel_width: 0,
            pixel_height: 0,
        }
    }
}

pub(super) fn byte(reader: &mut impl Read) -> io::Result<u8> {
    let mut bytes = [0; 1];
    reader.read_exact(&mut bytes)?;
    Ok(bytes[0])
}

pub(super) fn read_data<'a>(
    reader: &mut impl Read,
    bytes: &'a mut [u8; CHUNK],
) -> io::Result<&'a [u8]> {
    let mut len = [0; 2];
    reader.read_exact(&mut len)?;
    let len = usize::from(u16::from_be_bytes(len));
    let data = bytes
        .get_mut(..len)
        .ok_or_else(|| invalid("terminal data frame is too large"))?;
    reader.read_exact(data)?;
    Ok(data)
}

pub(super) fn write_data(writer: &mut impl Write, tag: u8, bytes: &[u8]) -> io::Result<()> {
    for chunk in bytes.chunks(CHUNK) {
        let [a, b] = (chunk.len() as u16).to_be_bytes();
        writer.write_all(&[tag, a, b])?;
        writer.write_all(chunk)?;
    }
    Ok(())
}

pub(super) fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
