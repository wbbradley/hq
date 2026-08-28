//! Shared bounded framing primitives for blocking local Unix clients.

use std::{io, io::Read, io::Write, os::unix::net::UnixStream};

use hq_local_api::protocol::v1::MAX_FRAME_BYTES;

pub(crate) fn write_frame(stream: &mut UnixStream, frame: &[u8]) -> io::Result<()> {
    stream.write_all(frame)
}

pub(crate) fn read_frame(stream: &mut UnixStream) -> io::Result<Vec<u8>> {
    let mut prefix = [0_u8; 4];
    stream.read_exact(&mut prefix)?;
    let body_len = usize::try_from(u32::from_be_bytes(prefix))
        .map_err(|_| io::Error::from(io::ErrorKind::InvalidData))?;
    if body_len > MAX_FRAME_BYTES {
        return Err(io::Error::from(io::ErrorKind::InvalidData));
    }
    let frame_len = body_len
        .checked_add(prefix.len())
        .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidData))?;
    let mut frame = prefix.to_vec();
    frame.resize(frame_len, 0);
    stream.read_exact(&mut frame[prefix.len()..])?;
    Ok(frame)
}
