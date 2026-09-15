//! Admission checks run before materializing DTOs. No I/O or source execution.
use crate::{Error, Result};
use std::collections::BTreeSet;

pub const FRAME_BYTES: usize = 1_048_576;
pub const PREPARE_BYTES: usize = 4096;
pub const REQUEST_BYTES: usize = 65_536;
pub const RESULT_BYTES: usize = 131_072;
pub const SCALAR_BYTES: usize = 8192;
pub const IDENTIFIER_BYTES: usize = 256;
pub const MAX_DEPTH: usize = 32;
pub const MAX_NODES: usize = 16_384;
pub const MAX_ROWS: usize = 1000;
pub const DEFAULT_ROWS: usize = 100;
pub const MAX_COLUMNS: usize = 128;
pub const MAX_PREDICATES: usize = 64;
pub const MAX_ORDER: usize = 16;
pub const MAX_BINDS: usize = 1024;
pub const MAX_ENTRIES: usize = 1000;
pub const KEYWORD_WINDOW_LINES: usize = 1000;
pub const SCAN_BYTES: usize = 1_048_576;
pub const SCAN_LINES: usize = 16_384;
pub const REGEX_BYTES: usize = 4096;
pub const REGEX_COMPILED_BYTES: usize = 1_048_576;
pub const INVENTORY_RECORDS: usize = 100_000;
pub const INVENTORY_WORK_BYTES: usize = 1_048_576;
pub const MAX_PACKAGES: usize = 500;
pub const MAX_PHP: usize = 32;
pub const IO_SECONDS: u64 = 10;
pub const SOURCE_SECONDS: u64 = 5;
pub const CALL_SECONDS: u64 = 30;
pub const LOCAL_RELEASE_SECONDS: u64 = 60;
pub const DEFAULT_ACTIVE_CALLS: usize = 4;
pub const MAX_ACTIVE_CALLS: usize = 16;
pub const MAX_PRINCIPAL_CALLS: usize = 2;
pub const CODEC_BYTES: usize = 8_388_608;
pub const PROTECTED_BYTES: usize = 1_048_576;

pub fn require(ok: bool) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(Error::InvalidRequest)
    }
}
pub fn cap(size: usize, max: usize) -> Result<()> {
    if size <= max {
        Ok(())
    } else {
        Err(Error::CapExceeded)
    }
}
pub fn identifier(text: &str) -> Result<()> {
    require(!text.is_empty())?;
    cap(text.len(), IDENTIFIER_BYTES)
}
pub fn configured_id(text: &str) -> Result<()> {
    require(
        !text.is_empty()
            && text.len() <= 64
            && text
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'),
    )
}
pub fn text(text: &str) -> Result<()> {
    cap(text.len(), SCALAR_BYTES)
}

/// Counts serialized bytes without allocating the serialized output.
pub fn serialized_size<T: serde::Serialize>(value: &T, max: usize) -> Result<usize> {
    struct Counter {
        count: usize,
        max: usize,
    }
    impl std::io::Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.count = self
                .count
                .checked_add(bytes.len())
                .filter(|n| *n <= self.max)
                .ok_or_else(|| std::io::Error::other("cap_exceeded"))?;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut out = Counter { count: 0, max };
    serde_json::to_writer(&mut out, value).map_err(|_| Error::CapExceeded)?;
    Ok(out.count)
}

/// Validate exact JSON, including duplicate decoded keys, before DTO allocation.
/// The scanner retains only bounded object keys. Number lexemes never visit f64.
pub fn json(bytes: &[u8], max: usize) -> Result<()> {
    cap(bytes.len(), max)?;
    let mut scan = Scanner {
        bytes,
        at: 0,
        nodes: 0,
        scalar_max: SCALAR_BYTES * 2,
    };
    scan.value(1)?;
    scan.space();
    require(scan.at == bytes.len())
}
/// JSON source values use the tighter source-scalar ceiling.
pub fn source_json(bytes: &[u8]) -> Result<()> {
    cap(bytes.len(), RESULT_BYTES)?;
    let mut scan = Scanner {
        bytes,
        at: 0,
        nodes: 0,
        scalar_max: SCALAR_BYTES,
    };
    scan.value(1)?;
    scan.space();
    require(scan.at == bytes.len())
}
struct Scanner<'a> {
    bytes: &'a [u8],
    at: usize,
    nodes: usize,
    scalar_max: usize,
}
impl Scanner<'_> {
    fn space(&mut self) {
        while self
            .bytes
            .get(self.at)
            .is_some_and(|b| matches!(b, b' ' | b'\t' | b'\r' | b'\n'))
        {
            self.at += 1;
        }
    }
    fn node(&mut self) -> Result<()> {
        self.nodes += 1;
        cap(self.nodes, MAX_NODES)
    }
    fn string(&mut self) -> Result<String> {
        let start = self.at;
        require(self.bytes.get(self.at) == Some(&b'"'))?;
        self.at += 1;
        loop {
            let b = *self.bytes.get(self.at).ok_or(Error::InvalidRequest)?;
            self.at += 1;
            if b == b'"' {
                break;
            }
            if b == b'\\' {
                self.at += 1;
            }
            // An escaped scalar is at most six bytes per decoded byte.
            cap(self.at - start, self.scalar_max * 6 + 2)?;
        }
        let s: String = serde_json::from_slice(&self.bytes[start..self.at])
            .map_err(|_| Error::InvalidRequest)?;
        cap(s.len(), self.scalar_max)?;
        Ok(s)
    }
    fn value(&mut self, depth: usize) -> Result<()> {
        cap(depth, MAX_DEPTH)?;
        self.node()?;
        self.space();
        match self
            .bytes
            .get(self.at)
            .copied()
            .ok_or(Error::InvalidRequest)?
        {
            b'"' => {
                self.string()?;
            }
            b'{' | b'[' => {
                let object = self.bytes[self.at] == b'{';
                let end = if object { b'}' } else { b']' };
                self.at += 1;
                self.space();
                let mut keys = BTreeSet::new();
                if self.bytes.get(self.at) == Some(&end) {
                    self.at += 1;
                    return Ok(());
                }
                loop {
                    if object {
                        self.node()?;
                        self.space();
                        require(keys.insert(self.string()?))?;
                        self.space();
                        require(self.bytes.get(self.at) == Some(&b':'))?;
                        self.at += 1;
                    }
                    self.value(depth + 1)?;
                    self.space();
                    match self.bytes.get(self.at) {
                        Some(b',') => self.at += 1,
                        Some(b) if *b == end => {
                            self.at += 1;
                            break;
                        }
                        _ => return Err(Error::InvalidRequest),
                    }
                }
            }
            _ => {
                let start = self.at;
                while self.bytes.get(self.at).is_some_and(|b| {
                    !matches!(b, b' ' | b'\t' | b'\r' | b'\n' | b',' | b']' | b'}')
                }) {
                    self.at += 1;
                }
                let token = &self.bytes[start..self.at];
                require(matches!(token, b"null" | b"true" | b"false") || number(token))?;
                cap(token.len(), SCALAR_BYTES)?;
            }
        }
        Ok(())
    }
}
/// JSON number grammar, preserving fractions and arbitrarily large exponents.
pub fn number(s: &[u8]) -> bool {
    let mut i = usize::from(s.first() == Some(&b'-'));
    match s.get(i) {
        Some(b'0') => i += 1,
        Some(b'1'..=b'9') => {
            i += 1;
            while s.get(i).is_some_and(u8::is_ascii_digit) {
                i += 1;
            }
        }
        _ => return false,
    }
    if s.get(i) == Some(&b'.') {
        i += 1;
        let start = i;
        while s.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        if i == start {
            return false;
        }
    }
    if matches!(s.get(i), Some(b'e' | b'E')) {
        i += 1;
        if matches!(s.get(i), Some(b'+' | b'-')) {
            i += 1;
        }
        let start = i;
        while s.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        if i == start {
            return false;
        }
    }
    i == s.len()
}

/// One frame accumulator. The caller owns deadlines and must close on any error.
/// A completed frame cannot accept more bytes, even in the same input chunk.
pub struct FrameBuffer {
    bytes: Vec<u8>,
    max: usize,
    finished: bool,
    failed: bool,
}
impl FrameBuffer {
    pub fn new(max: usize) -> Result<Self> {
        require(max > 0 && max <= FRAME_BYTES)?;
        Ok(Self {
            bytes: Vec::new(),
            max,
            finished: false,
            failed: false,
        })
    }
    pub fn push(&mut self, chunk: &[u8]) -> Result<bool> {
        require(!self.finished && !self.failed)?;
        self.failed = true;
        let size = self
            .bytes
            .len()
            .checked_add(chunk.len())
            .ok_or(Error::CapExceeded)?;
        cap(size, self.max)?;
        if let Some(i) = chunk.iter().position(|b| *b == b'\n') {
            require(i + 1 == chunk.len())?;
            self.finished = true;
        }
        self.bytes
            .try_reserve_exact(chunk.len())
            .map_err(|_| Error::CapExceeded)?;
        self.bytes.extend_from_slice(chunk);
        self.failed = false;
        Ok(self.finished)
    }
    pub fn finish(self) -> Result<Vec<u8>> {
        require(self.finished && !self.failed)?;
        Ok(self.bytes)
    }
}
