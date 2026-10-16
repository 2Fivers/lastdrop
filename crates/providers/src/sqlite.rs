//! A minimal, read-only reader of SQLite database files: enough to look up one row of a small
//! table by its first column (Cursor's `ItemTable` in `state.vscdb`), page by page through
//! [`crate::io::Fs::open_at`], so a 1.6 GB file is never read whole or copied and nothing is
//! written next to it (no journal, no lock, no `-shm`).
//!
//! It walks table b-trees only (SQLite's file format, sqlite.org/fileformat2.html): the schema
//! table on page 1 gives the table's root page, and the table's leaf cells give its rows. Only the
//! columns a lookup needs are decoded, and a row's overflow pages are read only when it is the row
//! asked for, so an unrelated large row costs nothing. Only UTF-8 databases are read.
//!
//! **Write-ahead log.** A database in WAL mode (header bytes 18–19 = 2) keeps its newest pages in
//! `<file>-wal` until a checkpoint copies them back. [`Db::open`] takes that file too and reads it
//! the way SQLite recovers it: frames whose salts match the log's header and whose running
//! checksums hold, up to the last commit frame; the newest committed copy of a page wins over the
//! main file. The `-shm` index is not used (it is shared memory, and reading it changes nothing
//! here).
//!
//! **No locks.** Another process (Cursor) may write while this reads. A torn read is caught where
//! it can be: a malformed page is an error, and in rollback mode a change counter (header offset
//! 24) that moved during the read is reported by [`Db::changed`]. Callers treat either as a passing
//! failure and read again later.
//!
//! Every length and page number read from a file is checked; a malformed file is an
//! `InvalidData` error, never a panic. A walk visits at most [`MAX_PAGES`] pages and never the
//! same page twice, so a damaged file cannot loop or grow memory without end.

use std::collections::{HashMap, HashSet};
use std::io;

use crate::io::ReadAt;

/// The most pages one walk visits.
pub const MAX_PAGES: u32 = 100_000;

/// The largest payload put together (the value of the row asked for), in bytes.
pub const MAX_PAYLOAD: u64 = 16 << 20;

/// The largest write-ahead log read, in bytes (a log grows until a checkpoint; SQLite's default
/// checkpoints at 1,000 pages).
pub const MAX_WAL: u64 = 256 << 20;

/// The magic string at the start of every SQLite database file.
const MAGIC: &[u8; 16] = b"SQLite format 3\0";

/// The magics of a write-ahead log: little-endian checksums, big-endian checksums.
const WAL_MAGIC_LE: u32 = 0x377f_0682;
/// See [`WAL_MAGIC_LE`].
const WAL_MAGIC_BE: u32 = 0x377f_0683;

/// Page type of a table b-tree interior page.
const TABLE_INTERIOR: u8 = 0x05;
/// Page type of a table b-tree leaf page.
const TABLE_LEAF: u8 = 0x0D;

fn malformed(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("sqlite: {what}"))
}

/// One column value of a row.
#[derive(Clone, Debug, PartialEq)]
pub enum Column {
    /// NULL.
    Null,
    /// An integer.
    Integer(i64),
    /// A float.
    Float(f64),
    /// Text (UTF-8 bytes).
    Text(Vec<u8>),
    /// A blob.
    Blob(Vec<u8>),
}

impl Column {
    /// The bytes of a text or blob value.
    pub fn bytes(&self) -> Option<&[u8]> {
        match self {
            Column::Text(b) | Column::Blob(b) => Some(b),
            Column::Null | Column::Integer(_) | Column::Float(_) => None,
        }
    }

    /// Overwrites the bytes of a text or blob value (a value that held a secret).
    pub fn wipe(&mut self) {
        if let Column::Text(b) | Column::Blob(b) = self {
            b.fill(0);
            std::hint::black_box(&*b);
        }
    }
}

/// The committed pages of a write-ahead log.
struct Wal {
    file: Box<dyn ReadAt>,
    /// Page number → offset of the newest committed copy's data in the log.
    frames: HashMap<u32, u64>,
    /// The database's size in pages after the last commit.
    pages: Option<u32>,
}

/// An open database file.
pub struct Db {
    file: Box<dyn ReadAt>,
    wal: Option<Wal>,
    page_size: u32,
    usable: u32,
    pages: u64,
    visited: u32,
    /// The change counter at open (header offset 24); `None` in WAL mode, which does not keep it.
    counter: Option<u32>,
}

impl Db {
    /// Reads the header of `file` (the magic string, the page size, the reserved bytes per page,
    /// the text encoding: UTF-8 only) and, for a database in WAL mode, the committed frames of its
    /// write-ahead log `wal` (`<file>-wal`; `None` when there is none). A log that does not belong
    /// to the file (another page size, a bad header checksum) is ignored, as SQLite would.
    pub fn open(mut file: Box<dyn ReadAt>, wal: Option<Box<dyn ReadAt>>) -> io::Result<Db> {
        let mut header = [0u8; 100];
        if file.size() < 100 {
            return Err(malformed("too short for a database"));
        }
        file.read_exact_at(0, &mut header)?;
        if &header[..16] != MAGIC {
            return Err(malformed("not an SQLite database"));
        }
        let raw = u32::from(u16::from_be_bytes([header[16], header[17]]));
        let page_size = if raw == 1 { 65_536 } else { raw };
        if !(512..=65_536).contains(&page_size) || !page_size.is_power_of_two() {
            return Err(malformed("bad page size"));
        }
        let reserved = u32::from(header[20]);
        let usable = page_size
            .checked_sub(reserved)
            .filter(|u| *u >= 480)
            .ok_or_else(|| malformed("bad reserved space"))?;
        let encoding = u32::from_be_bytes([header[56], header[57], header[58], header[59]]);
        if encoding != 0 && encoding != 1 {
            return Err(malformed("not a UTF-8 database"));
        }
        let wal_mode = header[18] == 2 || header[19] == 2;
        let wal = match (wal_mode, wal) {
            (true, Some(log)) => read_wal(log, page_size)?,
            _ => None,
        };
        let pages = wal
            .as_ref()
            .and_then(|w| w.pages)
            .map_or(file.size() / u64::from(page_size), u64::from);
        let counter = (!wal_mode)
            .then(|| u32::from_be_bytes([header[24], header[25], header[26], header[27]]));
        Ok(Db {
            file,
            wal,
            page_size,
            usable,
            pages,
            visited: 0,
            counter,
        })
    }

    /// Whether another process wrote the file since [`Db::open`] (rollback mode: its change
    /// counter moved), so what was read may be torn. Always `false` in WAL mode, where the log's
    /// checksums guard what is read.
    pub fn changed(&mut self) -> io::Result<bool> {
        let Some(before) = self.counter else {
            return Ok(false);
        };
        let mut now = [0u8; 4];
        self.file.read_exact_at(24, &mut now)?;
        Ok(u32::from_be_bytes(now) != before)
    }

    /// Page `number` (1-based), whole: the newest committed copy in the log, else the file's.
    fn page(&mut self, number: u32) -> io::Result<Vec<u8>> {
        if number == 0 || u64::from(number) > self.pages {
            return Err(malformed("page number out of range"));
        }
        self.visited += 1;
        if self.visited > MAX_PAGES {
            return Err(malformed("too many pages"));
        }
        let mut page = vec![0u8; self.page_size as usize];
        if let Some(wal) = &mut self.wal
            && let Some(&offset) = wal.frames.get(&number)
        {
            wal.file.read_exact_at(offset, &mut page)?;
            return Ok(page);
        }
        let offset = u64::from(number - 1) * u64::from(self.page_size);
        self.file.read_exact_at(offset, &mut page)?;
        Ok(page)
    }

    /// The root page of table `name`, from the schema table on page 1; `None` when there is no
    /// such table.
    pub fn table_root(&mut self, name: &str) -> io::Result<Option<u32>> {
        // sqlite_schema(type, name, tbl_name, rootpage, sql): the first two decide, the fourth is
        // the answer, the SQL is never read.
        let row = self.walk(1, 2, 4, &mut |key| {
            key.first().and_then(Column::bytes) == Some(b"table".as_slice())
                && key.get(1).and_then(Column::bytes) == Some(name.as_bytes())
        })?;
        Ok(match row.as_ref().and_then(|r| r.get(3)) {
            Some(Column::Integer(page)) => u32::try_from(*page).ok(),
            _ => None,
        })
    }

    /// The second column of the first row of the table at `root` whose first column is the text
    /// `key` (`ItemTable(key, value)`); `None` when there is no such row.
    pub fn lookup(&mut self, root: u32, key: &str) -> io::Result<Option<Column>> {
        let row = self.walk(root, 1, 2, &mut |first| {
            first.first().and_then(Column::bytes) == Some(key.as_bytes())
        })?;
        Ok(row.map(|mut r| {
            let value = r
                .get_mut(1)
                .map_or(Column::Null, |c| std::mem::replace(c, Column::Null));
            for column in &mut r {
                column.wipe();
            }
            value
        }))
    }

    /// The first row of the table b-tree at `root`, in key order, whose first `key_columns`
    /// columns `matches`, with its first `columns` columns. Other rows are decoded only as far
    /// as `key_columns` from the part of the cell on its page, and wiped.
    fn walk(
        &mut self,
        root: u32,
        key_columns: usize,
        columns: usize,
        matches: &mut dyn FnMut(&[Column]) -> bool,
    ) -> io::Result<Option<Vec<Column>>> {
        let mut stack = vec![root];
        let mut queued = HashSet::from([root]);
        while let Some(number) = stack.pop() {
            let page = self.page(number)?;
            // Page 1 starts with the 100-byte file header.
            let base = if number == 1 { 100 } else { 0 };
            let kind = *page.get(base).ok_or_else(|| malformed("short page"))?;
            let cells = usize::from(u16::from_be_bytes([
                *page.get(base + 3).ok_or_else(|| malformed("short page"))?,
                *page.get(base + 4).ok_or_else(|| malformed("short page"))?,
            ]));
            let header = match kind {
                TABLE_LEAF => 8,
                TABLE_INTERIOR => 12,
                _ => return Err(malformed("not a table b-tree page")),
            };
            let pointer = |i: usize| -> io::Result<usize> {
                let at = base + header + 2 * i;
                let hi = *page.get(at).ok_or_else(|| malformed("cell pointer"))?;
                let lo = *page.get(at + 1).ok_or_else(|| malformed("cell pointer"))?;
                Ok(usize::from(u16::from_be_bytes([hi, lo])))
            };
            if kind == TABLE_INTERIOR {
                // Children in key order: the cells' left children, then the right-most one. The
                // stack pops in reverse, so the right-most goes in first. A page has one parent:
                // a page met twice means a damaged file.
                let mut children = Vec::with_capacity(cells + 1);
                for i in 0..cells {
                    children.push(read_u32(&page, pointer(i)?)?);
                }
                children.push(read_u32(&page, base + 8)?);
                for child in children.into_iter().rev() {
                    if !queued.insert(child) || queued.len() > MAX_PAGES as usize {
                        return Err(malformed("a page is reached twice"));
                    }
                    stack.push(child);
                }
                continue;
            }
            for i in 0..cells {
                if let Some(row) =
                    self.leaf_row(&page, pointer(i)?, key_columns, columns, matches)?
                {
                    return Ok(Some(row));
                }
            }
        }
        Ok(None)
    }

    /// The row of the table leaf cell at `offset` of `page` when its first `key_columns` columns
    /// `matches`, with its first `columns` columns (overflow pages put back together for it only);
    /// `None` for any other row.
    fn leaf_row(
        &mut self,
        page: &[u8],
        offset: usize,
        key_columns: usize,
        columns: usize,
        matches: &mut dyn FnMut(&[Column]) -> bool,
    ) -> io::Result<Option<Vec<Column>>> {
        let (size, n) = varint(page, offset)?;
        let (_rowid, m) = varint(page, offset + n)?;
        let start = offset + n + m;
        let usable = self.usable as usize;
        let max_local = usable - 35;
        let size_usize = usize::try_from(size).unwrap_or(usize::MAX);
        let local = if size_usize <= max_local {
            size_usize
        } else {
            let min_local = (usable - 12) * 32 / 255 - 23;
            let k = min_local + (size_usize - min_local) % (usable - 4);
            if k <= max_local { k } else { min_local }
        };
        let here = page
            .get(start..start + local)
            .ok_or_else(|| malformed("cell outside its page"))?;
        let whole = local == size_usize;

        // The key, from the part on this page when it is all there.
        let mut key = match record(here, key_columns) {
            Ok(key) => Some(key),
            Err(e) if whole => return Err(e),
            Err(_) => None,
        };
        if let Some(key) = &mut key {
            let wanted = matches(key);
            for column in key.iter_mut() {
                column.wipe();
            }
            if !wanted {
                return Ok(None);
            }
            if whole {
                return record(here, columns).map(Some);
            }
        }

        // The row asked for (or one whose key spills over): put the payload back together.
        if size > MAX_PAYLOAD {
            return Err(malformed("payload too large"));
        }
        let mut payload = Vec::with_capacity(size_usize);
        payload.extend_from_slice(here);
        let mut next = read_u32(page, start + local)?;
        while payload.len() < size_usize {
            if next == 0 {
                return Err(malformed("overflow chain ends early"));
            }
            let overflow = self.page(next)?;
            next = read_u32(&overflow, 0)?;
            let take = (size_usize - payload.len()).min(usable - 4);
            payload.extend_from_slice(
                overflow
                    .get(4..4 + take)
                    .ok_or_else(|| malformed("overflow page too short"))?,
            );
        }
        let row = if key.is_some() {
            record(&payload, columns).map(Some)
        } else {
            match record(&payload, key_columns) {
                Ok(mut key) => {
                    let wanted = matches(&key);
                    for column in key.iter_mut() {
                        column.wipe();
                    }
                    if wanted {
                        record(&payload, columns).map(Some)
                    } else {
                        Ok(None)
                    }
                }
                Err(e) => Err(e),
            }
        };
        payload.fill(0);
        std::hint::black_box(&payload);
        row
    }
}

/// The committed frames of the write-ahead log `file` for pages of `page_size` bytes, or `None`
/// when it holds none that belong to the database (empty, another page size, a bad header).
fn read_wal(mut file: Box<dyn ReadAt>, page_size: u32) -> io::Result<Option<Wal>> {
    let size = file.size();
    if size < 32 {
        return Ok(None);
    }
    if size > MAX_WAL {
        return Err(malformed("write-ahead log too large"));
    }
    let mut header = [0u8; 32];
    file.read_exact_at(0, &mut header)?;
    let word = |b: &[u8], at: usize| u32::from_be_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]);
    let big_endian = match word(&header, 0) {
        WAL_MAGIC_LE => false,
        WAL_MAGIC_BE => true,
        _ => return Ok(None),
    };
    if word(&header, 8) != page_size {
        return Ok(None);
    }
    let salts = (word(&header, 16), word(&header, 20));
    let mut sum = checksum((0, 0), &header[..24], big_endian);
    if sum != (word(&header, 24), word(&header, 28)) {
        return Ok(None);
    }
    let frame_len = 24 + u64::from(page_size);
    let mut frames = HashMap::new();
    let mut pending: Vec<(u32, u64)> = Vec::new();
    let mut pages = None;
    let mut frame = [0u8; 24];
    let mut data = vec![0u8; page_size as usize];
    let mut at = 32u64;
    while at + frame_len <= size {
        file.read_exact_at(at, &mut frame)?;
        file.read_exact_at(at + 24, &mut data)?;
        if (word(&frame, 8), word(&frame, 12)) != salts {
            break;
        }
        sum = checksum(sum, &frame[..8], big_endian);
        sum = checksum(sum, &data, big_endian);
        if sum != (word(&frame, 16), word(&frame, 20)) {
            break;
        }
        pending.push((word(&frame, 0), at + 24));
        let commit = word(&frame, 4);
        if commit != 0 {
            frames.extend(pending.drain(..));
            pages = Some(commit);
        }
        at += frame_len;
    }
    data.fill(0);
    std::hint::black_box(&data);
    Ok((!frames.is_empty()).then_some(Wal {
        file,
        frames,
        pages,
    }))
}

/// SQLite's write-ahead log checksum over `data` (a multiple of 8 bytes), continuing `sum`.
fn checksum(sum: (u32, u32), data: &[u8], big_endian: bool) -> (u32, u32) {
    let (mut s0, mut s1) = sum;
    for chunk in data.as_chunks::<8>().0 {
        let word = |b: &[u8]| {
            let b = [b[0], b[1], b[2], b[3]];
            if big_endian {
                u32::from_be_bytes(b)
            } else {
                u32::from_le_bytes(b)
            }
        };
        s0 = s0.wrapping_add(word(&chunk[..4])).wrapping_add(s1);
        s1 = s1.wrapping_add(word(&chunk[4..])).wrapping_add(s0);
    }
    (s0, s1)
}

/// A big-endian `u32` at `at`.
fn read_u32(bytes: &[u8], at: usize) -> io::Result<u32> {
    let b = bytes
        .get(at..at + 4)
        .ok_or_else(|| malformed("short read"))?;
    Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

/// An SQLite varint at `at`: its value and its length (1–9 bytes).
fn varint(bytes: &[u8], at: usize) -> io::Result<(u64, usize)> {
    let mut value: u64 = 0;
    for i in 0..9 {
        let byte = *bytes.get(at + i).ok_or_else(|| malformed("short varint"))?;
        if i == 8 {
            return Ok(((value << 8) | u64::from(byte), 9));
        }
        value = (value << 7) | u64::from(byte & 0x7f);
        if byte & 0x80 == 0 {
            return Ok((value, i + 1));
        }
    }
    Err(malformed("bad varint"))
}

/// The first `max_columns` columns of a record (a header of serial types, then the values).
/// `payload` may be the record's first part: a column that does not fit is an error.
fn record(payload: &[u8], max_columns: usize) -> io::Result<Vec<Column>> {
    let (header_len, n) = varint(payload, 0)?;
    let header_len = usize::try_from(header_len).map_err(|_| malformed("record header"))?;
    if header_len > payload.len() || header_len < n {
        return Err(malformed("record header"));
    }
    let mut types = Vec::with_capacity(max_columns.min(16));
    let mut at = n;
    while at < header_len && types.len() < max_columns {
        let (kind, len) = varint(payload, at)?;
        types.push(kind);
        at += len;
    }
    let mut body = header_len;
    let mut columns = Vec::with_capacity(types.len());
    for kind in types {
        let size = match kind {
            0 | 8 | 9 | 10 | 11 => 0,
            1 => 1,
            2 => 2,
            3 => 3,
            4 => 4,
            5 => 6,
            6 | 7 => 8,
            n => usize::try_from((n - 12) / 2).map_err(|_| malformed("value size"))?,
        };
        let end = body
            .checked_add(size)
            .ok_or_else(|| malformed("value size"))?;
        let bytes = payload
            .get(body..end)
            .ok_or_else(|| malformed("value outside its record"))?;
        body = end;
        let integer = |b: &[u8]| {
            let mut v: i64 = if b.first().is_some_and(|f| f & 0x80 != 0) {
                -1
            } else {
                0
            };
            for byte in b {
                v = (v << 8) | i64::from(*byte);
            }
            v
        };
        columns.push(match kind {
            0 | 10 | 11 => Column::Null,
            1..=6 => Column::Integer(integer(bytes)),
            7 => {
                let mut b = [0u8; 8];
                b.copy_from_slice(bytes);
                Column::Float(f64::from_be_bytes(b))
            }
            8 => Column::Integer(0),
            9 => Column::Integer(1),
            n if n % 2 == 0 => Column::Blob(bytes.to_vec()),
            _ => Column::Text(bytes.to_vec()),
        });
    }
    Ok(columns)
}

#[cfg(test)]
mod tests {
    use super::{checksum, record, varint};

    #[test]
    fn varints_are_big_endian_seven_bits_then_eight() {
        assert_eq!(varint(&[0x05], 0).expect("varint"), (5, 1));
        assert_eq!(varint(&[0x81, 0x00], 0).expect("varint"), (128, 2));
        assert_eq!(varint(&[0xff; 9], 0).expect("varint"), (u64::MAX, 9));
        assert!(varint(&[0x81], 0).is_err(), "cut short");
    }

    #[test]
    fn records_decode_every_serial_type_and_only_the_columns_asked_for() {
        // header: len 6; types: NULL, int8, text "ab" (17), blob 1 byte (14), one (9)
        let payload = [6, 0, 1, 17, 14, 9, 0xfe, b'a', b'b', 0x7f];
        let columns = record(&payload, usize::MAX).expect("record");
        assert_eq!(columns.len(), 5);
        assert_eq!(columns[0], super::Column::Null);
        assert_eq!(columns[1], super::Column::Integer(-2));
        assert_eq!(columns[2], super::Column::Text(b"ab".to_vec()));
        assert_eq!(columns[3], super::Column::Blob(vec![0x7f]));
        assert_eq!(columns[4], super::Column::Integer(1));
        assert_eq!(record(&payload, 2).expect("record").len(), 2);
        // The first part of a record decodes as far as it goes.
        assert_eq!(record(&payload[..7], 2).expect("record").len(), 2);
        assert!(record(&payload[..7], 3).is_err(), "the text is not there");
        assert!(
            record(&[9, 1], 9).is_err(),
            "header longer than the payload"
        );
        assert!(record(&[2, 17], 9).is_err(), "value outside the record");
    }

    #[test]
    fn the_wal_checksum_matches_sqlites_algorithm() {
        // Two big-endian words per step: s0 += x0 + s1; s1 += x1 + s0.
        let data = [0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0, 3, 0, 0, 0, 4];
        assert_eq!(checksum((0, 0), &data, true), (1 + 3 + 3, 3 + 4 + 7));
        let le = [1, 0, 0, 0, 2, 0, 0, 0];
        assert_eq!(checksum((0, 0), &le, false), (1, 3));
    }
}
