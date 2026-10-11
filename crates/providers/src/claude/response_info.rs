//! Chromium's `HttpResponseInfo` as its HTTP cache stores it (stream 0 of a cache entry), read only
//! as far as Lastdrop needs — times, status, `Content-Encoding`, whether the cache compressed the
//! body — and the decoding that turns a stored body (stream 1) back into the bytes the page got.
//!
//! **`base::Pickle`** (`base/pickle.h`): a `Header { uint32 payload_size }`, then the payload.
//! Every value starts on a 4-byte boundary: `int` 4 bytes, `int64` 8, a string an `int` length and
//! its bytes padded to 4.
//!
//! **`HttpResponseInfo::InitFromPickle`** (`net/http/http_response_info.cc`) reads, in order:
//! `flags` (int: the version in bits 0–7, `RESPONSE_INFO_VERSION` =
//! `RESPONSE_INFO_MINIMUM_VERSION` = 3; `RESPONSE_INFO_TRUNCATED` = 1 << 12;
//! `RESPONSE_INFO_HAS_EXTRA_FLAGS` = 1 << 31); `extra_flags` (int, only with that bit); `request_time` and `response_time` (int64, `base::Time`
//! internal values: microseconds since 1601-01-01 UTC); `original_response_time` (int64, only with
//! `RESPONSE_EXTRA_INFO_HAS_ORIGINAL_RESPONSE_TIME` = 1 << 2); the headers (`HttpResponseHeaders`'
//! raw form, one string: the status line, then `name: value` lines, each ended by a NUL); then
//! certificate, connection and other details Lastdrop does not need. `MakePickleImpl` writes the
//! zstd-uncompressed body size (int64) **last**, flagged by
//! `RESPONSE_EXTRA_INFO_HAS_ZSTD_UNCOMPRESSED_BODY_SIZE` = 1 << 4 ("the response body on disk is
//! zstd-compressed"), so it is read from the payload's last 8 bytes without walking the variable
//! fields before it.
//!
//! **Bodies.** The HTTP cache stores a body as it came from the network, still in its
//! `Content-Encoding`; newer Chromium may also compress it with zstd for the disk (the flag above).
//! [`decode_body`] undoes the disk zstd, then each content coding in reverse order of application:
//! `br` (brotli-decompressor), `zstd` (ruzstd, checksum verified), `gzip` / `x-gzip` / `deflate`
//! (flate2's pure-Rust backend), `identity`. Pure Rust throughout; every output is capped.
//!
//! The raw headers can hold cookies (`Set-Cookie`): only the status code and `Content-Encoding`
//! are kept, and the caller wipes the stream's buffer after [`parse`].

use std::io::{self, Read};

use jiff::Timestamp;

use super::blockfile::chromium_time;

/// `RESPONSE_INFO_VERSION` (and `RESPONSE_INFO_MINIMUM_VERSION`).
pub const RESPONSE_INFO_VERSION: u32 = 3;
/// `RESPONSE_INFO_VERSION_MASK`.
const VERSION_MASK: u32 = 0xFF;
/// `RESPONSE_INFO_TRUNCATED`: the request was cancelled before the body was complete.
pub const TRUNCATED: u32 = 1 << 12;
/// `RESPONSE_INFO_HAS_EXTRA_FLAGS`.
pub const HAS_EXTRA_FLAGS: u32 = 1 << 31;
/// `RESPONSE_EXTRA_INFO_HAS_ORIGINAL_RESPONSE_TIME`.
pub const EXTRA_HAS_ORIGINAL_RESPONSE_TIME: u32 = 1 << 2;
/// `RESPONSE_EXTRA_INFO_HAS_ZSTD_UNCOMPRESSED_BODY_SIZE`.
pub const EXTRA_HAS_ZSTD_UNCOMPRESSED_BODY_SIZE: u32 = 1 << 4;

/// Largest zstd window accepted (**not from Chromium**): a usage body is a few KiB, and the cap
/// bounds what a frame header can make the decoder allocate.
const MAX_ZSTD_WINDOW: u64 = 8 << 20;

/// What Lastdrop reads of a cached response's info.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ResponseInfo {
    /// When the request was sent.
    pub request_time: Option<Timestamp>,
    /// When the response's headers arrived (or were last revalidated).
    pub response_time: Option<Timestamp>,
    /// The status code of the status line.
    pub status: Option<u16>,
    /// The `Content-Encoding` codings in the order the server applied them, lower case.
    pub content_encoding: Vec<String>,
    /// `RESPONSE_INFO_TRUNCATED`: the stored body is incomplete.
    pub truncated: bool,
    /// The body's size before the cache's own zstd compression, when the cache compressed it.
    pub disk_zstd_size: Option<u64>,
}

impl ResponseInfo {
    /// The content codings as a header would list them (`br`, `gzip, br`), `identity` for none.
    pub fn content_encoding_label(&self) -> String {
        if self.content_encoding.is_empty() {
            "identity".to_owned()
        } else {
            self.content_encoding.join(", ")
        }
    }
}

/// A response info that cannot be read.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum InfoError {
    /// Shorter than its fields, or sizes that do not add up.
    #[error("unreadable response info ({0})")]
    Malformed(&'static str),
    /// A version other than [`RESPONSE_INFO_VERSION`].
    #[error("response info version {0}, Lastdrop reads version 3")]
    Version(u32),
}

/// A body that cannot be decoded.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum DecodeError {
    /// A content coding Lastdrop does not decode (the name, when it is a plain token).
    #[error("unsupported content-encoding {0}")]
    Unsupported(String),
    /// The data is not valid in the coding it claims (`br`, `zstd`, `gzip`, `disk zstd`).
    #[error("invalid {0} data")]
    Corrupt(&'static str),
    /// Decoded, it would exceed the cap.
    #[error("decoded body larger than {0} bytes")]
    TooLarge(usize),
}

/// A `base::Pickle`'s payload, read from the front.
struct Pickle<'a> {
    payload: &'a [u8],
    pos: usize,
}

impl<'a> Pickle<'a> {
    fn new(data: &'a [u8]) -> Result<Pickle<'a>, InfoError> {
        let size = data
            .get(..4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .ok_or(InfoError::Malformed("no pickle header"))?;
        let payload = usize::try_from(size)
            .ok()
            .and_then(|size| data.get(4..4usize.checked_add(size)?))
            .ok_or(InfoError::Malformed("pickle longer than the stream"))?;
        Ok(Pickle { payload, pos: 0 })
    }

    /// `len` bytes, then the position moves to the next 4-byte boundary.
    fn bytes(&mut self, len: usize) -> Result<&'a [u8], InfoError> {
        let end = self
            .pos
            .checked_add(len)
            .filter(|end| *end <= self.payload.len())
            .ok_or(InfoError::Malformed("field past the end"))?;
        let bytes = &self.payload[self.pos..end];
        self.pos = end.next_multiple_of(4).min(self.payload.len());
        Ok(bytes)
    }

    fn int(&mut self) -> Result<i32, InfoError> {
        let b = self.bytes(4)?;
        Ok(i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn int64(&mut self) -> Result<i64, InfoError> {
        let b = self.bytes(8)?;
        let mut v = [0u8; 8];
        v.copy_from_slice(b);
        Ok(i64::from_le_bytes(v))
    }

    fn string(&mut self) -> Result<&'a [u8], InfoError> {
        let len = usize::try_from(self.int()?)
            .map_err(|_| InfoError::Malformed("negative string length"))?;
        self.bytes(len)
    }
}

/// Parses stream 0 of an HTTP cache entry (module docs). Only the status code and the
/// `Content-Encoding` header are taken from the headers; nothing else of them is kept.
pub fn parse(stream0: &[u8]) -> Result<ResponseInfo, InfoError> {
    let mut pickle = Pickle::new(stream0)?;
    let flags = pickle.int()? as u32;
    let extra = if flags & HAS_EXTRA_FLAGS != 0 {
        pickle.int()? as u32
    } else {
        0
    };
    let version = flags & VERSION_MASK;
    if version != RESPONSE_INFO_VERSION {
        return Err(InfoError::Version(version));
    }
    let request_time = chromium_time(pickle.int64()?);
    let response_time = chromium_time(pickle.int64()?);
    if extra & EXTRA_HAS_ORIGINAL_RESPONSE_TIME != 0 {
        pickle.int64()?;
    }
    let raw_headers = pickle.string()?;
    let (status, content_encoding) = read_headers(raw_headers);
    let disk_zstd_size = if extra & EXTRA_HAS_ZSTD_UNCOMPRESSED_BODY_SIZE != 0 {
        // Written last by `MakePickleImpl`; never inside what was already read.
        let len = pickle.payload.len();
        if len < pickle.pos + 8 {
            return Err(InfoError::Malformed("no zstd body size"));
        }
        let mut v = [0u8; 8];
        v.copy_from_slice(&pickle.payload[len - 8..]);
        Some(
            u64::try_from(i64::from_le_bytes(v))
                .map_err(|_| InfoError::Malformed("negative zstd body size"))?,
        )
    } else {
        None
    };
    Ok(ResponseInfo {
        request_time,
        response_time,
        status,
        content_encoding,
        truncated: flags & TRUNCATED != 0,
        disk_zstd_size,
    })
}

/// The status code and the `Content-Encoding` codings of `HttpResponseHeaders`' raw form
/// (`HTTP/1.1 200\0content-type: …\0content-encoding: br\0\0`). Several `Content-Encoding` lines
/// add up in order.
fn read_headers(raw: &[u8]) -> (Option<u16>, Vec<String>) {
    let mut lines = raw.split(|b| *b == 0);
    let status = lines
        .next()
        .and_then(|line| std::str::from_utf8(line).ok())
        .and_then(|line| line.split_ascii_whitespace().nth(1))
        .and_then(|code| code.parse().ok());
    let mut codings = Vec::new();
    for line in lines {
        let Some(colon) = line.iter().position(|b| *b == b':') else {
            continue;
        };
        if !line[..colon]
            .trim_ascii()
            .eq_ignore_ascii_case(b"content-encoding")
        {
            continue;
        }
        let Ok(value) = std::str::from_utf8(&line[colon + 1..]) else {
            codings.push(String::new());
            continue;
        };
        codings.extend(
            value
                .split(',')
                .map(|c| c.trim().to_ascii_lowercase())
                .filter(|c| !c.is_empty()),
        );
    }
    (status, codings)
}

/// A coding's name for messages: itself when it is a short plain token, else `?`.
fn token(name: &str) -> String {
    let plain = !name.is_empty()
        && name.len() <= 16
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if plain {
        name.to_owned()
    } else {
        "?".to_owned()
    }
}

/// The stored body `stored` decoded to the bytes the page received (module docs): the cache's
/// own zstd first, then each content coding of `info` in reverse order; at most `max` bytes at
/// every step.
pub fn decode_body(stored: &[u8], info: &ResponseInfo, max: usize) -> Result<Vec<u8>, DecodeError> {
    let mut body = match info.disk_zstd_size {
        Some(size) if size > max as u64 => return Err(DecodeError::TooLarge(max)),
        Some(_) => zstd(stored, max, "disk zstd")?,
        None => stored.to_vec(),
    };
    for coding in info.content_encoding.iter().rev() {
        body = match coding.as_str() {
            "identity" => continue,
            "br" => capped(
                brotli_decompressor::Decompressor::new(body.as_slice(), 4096),
                max,
                "br",
            )?,
            "zstd" => zstd(&body, max, "zstd")?,
            "gzip" | "x-gzip" => capped(
                flate2::read::MultiGzDecoder::new(body.as_slice()),
                max,
                "gzip",
            )?,
            // `deflate` is zlib-wrapped (RFC 9110 §8.4.1.2); some servers send it raw, which
            // Chromium accepts too.
            "deflate" => capped(
                flate2::read::ZlibDecoder::new(body.as_slice()),
                max,
                "deflate",
            )
            .or_else(|_| {
                capped(
                    flate2::read::DeflateDecoder::new(body.as_slice()),
                    max,
                    "deflate",
                )
            })?,
            other => return Err(DecodeError::Unsupported(token(other))),
        };
    }
    Ok(body)
}

/// Everything `reader` gives, at most `max` bytes.
fn capped(reader: impl Read, max: usize, coding: &'static str) -> Result<Vec<u8>, DecodeError> {
    let mut out = Vec::new();
    reader
        .take(max as u64 + 1)
        .read_to_end(&mut out)
        .map_err(|_| DecodeError::Corrupt(coding))?;
    if out.len() > max {
        return Err(DecodeError::TooLarge(max));
    }
    Ok(out)
}

/// Every zstd frame of `input` decoded and concatenated, checksums verified, at most `max`
/// bytes. Skippable frames are not expected here and count as invalid data.
fn zstd(input: &[u8], max: usize, coding: &'static str) -> Result<Vec<u8>, DecodeError> {
    use ruzstd::decoding::StreamingDecoder;

    let mut rest = input;
    let mut out = Vec::new();
    if rest.is_empty() {
        return Err(DecodeError::Corrupt(coding));
    }
    while !rest.is_empty() {
        let mut decoder = StreamingDecoder::new_with_max_window_size(&mut rest, MAX_ZSTD_WINDOW)
            .map_err(|_| DecodeError::Corrupt(coding))?;
        let room = (max - out.len()) as u64 + 1;
        (&mut decoder)
            .take(room)
            .read_to_end(&mut out)
            .map_err(|_: io::Error| DecodeError::Corrupt(coding))?;
        if out.len() > max {
            return Err(DecodeError::TooLarge(max));
        }
        let frame = &decoder.decoder;
        if !frame.is_finished() {
            return Err(DecodeError::Corrupt(coding));
        }
        if let (Some(stored), Some(computed)) = (
            frame.get_checksum_from_data(),
            frame.get_calculated_checksum(),
        ) && stored != computed
        {
            return Err(DecodeError::Corrupt(coding));
        }
    }
    Ok(out)
}
