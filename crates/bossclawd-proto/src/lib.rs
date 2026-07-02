//! # bossclawd-proto
//!
//! Wire protocol for `bossclawd`, the local Unix-socket daemon that owns the
//! desktop app's memory engine. This crate is deliberately tiny: it is the
//! length-prefixed frame codec and nothing else. Request/Response types and the
//! handshake are layered on top of these frames elsewhere (M1a Task 2).
//!
//! ## Frame format
//!
//! Each frame is a big-endian [`u32`] length prefix followed by exactly that
//! many payload bytes. Big-endian is the conventional network byte order and
//! keeps the on-wire form language-agnostic. A zero-length payload is a valid
//! frame (the prefix is `0` and no body follows).
//!
//! ## Size guard
//!
//! [`MAX_FRAME`] (32 MiB) caps the payload of a single frame. The cap is
//! enforced on both ends: [`write_frame`] refuses to send an oversize payload,
//! and [`read_frame`] rejects an oversize *declared* length **before** it
//! allocates or reads the body. This makes the reader safe against a hostile or
//! buggy peer that announces a huge length to force a large allocation.

#![forbid(unsafe_code)]

use std::io;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Maximum payload size, in bytes, of a single frame (32 MiB).
///
/// A frame whose payload exceeds this is rejected by both [`write_frame`] and
/// [`read_frame`]. The bound protects the reader from a peer that declares a
/// huge length to trigger an oversized allocation.
pub const MAX_FRAME: usize = 32 * 1024 * 1024;

/// Write one length-prefixed frame: a big-endian [`u32`] length followed by
/// `payload`.
///
/// # Errors
///
/// Returns [`io::ErrorKind::InvalidInput`] if `payload` is longer than
/// [`MAX_FRAME`] (nothing is written in that case), or propagates any I/O error
/// from the underlying writer. The frame is flushed before returning so the
/// bytes are handed to the OS rather than sitting in a buffer.
pub async fn write_frame<W>(writer: &mut W, payload: &[u8]) -> io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    if payload.len() > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "frame payload of {} bytes exceeds MAX_FRAME ({MAX_FRAME} bytes)",
                payload.len()
            ),
        ));
    }
    // `payload.len() <= MAX_FRAME` (a `u32`-sized bound), so this never truncates.
    let len = payload.len() as u32;
    writer.write_all(&len.to_be_bytes()).await?;
    writer.write_all(payload).await?;
    writer.flush().await?;
    Ok(())
}

/// Read one length-prefixed frame written by [`write_frame`], returning its
/// payload bytes.
///
/// # Errors
///
/// - [`io::ErrorKind::InvalidData`] if the declared length exceeds
///   [`MAX_FRAME`]. The oversize length is rejected **before** any body bytes
///   are allocated or read.
/// - [`io::ErrorKind::UnexpectedEof`] if the peer closes the connection before a
///   full frame has arrived — whether mid-prefix or mid-body. A clean EOF at a
///   frame boundary (before any prefix byte) surfaces the same way, so callers
///   treat `UnexpectedEof` as "the stream ended".
/// - any other I/O error from the underlying reader is propagated as-is.
pub async fn read_frame<R>(reader: &mut R) -> io::Result<Vec<u8>>
where
    R: AsyncRead + Unpin,
{
    let mut len_buf = [0u8; 4];
    // `read_exact` maps a peer close (0 bytes read while bytes are still needed)
    // to `UnexpectedEof`, so a mid-prefix or mid-body close errors rather than
    // hanging or panicking.
    reader.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf) as usize;

    if len > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("declared frame length {len} bytes exceeds MAX_FRAME ({MAX_FRAME} bytes)"),
        ));
    }

    let mut payload = vec![0u8; len];
    reader.read_exact(&mut payload).await?;
    Ok(payload)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::duplex;

    #[tokio::test]
    async fn frame_roundtrip() {
        let (mut a, mut b) = duplex(1024);
        write_frame(&mut a, b"hello frame").await.unwrap();
        assert_eq!(read_frame(&mut b).await.unwrap(), b"hello frame");
    }

    #[tokio::test]
    async fn empty_frame_roundtrip() {
        // A zero-length payload is a legitimate frame.
        let (mut a, mut b) = duplex(1024);
        write_frame(&mut a, b"").await.unwrap();
        assert_eq!(read_frame(&mut b).await.unwrap(), b"");
    }

    #[tokio::test]
    async fn max_size_frame_roundtrip() {
        // A payload of exactly MAX_FRAME is allowed on both ends.
        let payload = vec![0x5au8; MAX_FRAME];
        // duplex buffer must hold prefix + body without a reader draining it.
        let (mut a, mut b) = duplex(MAX_FRAME + 4);
        write_frame(&mut a, &payload).await.unwrap();
        assert_eq!(read_frame(&mut b).await.unwrap(), payload);
    }

    #[tokio::test]
    async fn write_rejects_oversize_payload() {
        // One byte past the cap must be refused, and nothing may be written.
        let payload = vec![0u8; MAX_FRAME + 1];
        let (mut a, mut b) = duplex(1024);
        let err = write_frame(&mut a, &payload).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);

        // Prove no partial frame leaked onto the wire: drop the writer so the
        // reader sees EOF immediately instead of blocking, then confirm the
        // reader gets a clean EOF (zero bytes buffered), not a stray prefix.
        drop(a);
        let read_err = read_frame(&mut b).await.unwrap_err();
        assert_eq!(read_err.kind(), io::ErrorKind::UnexpectedEof);
    }

    #[tokio::test]
    async fn read_rejects_oversize_declared_length_before_body() {
        // Hand-craft a frame header that declares MAX_FRAME + 1 bytes but send
        // NO body. If the reader tried to read the body it would block forever;
        // rejecting on the declared length alone means it returns promptly.
        let (mut a, mut b) = duplex(1024);
        let bogus_len = (MAX_FRAME + 1) as u32;
        a.write_all(&bogus_len.to_be_bytes()).await.unwrap();
        a.flush().await.unwrap();

        let err = read_frame(&mut b).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[tokio::test]
    async fn read_errors_on_eof_before_any_prefix() {
        // Peer closes at a frame boundary having sent nothing.
        let (a, mut b) = duplex(1024);
        drop(a);
        let err = read_frame(&mut b).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
    }

    #[tokio::test]
    async fn read_errors_on_peer_close_mid_frame() {
        // Peer sends a valid prefix promising 100 bytes, one body byte, then
        // closes. The reader must error (UnexpectedEof), not hang or panic.
        let (mut a, mut b) = duplex(1024);
        a.write_all(&100u32.to_be_bytes()).await.unwrap();
        a.write_all(b"x").await.unwrap();
        a.flush().await.unwrap();
        drop(a);

        let err = read_frame(&mut b).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
    }

    #[tokio::test]
    async fn two_frames_back_to_back() {
        // Framing must delimit correctly: two writes, two clean reads.
        let (mut a, mut b) = duplex(1024);
        write_frame(&mut a, b"first").await.unwrap();
        write_frame(&mut a, b"second").await.unwrap();
        assert_eq!(read_frame(&mut b).await.unwrap(), b"first");
        assert_eq!(read_frame(&mut b).await.unwrap(), b"second");
    }
}
