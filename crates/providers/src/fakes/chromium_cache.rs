//! Synthetic Chromium block-file HTTP caches for tests: invented entries written into a
//! [`FakeFs`] in Chromium's layout, so `claude::blockfile` and `claude::desktop_cache` can be
//! tested without a real cache (which holds a person's conversations and is never copied).
//!
//! The layout is written out here from Chromium's structs, independently of the reader:
//! `disk_format.h` (`IndexHeader`, `Index`, `EntryStore`), `disk_format_base.h`
//! (`BlockFileHeader`), `addr.h` (`Addr`), `net/http/http_response_info.cc` (the response info
//! pickle) and `net/http/http_cache.cc` (`GenerateCacheKey`). Streams go where Chromium puts them
//! (`Addr::RequiredFileType`): under 1 KiB in `data_1` (256-byte blocks), under 4 KiB in `data_2`,
//! up to 16 KiB in `data_3`, larger in an `f_xxxxxx` file. Entries are hashed into buckets with
//! `base::PersistentHash` ([`super_fast_hash`]), which also gives `EntryStore::hash` and
//! `self_hash`, and chained through `EntryStore::next`.

use std::collections::BTreeMap;
use std::path::Path;

use jiff::Timestamp;

use super::FakeFs;

/// `kIndexMagic`.
const INDEX_MAGIC: u32 = 0xC103_CAC3;
/// `kBlockMagic`.
const BLOCK_MAGIC: u32 = 0xC104_CAC3;
/// `kVersion3_0` (index) and `kBlockVersion2` (block files), what Chromium writes today.
const INDEX_VERSION: u32 = 0x30000;
const BLOCK_VERSION: u32 = 0x20000;
/// `sizeof(IndexHeader)`.
const INDEX_HEADER: usize = 368;
/// `kBlockHeaderSize`.
const BLOCK_HEADER: usize = 8192;
/// `base::Time`'s epoch (1601-01-01) before the Unix epoch, in microseconds.
const WINDOWS_EPOCH_OFFSET_MICROS: i64 = 11_644_473_600_000_000;

/// A `base::Time` internal value.
pub fn chromium_micros(at: Timestamp) -> i64 {
    at.as_microsecond() + WINDOWS_EPOCH_OFFSET_MICROS
}

/// An invented HTTP response as the cache's stream 0 records it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FakeResponse {
    /// The status line (`HTTP/1.1 200`).
    pub status_line: String,
    /// Header lines, in order.
    pub headers: Vec<(String, String)>,
    /// When the request was sent.
    pub request_time: Timestamp,
    /// When the response arrived.
    pub response_time: Timestamp,
    /// `RESPONSE_INFO_TRUNCATED`.
    pub truncated: bool,
    /// Writes `RESPONSE_EXTRA_INFO_HAS_ORIGINAL_RESPONSE_TIME` and this time.
    pub original_response_time: Option<Timestamp>,
    /// Writes `RESPONSE_EXTRA_INFO_HAS_ZSTD_UNCOMPRESSED_BODY_SIZE` and this size, last: the body
    /// on disk is zstd-compressed by the cache (the caller compresses it).
    pub disk_zstd_size: Option<u64>,
    /// The response info version (3).
    pub version: u32,
}

impl FakeResponse {
    /// A `200` with `content-type: application/json`, `content-encoding: <coding>` (none for
    /// `identity`) and an invented cookie header the reader must not keep, received at `at`.
    pub fn ok(at: Timestamp, coding: &str) -> FakeResponse {
        let mut headers = vec![
            ("content-type".to_owned(), "application/json".to_owned()),
            (
                "set-cookie".to_owned(),
                "session=test-cookie-not-real; Secure; HttpOnly".to_owned(),
            ),
        ];
        if coding != "identity" {
            headers.push(("content-encoding".to_owned(), coding.to_owned()));
        }
        FakeResponse {
            status_line: "HTTP/1.1 200".to_owned(),
            headers,
            request_time: at - jiff::SignedDuration::from_millis(180),
            response_time: at,
            truncated: false,
            original_response_time: None,
            disk_zstd_size: None,
            version: 3,
        }
    }

    /// The `HttpResponseInfo` pickle (module docs): flags, extra flags, times, raw headers, the
    /// remote endpoint, ALPN and connection info, the encoded body size, the disk zstd size.
    pub fn pickle(&self) -> Vec<u8> {
        let mut extra = 1u32 << 3; // RESPONSE_EXTRA_INFO_HAS_ENCODED_BODY_SIZE
        if self.original_response_time.is_some() {
            extra |= 1 << 2;
        }
        if self.disk_zstd_size.is_some() {
            extra |= 1 << 4;
        }
        // Version, WAS_ALPN, HAS_ALPN_NEGOTIATED_PROTOCOL, HAS_CONNECTION_INFO, HAS_EXTRA_FLAGS.
        let mut flags = self.version | 1 << 14 | 1 << 17 | 1 << 18 | 1 << 31;
        if self.truncated {
            flags |= 1 << 12;
        }
        let mut p = PickleWriter::default();
        p.int(flags as i32);
        p.int(extra as i32);
        p.int64(chromium_micros(self.request_time));
        p.int64(chromium_micros(self.response_time));
        if let Some(original) = self.original_response_time {
            p.int64(chromium_micros(original));
        }
        let mut raw = self.status_line.clone().into_bytes();
        raw.push(0);
        for (name, value) in &self.headers {
            raw.extend_from_slice(format!("{name}: {value}").as_bytes());
            raw.push(0);
        }
        raw.push(0);
        p.string(&raw);
        p.string(b"192.0.2.10"); // TEST-NET-1, RFC 5737
        p.int(443); // WriteUInt16, padded to 4
        p.string(b"h2");
        p.int(2); // HttpConnectionInfo::kHTTP2
        p.int64(4321); // encoded body size
        if let Some(size) = self.disk_zstd_size {
            p.int64(i64::try_from(size).unwrap_or(i64::MAX));
        }
        p.finish()
    }
}

/// A `base::Pickle` being written: 4-byte aligned values after a `payload_size` header.
#[derive(Default)]
struct PickleWriter(Vec<u8>);

impl PickleWriter {
    fn pad(&mut self) {
        while !self.0.len().is_multiple_of(4) {
            self.0.push(0);
        }
    }

    fn int(&mut self, v: i32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }

    fn int64(&mut self, v: i64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }

    fn string(&mut self, s: &[u8]) {
        self.int(i32::try_from(s.len()).unwrap_or(i32::MAX));
        self.0.extend_from_slice(s);
        self.pad();
    }

    fn finish(self) -> Vec<u8> {
        let mut out = u32::try_from(self.0.len())
            .unwrap_or(u32::MAX)
            .to_le_bytes()
            .to_vec();
        out.extend(self.0);
        out
    }
}

/// One invented cache entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FakeEntry {
    /// The full cache key (`1/0/https://…`).
    pub key: Vec<u8>,
    /// Stream 0 (a [`FakeResponse::pickle`], or anything to test damage).
    pub info: Vec<u8>,
    /// Stream 1, the body as stored.
    pub body: Vec<u8>,
    /// `EntryStore::state` (`ENTRY_NORMAL` 0, evicted 1, doomed 2).
    pub state: i32,
    /// Put the body in an `f_` file whatever its size.
    pub external_body: bool,
    /// Leave stream 1's address uninitialized (data still in Chromium's memory).
    pub body_not_written: bool,
    /// `EntryStore::creation_time`.
    pub created: Timestamp,
    /// `EntryStore::flags` (`PARENT_ENTRY` 1, `CHILD_ENTRY` 2).
    pub flags: u32,
    /// Write `EntryStore::self_hash`, as Chromium does on every store (an entry without one, 0,
    /// is accepted unchecked).
    pub self_hash: bool,
}

impl FakeEntry {
    /// A live entry with `key`, response info `info` and stored body `body`, created at `created`.
    pub fn new(key: &str, info: Vec<u8>, body: Vec<u8>, created: Timestamp) -> FakeEntry {
        FakeEntry {
            key: key.as_bytes().to_vec(),
            info,
            body,
            state: 0,
            external_body: false,
            body_not_written: false,
            created,
            flags: 0,
            self_hash: true,
        }
    }
}

/// A block file being filled: its blocks after the 8 KiB header.
struct BlockFileWriter {
    selector: u32,
    file_type: u32,
    block_size: usize,
    blocks: Vec<u8>,
}

impl BlockFileWriter {
    fn new(selector: u32, file_type: u32, block_size: usize) -> BlockFileWriter {
        BlockFileWriter {
            selector,
            file_type,
            block_size,
            blocks: Vec::new(),
        }
    }

    /// Appends `data` in 1–4 blocks and returns its `CacheAddr`.
    fn add(&mut self, data: &[u8]) -> u32 {
        let count = data.len().div_ceil(self.block_size).clamp(1, 4);
        let start = self.blocks.len() / self.block_size;
        self.blocks.extend_from_slice(data);
        self.blocks.resize((start + count) * self.block_size, 0);
        block_addr(self.file_type, count as u32, self.selector, start as u32)
    }

    /// Writes `data` over the blocks at `addr` (to fill in `next` after placing every entry).
    fn overwrite(&mut self, addr: u32, at: usize, data: &[u8]) {
        let start = (addr & 0xFFFF) as usize * self.block_size + at;
        self.blocks[start..start + data.len()].copy_from_slice(data);
    }

    /// The file: `BlockFileHeader` (magic, version, `this_file`, `next_file` 0, `entry_size`,
    /// `num_entries`, `max_entries` rounded up to 16 blocks), the bitmap left empty, the blocks.
    fn finish(mut self) -> Vec<u8> {
        let used = self.blocks.len() / self.block_size;
        let max = used.next_multiple_of(16).max(16);
        self.blocks.resize(max * self.block_size, 0);
        let mut file = vec![0u8; BLOCK_HEADER];
        file[0..4].copy_from_slice(&BLOCK_MAGIC.to_le_bytes());
        file[4..8].copy_from_slice(&BLOCK_VERSION.to_le_bytes());
        file[8..10].copy_from_slice(&(self.selector as i16).to_le_bytes());
        file[12..16].copy_from_slice(&(self.block_size as i32).to_le_bytes());
        file[16..20].copy_from_slice(&(used as i32).to_le_bytes());
        file[20..24].copy_from_slice(&(max as i32).to_le_bytes());
        file.extend(self.blocks);
        file
    }
}

/// A block-file `CacheAddr`: initialized, type, blocks − 1, selector, start block.
fn block_addr(file_type: u32, blocks: u32, selector: u32, start: u32) -> u32 {
    0x8000_0000 | file_type << 28 | (blocks - 1) << 24 | selector << 16 | start
}

/// `base::PersistentHash`, Paul Hsieh's SuperFastHash as `third_party/superfasthash` writes it
/// (written out here apart from the reader's): `uint32_t hash = len`, two 16-bit little-endian
/// halves per 4 bytes, the tail by its length (a lone byte as `signed char`), then the avalanche.
pub fn super_fast_hash(data: &[u8]) -> u32 {
    if data.is_empty() {
        return 0;
    }
    let half = |i: usize| u32::from(data[i]) | u32::from(data[i + 1]) << 8;
    let mut hash = data.len() as u32;
    let mut i = 0;
    while i + 4 <= data.len() {
        hash = hash.wrapping_add(half(i));
        let tmp = (half(i + 2) << 11) ^ hash;
        hash = (hash << 16) ^ tmp;
        hash = hash.wrapping_add(hash >> 11);
        i += 4;
    }
    match data.len() - i {
        3 => {
            hash = hash.wrapping_add(half(i));
            hash ^= hash << 16;
            hash ^= ((data[i + 2] as i8 as i32) << 18) as u32;
            hash = hash.wrapping_add(hash >> 11);
        }
        2 => {
            hash = hash.wrapping_add(half(i));
            hash ^= hash << 11;
            hash = hash.wrapping_add(hash >> 17);
        }
        1 => {
            hash = hash.wrapping_add(data[i] as i8 as i32 as u32);
            hash ^= hash << 10;
            hash = hash.wrapping_add(hash >> 1);
        }
        _ => {}
    }
    hash ^= hash << 3;
    hash = hash.wrapping_add(hash >> 5);
    hash ^= hash << 4;
    hash = hash.wrapping_add(hash >> 17);
    hash ^= hash << 25;
    hash = hash.wrapping_add(hash >> 6);
    hash
}

/// An invented block-file cache (module docs).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FakeChromiumCache {
    /// `IndexHeader::table_len` (a power of two).
    pub table_len: u32,
    /// The entries, in insertion order.
    pub entries: Vec<FakeEntry>,
}

impl Default for FakeChromiumCache {
    fn default() -> FakeChromiumCache {
        FakeChromiumCache::new()
    }
}

impl FakeChromiumCache {
    /// An empty cache with a 256-bucket table.
    pub fn new() -> FakeChromiumCache {
        FakeChromiumCache {
            table_len: 256,
            entries: Vec::new(),
        }
    }

    /// Adds an entry.
    pub fn with(mut self, entry: FakeEntry) -> FakeChromiumCache {
        self.entries.push(entry);
        self
    }

    /// The cache's files by name: `index`, `data_0` … `data_3` and any `f_xxxxxx`.
    pub fn files(&self) -> BTreeMap<String, Vec<u8>> {
        // data_0: rankings (36-byte blocks), header only; the reader never needs them.
        let rankings = BlockFileWriter::new(0, 1, 36);
        let mut entries = BlockFileWriter::new(1, 2, 256);
        let mut k1 = BlockFileWriter::new(2, 3, 1024);
        let mut k4 = BlockFileWriter::new(3, 4, 4096);
        let mut external: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        let mut table = vec![0u32; self.table_len as usize];
        // The last entry of each bucket's chain.
        let mut tails: BTreeMap<usize, u32> = BTreeMap::new();
        // Entries that get a `self_hash` once every `next` is in place.
        let mut hashed: Vec<u32> = Vec::new();

        for entry in &self.entries {
            let mut stream = |data: &[u8], force_external: bool, entries: &mut BlockFileWriter| {
                if data.is_empty() {
                    return 0;
                }
                if force_external || data.len() > 4 * 4096 {
                    let number = external.len() as u32 + 1;
                    external.insert(format!("f_{number:06x}"), data.to_vec());
                    0x8000_0000 | number
                } else if data.len() < 1024 {
                    entries.add(data)
                } else if data.len() < 4096 {
                    k1.add(data)
                } else {
                    k4.add(data)
                }
            };
            let info_addr = stream(&entry.info, false, &mut entries);
            let body_addr = if entry.body_not_written {
                0
            } else {
                stream(&entry.body, entry.external_body, &mut entries)
            };

            let hash = super_fast_hash(&entry.key);
            let mut store = vec![0u8; 96];
            store[0..4].copy_from_slice(&hash.to_le_bytes());
            store[20..24].copy_from_slice(&entry.state.to_le_bytes());
            store[24..32].copy_from_slice(&chromium_micros(entry.created).to_le_bytes());
            store[32..36].copy_from_slice(&(entry.key.len() as i32).to_le_bytes());
            store[40..44].copy_from_slice(&(entry.info.len() as i32).to_le_bytes());
            store[44..48].copy_from_slice(&(entry.body.len() as i32).to_le_bytes());
            store[56..60].copy_from_slice(&info_addr.to_le_bytes());
            store[60..64].copy_from_slice(&body_addr.to_le_bytes());
            store[72..76].copy_from_slice(&entry.flags.to_le_bytes());
            store.extend_from_slice(&entry.key);
            store.push(0);
            let addr = entries.add(&store);
            if entry.self_hash {
                hashed.push(addr);
            }
            let bucket = (hash & (self.table_len - 1)) as usize;
            match tails.insert(bucket, addr) {
                // `EntryStore::next` of the bucket's last entry.
                Some(previous) => entries.overwrite(previous, 4, &addr.to_le_bytes()),
                None => table[bucket] = addr,
            }
        }
        // `StorageBlock::Store` writes the hash of the bytes before `self_hash` (offset 92) with
        // every store, also when `next` changes.
        for addr in hashed {
            let at = (addr & 0xFFFF) as usize * entries.block_size;
            let self_hash = super_fast_hash(&entries.blocks[at..at + 92]);
            entries.overwrite(addr, 92, &self_hash.to_le_bytes());
        }

        let mut index = vec![0u8; INDEX_HEADER];
        index[0..4].copy_from_slice(&INDEX_MAGIC.to_le_bytes());
        index[4..8].copy_from_slice(&INDEX_VERSION.to_le_bytes());
        index[8..12].copy_from_slice(&(self.entries.len() as i32).to_le_bytes());
        index[28..32].copy_from_slice(&(self.table_len as i32).to_le_bytes());
        for addr in &table {
            index.extend_from_slice(&addr.to_le_bytes());
        }

        let mut files = external;
        files.insert("index".to_owned(), index);
        files.insert("data_0".to_owned(), rankings.finish());
        files.insert("data_1".to_owned(), entries.finish());
        files.insert("data_2".to_owned(), k1.finish());
        files.insert("data_3".to_owned(), k4.finish());
        files
    }

    /// Writes [`FakeChromiumCache::files`] into `dir` of `fs`.
    pub fn write(&self, fs: &FakeFs, dir: &Path, modified: Timestamp) {
        write_files(fs, dir, &self.files(), modified);
    }
}

/// Writes cache files (possibly altered by a test) into `dir` of `fs`.
pub fn write_files(
    fs: &FakeFs,
    dir: &Path,
    files: &BTreeMap<String, Vec<u8>>,
    modified: Timestamp,
) {
    for (name, data) in files {
        fs.add_file(dir.join(name), data.clone(), modified);
    }
}
