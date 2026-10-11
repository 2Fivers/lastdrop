//! A read-only reader of Chromium's **block-file** disk cache (the "blockfile" backend), the format
//! of an Electron app's HTTP cache on Windows: `Cache\Cache_Data` with `index`, `data_0` …
//! `data_N` and `f_xxxxxx`. Used for Claude Desktop's cache ([`super::desktop_cache`]).
//!
//! Layouts implemented, from Chromium's `net/disk_cache/blockfile/` (byte offsets, little-endian):
//!
//! - `disk_format.h`, **`IndexHeader`** (368 bytes): `magic` at 0 (`kIndexMagic` = 0xC103CAC3),
//!   `version` at 4 (`kVersion2_0` = 0x20000 … `kVersion3_0` = 0x30000), `num_entries` at 8,
//!   `table_len` at 28 (0 means `kIndexTablesize` = 0x10000), `LruData` at 256. **`Index`**: the
//!   header, then `CacheAddr table[table_len]` at 368; each bucket heads a chain of entries linked
//!   by `EntryStore::next`.
//! - `disk_format.h`, **`EntryStore`** (one 256-byte block, a long key continues into up to three
//!   more): `hash` 0, `next` 4, `rankings_node` 8, `reuse_count` 12, `refetch_count` 16, `state`
//!   20 (`ENTRY_NORMAL` 0, `ENTRY_EVICTED` 1, `ENTRY_DOOMED` 2), `creation_time` 24 (u64),
//!   `key_len` 32, `long_key` 36, `data_size[4]` 40, `data_addr[4]` 56, `flags` 72
//!   (`PARENT_ENTRY` 1, `CHILD_ENTRY` 2), `pad[4]` 76, `self_hash` 92, `key` 96 (NUL-terminated,
//!   at most `kMaxInternalKeyLength` = 927 characters inline, else stored at `long_key`).
//! - `disk_format_base.h`, **`BlockFileHeader`** (`kBlockHeaderSize` = 8192 bytes): `magic` 0
//!   (`kBlockMagic` = 0xC104CAC3), `version` 4, `this_file` 8 (i16), `next_file` 10 (i16),
//!   `entry_size` 12, `num_entries` 16, `max_entries` 20, then counters, hints and the allocation
//!   bitmap. Block `n` of a block file starts at `8192 + n × entry_size`.
//! - `addr.h`, **`Addr`** (`CacheAddr`, u32): bit 31 initialized; bits 28–30 the file type
//!   (`EXTERNAL` 0, `RANKINGS` 1 with 36-byte blocks, `BLOCK_256` 2, `BLOCK_1K` 3, `BLOCK_4K` 4;
//!   5–7 belong to a format version Chromium never shipped). `EXTERNAL`: bits 0–27 number the
//!   separate file `f_%06x`. Block files: bits 26–27 reserved (0), bits 24–25 the number of
//!   contiguous blocks minus one, bits 16–23 the file selector (`data_%d`), bits 0–15 the first
//!   block. `Addr::SanityCheck` / `SanityCheckForEntry` (`addr.cc`) are [`Addr::is_sane`] and
//!   [`Addr::is_sane_for_entry`].
//!
//! An HTTP cache entry keeps the response info (`HttpResponseInfo`'s pickle,
//! [`super::response_info`]) in stream 0 and the body as stored in stream 1.
//!
//! **Read-only and bounded.** Every file is opened with [`Fs::open_at`] (read-only, sharing
//! read, write and delete), so Chromium keeps working; nothing is copied. The hash table is read
//! in chunks of [`TABLE_CHUNK`] buckets, entries one at a time (≤ 1 KiB), streams only up to the
//! caller's cap, and at most [`MAX_VISITS`] entries are visited, so a damaged chain cannot loop.
//! Chromium may be writing while Lastdrop reads: every address, size and offset is checked
//! against the file it points into, and an entry that does not check out is skipped.
//!
//! The **simple** backend (`index-dir\the-real-index` with `<hash>_0` files; Chromium's default
//! on other platforms) is recognised and reported as [`CacheError::SimpleBackend`]; it is not
//! read.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use jiff::Timestamp;

use crate::io::{Fs, ReadAt};

/// `sizeof(IndexHeader)`: the hash table starts here.
pub const INDEX_HEADER_SIZE: u64 = 368;
/// `kIndexMagic`.
pub const INDEX_MAGIC: u32 = 0xC103_CAC3;
/// `kBlockMagic`.
pub const BLOCK_MAGIC: u32 = 0xC104_CAC3;
/// `kBlockHeaderSize`: block 0 of a block file starts here.
pub const BLOCK_HEADER_SIZE: u64 = 8192;
/// `kIndexTablesize`, the table length an `IndexHeader::table_len` of 0 stands for.
pub const DEFAULT_TABLE_LEN: u32 = 0x10000;
/// `sizeof(EntryStore)`, the block size of the entries' block files (`BLOCK_256`).
pub const ENTRY_BLOCK: usize = 256;
/// `offsetof(EntryStore, key)`.
pub const KEY_OFFSET: usize = 96;
/// The first 8 bytes of the simple backend's `index` file (`kSimpleInitialMagicNumber`).
pub const SIMPLE_INITIAL_MAGIC: u64 = 0xfcfb_6d1b_a772_5c30;

/// Largest hash table accepted: Chromium sizes it to the cache (0x10000 by default, 0x20000 for a
/// larger cache); a bigger `table_len` is a damaged header (**not from Chromium**).
pub const MAX_TABLE_LEN: u32 = 1 << 20;
/// Buckets read from the index at a time (16 KiB).
pub const TABLE_CHUNK: u32 = 4096;
/// Most entries visited in one walk; past it the index is treated as damaged (a chain that loops).
pub const MAX_VISITS: u32 = 1 << 18;

/// `ENTRY_NORMAL`.
const ENTRY_NORMAL: i32 = 0;
/// `CHILD_ENTRY`: a sparse child, which holds a range of another entry's data.
const CHILD_ENTRY: u32 = 1 << 1;
/// `base::Time`'s epoch, 1601-01-01 UTC, before the Unix epoch, in microseconds.
const WINDOWS_EPOCH_OFFSET_MICROS: i64 = 11_644_473_600_000_000;

/// Why the cache could not be read. Texts carry no path, key or content.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum CacheError {
    /// No `index` file: no cache at this place.
    #[error("no cache index")]
    Missing,
    /// The cache is in Chromium's simple format.
    #[error("the cache uses Chromium's simple format, which Lastdrop does not read")]
    SimpleBackend,
    /// A magic number or version this reader does not know.
    #[error("unknown cache format ({0})")]
    Unsupported(&'static str),
    /// Sizes, addresses or headers that do not add up (or a file being rewritten meanwhile).
    #[error("damaged cache ({0})")]
    Corrupt(&'static str),
    /// A stream longer than the caller's cap.
    #[error("a stream is larger than {0} bytes")]
    TooLarge(u64),
    /// A stream whose data Chromium still holds in memory (no address yet).
    #[error("a stream is not on disk yet")]
    NotWritten,
    /// A file could not be read (locked, denied, gone).
    #[error("cannot read the cache ({0:?})")]
    Io(io::ErrorKind),
}

impl From<io::Error> for CacheError {
    fn from(error: io::Error) -> CacheError {
        CacheError::Io(error.kind())
    }
}

/// A `CacheAddr` (module docs, `addr.h`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Addr(pub u32);

impl Addr {
    /// `kInitializedMask`.
    pub fn is_initialized(self) -> bool {
        self.0 & 0x8000_0000 != 0
    }

    /// `file_type()`: bits 28–30.
    pub fn file_type(self) -> u32 {
        (self.0 & 0x7000_0000) >> 28
    }

    /// `is_separate_file()`: an `EXTERNAL` address, an `f_` file.
    pub fn is_separate_file(self) -> bool {
        self.file_type() == 0
    }

    /// `FileNumber()`: the `f_` file's number, or the block file's selector.
    pub fn file_number(self) -> u32 {
        if self.is_separate_file() {
            self.0 & 0x0FFF_FFFF
        } else {
            (self.0 & 0x00FF_0000) >> 16
        }
    }

    /// `num_blocks()`: 1–4 contiguous blocks.
    pub fn num_blocks(self) -> u32 {
        ((self.0 & 0x0300_0000) >> 24) + 1
    }

    /// `start_block()`.
    pub fn start_block(self) -> u32 {
        self.0 & 0xFFFF
    }

    /// `BlockSizeForFileType()` of the types this format version uses; `None` for `EXTERNAL`
    /// and the unused types 5–7.
    pub fn block_size(self) -> Option<u32> {
        match self.file_type() {
            1 => Some(36),
            2 => Some(256),
            3 => Some(1024),
            4 => Some(4096),
            _ => None,
        }
    }

    /// `Addr::SanityCheck`: initialized, a known type, reserved bits clear.
    pub fn is_sane(self) -> bool {
        if !self.is_initialized() || self.file_type() > 4 {
            return false;
        }
        self.is_separate_file() || self.0 & 0x0C00_0000 == 0
    }

    /// `Addr::SanityCheckForEntry`: an entry lives in a `BLOCK_256` file.
    pub fn is_sane_for_entry(self) -> bool {
        self.is_sane() && self.file_type() == 2
    }
}

/// A `base::Time` internal value (microseconds since 1601-01-01 UTC); `None` for 0 (null) and
/// values outside jiff's range.
pub fn chromium_time(micros: i64) -> Option<Timestamp> {
    if micros == 0 {
        return None;
    }
    let unix = micros.checked_sub(WINDOWS_EPOCH_OFFSET_MICROS)?;
    Timestamp::from_microsecond(unix).ok()
}

/// An entry that matched, with what it takes to read its streams (never its key).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// `EntryStore::creation_time`.
    pub created: Option<Timestamp>,
    /// `EntryStore::data_size`.
    data_size: [i32; 4],
    /// `EntryStore::data_addr`.
    data_addr: [u32; 4],
}

impl Entry {
    /// The stored length of stream `index` (0–3); `None` for a negative (damaged) size.
    pub fn stream_len(&self, index: usize) -> Option<u64> {
        u64::try_from(*self.data_size.get(index)?).ok()
    }
}

/// What a walk of the index found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Found {
    /// The matching entries, newest-created last kept when more than the limit matched.
    pub entries: Vec<Entry>,
    /// Entries read.
    pub visited: u32,
    /// Addresses that did not check out (damaged, or being rewritten while read).
    pub skipped: u32,
}

/// One open block file and the part of its header the reader trusts.
struct BlockFile {
    file: Box<dyn ReadAt>,
    block_size: u32,
    max_entries: u32,
}

/// A block-file cache opened for reading (module docs).
pub struct BlockfileCache<'a> {
    fs: &'a dyn Fs,
    dir: PathBuf,
    index: Box<dyn ReadAt>,
    table_len: u32,
    num_entries: u32,
    /// Block files opened so far by selector; `None` for one that is missing or not valid.
    files: BTreeMap<u32, Option<BlockFile>>,
}

/// Whether `dir` holds a cache in the simple format: an `index-dir` folder, or an `index` file
/// that starts with `kSimpleInitialMagicNumber`.
pub fn is_simple_backend(fs: &dyn Fs, dir: &Path) -> bool {
    if fs.metadata(&dir.join("index-dir")).is_ok_and(|m| m.is_dir) {
        return true;
    }
    let mut head = [0u8; 8];
    fs.open_at(&dir.join("index"))
        .and_then(|mut f| f.read_exact_at(0, &mut head))
        .is_ok_and(|()| u64::from_le_bytes(head) == SIMPLE_INITIAL_MAGIC)
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    let mut b = [0u8; 4];
    b.copy_from_slice(&bytes[at..at + 4]);
    u32::from_le_bytes(b)
}

fn i32_at(bytes: &[u8], at: usize) -> i32 {
    let mut b = [0u8; 4];
    b.copy_from_slice(&bytes[at..at + 4]);
    i32::from_le_bytes(b)
}

fn i64_at(bytes: &[u8], at: usize) -> i64 {
    let mut b = [0u8; 8];
    b.copy_from_slice(&bytes[at..at + 8]);
    i64::from_le_bytes(b)
}

/// A read that ran past the end of the file is a damaged (or changing) address, not an I/O
/// failure.
fn eof_as_none(result: io::Result<()>) -> Result<Option<()>, CacheError> {
    match result {
        Ok(()) => Ok(Some(())),
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => Ok(None),
        Err(e) => Err(e.into()),
    }
}

impl<'a> BlockfileCache<'a> {
    /// Opens the cache in `dir` and checks its index header.
    ///
    /// [`CacheError::Missing`] without an `index` file, [`CacheError::SimpleBackend`] for the
    /// simple format, [`CacheError::Unsupported`] for another magic number or a major version
    /// other than 2 or 3, [`CacheError::Corrupt`] when the file is shorter than its table.
    pub fn open(fs: &'a dyn Fs, dir: &Path) -> Result<BlockfileCache<'a>, CacheError> {
        let mut index = match fs.open_at(&dir.join("index")) {
            Ok(index) => index,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Err(if is_simple_backend(fs, dir) {
                    CacheError::SimpleBackend
                } else {
                    CacheError::Missing
                });
            }
            Err(e) => return Err(e.into()),
        };
        if index.size() < 8 {
            return Err(CacheError::Corrupt("index shorter than its header"));
        }
        let mut head = [0u8; 32];
        let mut magic = [0u8; 8];
        index.read_exact_at(0, &mut magic)?;
        if u64::from_le_bytes(magic) == SIMPLE_INITIAL_MAGIC
            || fs.metadata(&dir.join("index-dir")).is_ok_and(|m| m.is_dir)
        {
            return Err(CacheError::SimpleBackend);
        }
        if u32_at(&magic, 0) != INDEX_MAGIC {
            return Err(CacheError::Unsupported("index magic"));
        }
        if index.size() < INDEX_HEADER_SIZE {
            return Err(CacheError::Corrupt("index shorter than its header"));
        }
        index.read_exact_at(0, &mut head)?;
        if !(2..=3).contains(&(u32_at(&head, 4) >> 16)) {
            return Err(CacheError::Unsupported("index version"));
        }
        let table_len = match i32_at(&head, 28) {
            0 => DEFAULT_TABLE_LEN,
            len => u32::try_from(len)
                .ok()
                .filter(|len| *len <= MAX_TABLE_LEN)
                .ok_or(CacheError::Corrupt("index table length"))?,
        };
        if index.size() < INDEX_HEADER_SIZE + 4 * u64::from(table_len) {
            return Err(CacheError::Corrupt("index shorter than its table"));
        }
        Ok(BlockfileCache {
            fs,
            dir: dir.to_path_buf(),
            index,
            table_len,
            num_entries: u32::try_from(i32_at(&head, 8)).unwrap_or(0),
            files: BTreeMap::new(),
        })
    }

    /// `IndexHeader::num_entries`: how many entries the cache says it holds.
    pub fn num_entries(&self) -> u32 {
        self.num_entries
    }

    /// The block file `data_<selector>` holding blocks of `block_size`, opened once and checked
    /// (`BlockFileHeader`: magic, major version 2 or 3, `this_file`, `entry_size`); `None` when it
    /// is missing or does not check out.
    fn block_file(
        &mut self,
        selector: u32,
        block_size: u32,
    ) -> Result<Option<&mut BlockFile>, CacheError> {
        if !self.files.contains_key(&selector) {
            let opened = self.open_block_file(selector)?;
            self.files.insert(selector, opened);
        }
        Ok(self
            .files
            .get_mut(&selector)
            .and_then(Option::as_mut)
            .filter(|f| f.block_size == block_size))
    }

    fn open_block_file(&self, selector: u32) -> Result<Option<BlockFile>, CacheError> {
        let path = self.dir.join(format!("data_{selector}"));
        let mut file = match self.fs.open_at(&path) {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        if file.size() < BLOCK_HEADER_SIZE {
            return Ok(None);
        }
        let mut head = [0u8; 24];
        file.read_exact_at(0, &mut head)?;
        let this_file = i16::from_le_bytes([head[8], head[9]]);
        let valid = u32_at(&head, 0) == BLOCK_MAGIC
            && (2..=3).contains(&(u32_at(&head, 4) >> 16))
            && u32::try_from(this_file).ok() == Some(selector);
        let (Ok(block_size), Ok(max_entries)) = (
            u32::try_from(i32_at(&head, 12)),
            u32::try_from(i32_at(&head, 20)),
        ) else {
            return Ok(None);
        };
        Ok(valid.then_some(BlockFile {
            file,
            block_size,
            max_entries,
        }))
    }

    /// Reads `buf.len()` bytes of the blocks at `addr` (a block-file address); `None` when the
    /// address points outside its file or the file does not check out.
    fn read_blocks(&mut self, addr: Addr, buf: &mut [u8]) -> Result<Option<()>, CacheError> {
        let Some(block_size) = addr.block_size() else {
            return Ok(None);
        };
        let Some(file) = self.block_file(addr.file_number(), block_size)? else {
            return Ok(None);
        };
        let blocks = addr.num_blocks();
        let span = u64::from(blocks) * u64::from(block_size);
        let offset = BLOCK_HEADER_SIZE + u64::from(addr.start_block()) * u64::from(block_size);
        if addr.start_block() + blocks > file.max_entries
            || buf.len() as u64 > span
            || offset + buf.len() as u64 > file.file.size()
        {
            return Ok(None);
        }
        eof_as_none(file.file.read_exact_at(offset, buf))
    }

    /// Walks every chain of the index and returns the entries in `ENTRY_NORMAL` state, not sparse
    /// children, whose inline key `matches` (keys stored apart, longer than 927 characters, are
    /// not read). At most `limit` entries are kept: when more match, those created last.
    ///
    /// `matches` sees each key in place, for the length of the call; keys are never kept.
    /// [`CacheError::Corrupt`] when the walk passes [`MAX_VISITS`] entries.
    pub fn find(
        &mut self,
        matches: &mut dyn FnMut(&[u8]) -> bool,
        limit: usize,
    ) -> Result<Found, CacheError> {
        let mut found = Found::default();
        let mut table = vec![0u8; TABLE_CHUNK as usize * 4];
        let mut entry = [0u8; 4 * ENTRY_BLOCK];
        let mut start = 0;
        while start < self.table_len {
            let n = TABLE_CHUNK.min(self.table_len - start) as usize;
            let chunk = &mut table[..n * 4];
            self.index
                .read_exact_at(INDEX_HEADER_SIZE + u64::from(start) * 4, chunk)?;
            for bucket in chunk.as_chunks::<4>().0 {
                let mut addr = Addr(u32::from_le_bytes(*bucket));
                while addr.0 != 0 {
                    found.visited += 1;
                    if found.visited > MAX_VISITS {
                        return Err(CacheError::Corrupt("index chains do not end"));
                    }
                    if !addr.is_sane_for_entry() {
                        found.skipped += 1;
                        break;
                    }
                    let len = addr.num_blocks() as usize * ENTRY_BLOCK;
                    let store = &mut entry[..len];
                    if self.read_blocks(addr, store)?.is_none() {
                        found.skipped += 1;
                        break;
                    }
                    if let Some(hit) = Self::matching(store, matches) {
                        keep(&mut found.entries, hit, limit);
                    }
                    addr = Addr(u32_at(store, 4));
                }
            }
            start += n as u32;
        }
        Ok(found)
    }

    /// The entry in `store` (one to four `EntryStore` blocks) when it is live and its inline key
    /// matches.
    fn matching(store: &[u8], matches: &mut dyn FnMut(&[u8]) -> bool) -> Option<Entry> {
        let state = i32_at(store, 20);
        let flags = u32_at(store, 72);
        let long_key = Addr(u32_at(store, 36));
        if state != ENTRY_NORMAL || flags & CHILD_ENTRY != 0 || long_key.is_initialized() {
            return None;
        }
        let key_len = usize::try_from(i32_at(store, 32)).ok()?;
        // NUL-terminated within the entry's blocks.
        if key_len == 0 || KEY_OFFSET + key_len >= store.len() {
            return None;
        }
        if !matches(&store[KEY_OFFSET..KEY_OFFSET + key_len]) {
            return None;
        }
        let mut data_size = [0i32; 4];
        let mut data_addr = [0u32; 4];
        for i in 0..4 {
            data_size[i] = i32_at(store, 40 + 4 * i);
            data_addr[i] = u32_at(store, 56 + 4 * i);
        }
        Some(Entry {
            created: chromium_time(i64_at(store, 24)),
            data_size,
            data_addr,
        })
    }

    /// Reads stream `index` (0–3) of `entry`, at most `max_len` bytes: from its blocks in a block
    /// file, or from the start of its `f_` file.
    ///
    /// [`CacheError::TooLarge`] past `max_len`; [`CacheError::NotWritten`] when the stream has a
    /// size but no address yet; [`CacheError::Corrupt`] when the address or size does not fit the
    /// file it points into.
    pub fn read_stream(
        &mut self,
        entry: &Entry,
        index: usize,
        max_len: u64,
    ) -> Result<Vec<u8>, CacheError> {
        let len = entry
            .stream_len(index)
            .ok_or(CacheError::Corrupt("negative stream size"))?;
        if len == 0 {
            return Ok(Vec::new());
        }
        if len > max_len {
            return Err(CacheError::TooLarge(max_len));
        }
        let addr = Addr(entry.data_addr[index]);
        if !addr.is_initialized() {
            return Err(CacheError::NotWritten);
        }
        if !addr.is_sane() {
            return Err(CacheError::Corrupt("stream address"));
        }
        let mut buf = vec![0u8; usize::try_from(len).map_err(|_| CacheError::TooLarge(max_len))?];
        if addr.is_separate_file() {
            let path = self.dir.join(format!("f_{:06x}", addr.file_number()));
            let mut file = match self.fs.open_at(&path) {
                Ok(file) => file,
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    return Err(CacheError::Corrupt("a stream's file is missing"));
                }
                Err(e) => return Err(e.into()),
            };
            if file.size() < len {
                return Err(CacheError::Corrupt(
                    "a stream's file is shorter than the stream",
                ));
            }
            eof_as_none(file.read_exact_at(0, &mut buf))?.ok_or(CacheError::Corrupt(
                "a stream's file is shorter than the stream",
            ))?;
        } else {
            self.read_blocks(addr, &mut buf)?
                .ok_or(CacheError::Corrupt("a stream's blocks are out of place"))?;
        }
        Ok(buf)
    }
}

/// Adds `hit` to `entries`, keeping at most `limit`: when full, it replaces the entry created
/// first if `hit` was created later.
fn keep(entries: &mut Vec<Entry>, hit: Entry, limit: usize) {
    if entries.len() < limit {
        entries.push(hit);
        return;
    }
    if let Some((oldest, _)) = entries
        .iter()
        .enumerate()
        .min_by_key(|(_, e)| e.created)
        .filter(|(_, e)| e.created < hit.created)
    {
        entries[oldest] = hit;
    }
}
