//! A minimal, read-only reader of SQLite database files: enough to look up one row of a small
//! table by its first column (Cursor's `ItemTable` in `state.vscdb`), page by page through
//! [`crate::io::Fs::open_at`], so a 1.6 GB file is never read whole or copied and nothing is
//! written next to it (no journal, no lock file).
//!
//! It walks table b-trees only (SQLite's file format, sqlite.org/fileformat2.html): the schema
//! table on page 1 gives the table's root page, and the table's leaf cells give its rows, with
//! payloads spilled to overflow pages put back together. Only UTF-8 databases are read. A
//! write-ahead log next to the file is not read (Cursor keeps its state in rollback mode), so a
//! page another process is rewriting at that moment may read as malformed: the caller treats that
//! as a passing failure.
//!
//! Every length and page number read from the file is checked; a malformed file is an
//! `InvalidData` error, never a panic, and the walk visits at most [`MAX_PAGES`] pages.

use std::io;

use crate::io::ReadAt;

/// The most pages one lookup visits (a loop in a damaged file ends here).
pub const MAX_PAGES: u32 = 100_000;

/// The largest payload put together (a row's value), in bytes.
pub const MAX_PAYLOAD: u64 = 16 << 20;

/// The magic string at the start of every SQLite database file.
const MAGIC: &[u8; 16] = b"SQLite format 3\0";

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

/// An open database file.
pub struct Db {
    file: Box<dyn ReadAt>,
    page_size: u32,
    usable: u32,
    pages: u64,
    visited: u32,
}

impl Db {
    /// Reads the header of `file`: the magic string, the page size, the reserved bytes per page
    /// and the text encoding (UTF-8 only).
    pub fn open(mut file: Box<dyn ReadAt>) -> io::Result<Db> {
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
        let pages = file.size() / u64::from(page_size);
        Ok(Db {
            file,
            page_size,
            usable,
            pages,
            visited: 0,
        })
    }

    /// Page `number` (1-based), whole.
    fn page(&mut self, number: u32) -> io::Result<Vec<u8>> {
        if number == 0 || u64::from(number) > self.pages {
            return Err(malformed("page number out of range"));
        }
        self.visited += 1;
        if self.visited > MAX_PAGES {
            return Err(malformed("too many pages"));
        }
        let mut page = vec![0u8; self.page_size as usize];
        let offset = u64::from(number - 1) * u64::from(self.page_size);
        self.file.read_exact_at(offset, &mut page)?;
        Ok(page)
    }

    /// The root page of table `name`, from the schema table on page 1; `None` when there is no
    /// such table.
    pub fn table_root(&mut self, name: &str) -> io::Result<Option<u32>> {
        let mut root = None;
        self.walk(1, &mut |row| {
            // sqlite_schema(type, name, tbl_name, rootpage, sql)
            let is_table = row.first().and_then(Column::bytes) == Some(b"table".as_slice());
            let named = row.get(1).and_then(Column::bytes) == Some(name.as_bytes());
            if is_table
                && named
                && let Some(Column::Integer(page)) = row.get(3)
            {
                root = u32::try_from(*page).ok();
                return true;
            }
            false
        })?;
        Ok(root)
    }

    /// The second column of the first row of the table at `root` whose first column is the text
    /// `key` (`ItemTable(key, value)`); `None` when there is no such row.
    pub fn lookup(&mut self, root: u32, key: &str) -> io::Result<Option<Column>> {
        let mut found = None;
        self.walk(root, &mut |row| {
            if row.first().and_then(Column::bytes) == Some(key.as_bytes()) {
                found = Some(
                    row.get_mut(1)
                        .map_or(Column::Null, |c| std::mem::replace(c, Column::Null)),
                );
                return true;
            }
            false
        })?;
        Ok(found)
    }

    /// Calls `visit` with every row of the table b-tree at `root`, in order, until it returns
    /// `true`. Rows are wiped after each call (a row may hold a secret).
    fn walk(
        &mut self,
        root: u32,
        visit: &mut dyn FnMut(&mut Vec<Column>) -> bool,
    ) -> io::Result<()> {
        let mut stack = vec![root];
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
                // stack pops in reverse, so push the right-most first.
                let right = read_u32(&page, base + 8)?;
                stack.push(right);
                let mut children = Vec::with_capacity(cells);
                for i in 0..cells {
                    children.push(read_u32(&page, pointer(i)?)?);
                }
                stack.extend(children.into_iter().rev());
                continue;
            }
            for i in 0..cells {
                let mut row = self.leaf_row(&page, pointer(i)?)?;
                let done = visit(&mut row);
                for column in &mut row {
                    column.wipe();
                }
                if done {
                    return Ok(());
                }
            }
        }
        Ok(())
    }

    /// The row of the table leaf cell at `offset` of `page`: its payload (with overflow pages)
    /// decoded as a record.
    fn leaf_row(&mut self, page: &[u8], offset: usize) -> io::Result<Vec<Column>> {
        let (size, n) = varint(page, offset)?;
        let (_rowid, m) = varint(page, offset + n)?;
        if size > MAX_PAYLOAD {
            return Err(malformed("payload too large"));
        }
        let size = usize::try_from(size).map_err(|_| malformed("payload too large"))?;
        let start = offset + n + m;
        let usable = self.usable as usize;
        let max_local = usable - 35;
        let local = if size <= max_local {
            size
        } else {
            let min_local = (usable - 12) * 32 / 255 - 23;
            let k = min_local + (size - min_local) % (usable - 4);
            if k <= max_local { k } else { min_local }
        };
        let mut payload = Vec::with_capacity(size);
        payload.extend_from_slice(
            page.get(start..start + local)
                .ok_or_else(|| malformed("cell outside its page"))?,
        );
        if local < size {
            let mut next = read_u32(page, start + local)?;
            while payload.len() < size {
                if next == 0 {
                    return Err(malformed("overflow chain ends early"));
                }
                let overflow = self.page(next)?;
                next = read_u32(&overflow, 0)?;
                let take = (size - payload.len()).min(usable - 4);
                payload.extend_from_slice(
                    overflow
                        .get(4..4 + take)
                        .ok_or_else(|| malformed("overflow page too short"))?,
                );
            }
        }
        let row = record(&payload);
        payload.fill(0);
        std::hint::black_box(&payload);
        row
    }
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

/// The columns of a record (header of serial types, then the values).
fn record(payload: &[u8]) -> io::Result<Vec<Column>> {
    let (header_len, n) = varint(payload, 0)?;
    let header_len = usize::try_from(header_len).map_err(|_| malformed("record header"))?;
    if header_len > payload.len() || header_len < n {
        return Err(malformed("record header"));
    }
    let mut types = Vec::new();
    let mut at = n;
    while at < header_len {
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
        let bytes = payload
            .get(body..body + size)
            .ok_or_else(|| malformed("value outside its record"))?;
        body += size;
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
    use super::{record, varint};

    #[test]
    fn varints_are_big_endian_seven_bits_then_eight() {
        assert_eq!(varint(&[0x05], 0).expect("varint"), (5, 1));
        assert_eq!(varint(&[0x81, 0x00], 0).expect("varint"), (128, 2));
        assert_eq!(varint(&[0xff; 9], 0).expect("varint"), (u64::MAX, 9));
        assert!(varint(&[0x81], 0).is_err(), "cut short");
    }

    #[test]
    fn records_decode_every_serial_type() {
        // header: len 6; types: NULL, int8, text "ab" (17), blob 1 byte (14), one (9)
        let payload = [6, 0, 1, 17, 14, 9, 0xfe, b'a', b'b', 0x7f];
        let columns = record(&payload).expect("record");
        assert_eq!(columns.len(), 5);
        assert_eq!(columns[0], super::Column::Null);
        assert_eq!(columns[1], super::Column::Integer(-2));
        assert_eq!(columns[2], super::Column::Text(b"ab".to_vec()));
        assert_eq!(columns[3], super::Column::Blob(vec![0x7f]));
        assert_eq!(columns[4], super::Column::Integer(1));
        assert!(record(&[9, 1]).is_err(), "header longer than the payload");
        assert!(record(&[2, 17]).is_err(), "value outside the record");
    }
}
