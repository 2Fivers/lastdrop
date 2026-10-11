//! A minimal PNG encoder for the `--preview-flyout` snapshot: 8-bit RGBA, no filtering, deflate
//! "stored" blocks (no compression). The files are a few hundred KB, which is fine for a
//! development aid, and it keeps an image crate out of the binary (ARCHITECTURE goal 1).

use std::io::Write;
use std::path::Path;

/// Encodes `rgba` (straight alpha, top-down rows, `width × height × 4` bytes) as a PNG.
///
/// Returns `None` when the buffer does not match the size or a dimension is zero.
pub fn encode_rgba(width: u32, height: u32, rgba: &[u8]) -> Option<Vec<u8>> {
    let row = usize::try_from(width).ok()?.checked_mul(4)?;
    let rows = usize::try_from(height).ok()?;
    if width == 0 || height == 0 || rgba.len() != row.checked_mul(rows)? {
        return None;
    }
    // Each scanline starts with filter type 0 (None).
    let mut raw = Vec::with_capacity((row + 1) * rows);
    for line in rgba.chunks_exact(row) {
        raw.push(0);
        raw.extend_from_slice(line);
    }

    let mut out = Vec::with_capacity(raw.len() + raw.len() / 65_535 * 5 + 64);
    out.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    // Bit depth 8, colour type 6 (RGBA), deflate, adaptive filtering, no interlace.
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &zlib_stored(&raw));
    chunk(&mut out, b"IEND", &[]);
    Some(out)
}

/// Encodes and writes a PNG file.
pub fn write_rgba(path: &Path, width: u32, height: u32, rgba: &[u8]) -> std::io::Result<()> {
    let bytes = encode_rgba(width, height, rgba).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "image buffer does not match its size",
        )
    })?;
    let mut file = std::fs::File::create(path)?;
    file.write_all(&bytes)?;
    file.sync_all()
}

/// A zlib stream (RFC 1950) of deflate stored blocks (RFC 1951 §3.2.4).
fn zlib_stored(data: &[u8]) -> Vec<u8> {
    const MAX_BLOCK: usize = 65_535;
    let mut z = Vec::with_capacity(data.len() + data.len() / MAX_BLOCK * 5 + 11);
    // CMF: deflate, 32 K window; FLG: no dictionary, fastest; (0x78 << 8 | 0x01) % 31 == 0.
    z.extend_from_slice(&[0x78, 0x01]);
    let mut blocks = data.chunks(MAX_BLOCK).peekable();
    if blocks.peek().is_none() {
        // One empty final block.
        z.extend_from_slice(&[1, 0, 0, 0xFF, 0xFF]);
    }
    while let Some(block) = blocks.next() {
        let last = blocks.peek().is_none();
        z.push(u8::from(last));
        // `chunks(MAX_BLOCK)` keeps every block within u16.
        let len = u16::try_from(block.len()).unwrap_or(u16::MAX);
        z.extend_from_slice(&len.to_le_bytes());
        z.extend_from_slice(&(!len).to_le_bytes());
        z.extend_from_slice(block);
    }
    z.extend_from_slice(&adler32(data).to_be_bytes());
    z
}

/// One PNG chunk: length, type, data, CRC of type and data.
fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    let len = u32::try_from(data.len()).unwrap_or(u32::MAX);
    out.extend_from_slice(&len.to_be_bytes());
    let start = out.len();
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let crc = crc32(&out[start..]);
    out.extend_from_slice(&crc.to_be_bytes());
}

/// CRC-32 (ISO 3309, as PNG uses it).
fn crc32(data: &[u8]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for &b in data {
        c ^= u32::from(b);
        for _ in 0..8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
        }
    }
    !c
}

/// Adler-32 (RFC 1950).
fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in data {
        a = (a + u32::from(byte)) % 65_521;
        b = (b + a) % 65_521;
    }
    (b << 16) | a
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksums_match_known_values() {
        assert_eq!(crc32(b"IEND"), 0xAE42_6082);
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
    }

    #[test]
    fn encodes_a_small_image() {
        let rgba = [255, 0, 0, 255, 0, 255, 0, 128];
        let png = encode_rgba(2, 1, &rgba).expect("valid size");
        assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        assert_eq!(&png[12..16], b"IHDR");
        assert_eq!(&png[16..20], &2u32.to_be_bytes());
        assert_eq!(&png[png.len() - 8..png.len() - 4], b"IEND");
        assert!(encode_rgba(3, 1, &rgba).is_none());
        assert!(encode_rgba(0, 0, &[]).is_none());
    }

    #[test]
    fn splits_large_data_into_stored_blocks() {
        let data = vec![7u8; 70_000];
        let z = zlib_stored(&data);
        // Header, two blocks with 5-byte headers, Adler-32.
        assert_eq!(z.len(), 2 + 5 + 65_535 + 5 + (70_000 - 65_535) + 4);
        assert_eq!(z[2], 0, "first block is not final");
        assert_eq!(z[2 + 5 + 65_535], 1, "second block is final");
    }
}
