//! Game data access: the JCL archive format (wcgsl/jclib.c) plus optional
//! loose-file overrides, which the original also honoured (files on disk took
//! precedence over the archive) and which double as a hook for replacement
//! assets.

use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt;

const JCL_MAGIC: u32 = 0xdf73_b489;

#[derive(Debug)]
pub enum DataError {
    NotJcl,
    Truncated,
    Missing(String),
    Format(String, &'static str),
}

impl fmt::Display for DataError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            DataError::NotJcl => write!(f, "not a Speed Haste data file (bad JCL signature)"),
            DataError::Truncated => write!(f, "data file is truncated"),
            DataError::Missing(n) => write!(f, "missing data file entry {n}"),
            DataError::Format(n, why) => write!(f, "bad data in {n}: {why}"),
        }
    }
}

impl std::error::Error for DataError {}

pub type Result<T> = std::result::Result<T, DataError>;

/// The shareware release's SPEEDH.JCL (freely copyable, as its own
/// startup banner said), built in for when no registered copy is given.
static SHAREWARE: &[u8] = include_bytes!("../../../shareware/SPEEDH.JCL");

/// One opened JCL archive.
pub struct Jcl {
    bytes: Cow<'static, [u8]>,
    entries: HashMap<String, (usize, usize)>,
}

impl Jcl {
    /// The built-in shareware data.
    pub fn shareware() -> Jcl {
        Jcl::parse(SHAREWARE).expect("built-in shareware SPEEDH.JCL")
    }

    pub fn parse(bytes: impl Into<Cow<'static, [u8]>>) -> Result<Jcl> {
        let bytes = bytes.into();
        let len = bytes.len();
        if len <= 16 {
            return Err(DataError::NotJcl);
        }
        let u32_at = |o: usize| u32::from_le_bytes(bytes[o..o + 4].try_into().unwrap());
        if u32_at(len - 16) != JCL_MAGIC {
            return Err(DataError::NotJcl);
        }
        let nfiles = u32_at(len - 12) as usize;
        let dirsize = u32_at(len - 4) as usize;
        let dir = len.checked_sub(dirsize).ok_or(DataError::Truncated)?;
        if dir + nfiles * 32 > len {
            return Err(DataError::Truncated);
        }
        let mut entries = HashMap::with_capacity(nfiles);
        for i in 0..nfiles {
            let e = &bytes[dir + i * 32..dir + i * 32 + 32];
            let name_len = e[..24].iter().position(|&c| c == 0).unwrap_or(24);
            let name = String::from_utf8_lossy(&e[..name_len]).to_ascii_uppercase();
            let back = u32::from_le_bytes(e[24..28].try_into().unwrap()) as usize;
            let size = u32::from_le_bytes(e[28..32].try_into().unwrap()) as usize;
            let off = dir.checked_sub(back).ok_or(DataError::Truncated)?;
            if off + size > len {
                return Err(DataError::Truncated);
            }
            entries.insert(name, (off, size));
        }
        Ok(Jcl { bytes, entries })
    }

    pub fn get(&self, name: &str) -> Option<&[u8]> {
        let &(off, size) = self.entries.get(&name.to_ascii_uppercase())?;
        Some(&self.bytes[off..off + size])
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(String::as_str)
    }
}

/// All game data: archives (searched last-added first, like the original's
/// `-file` option) and loose override files (searched before everything).
#[derive(Default)]
pub struct Data {
    archives: Vec<Jcl>,
    overrides: HashMap<String, Vec<u8>>,
}

impl Data {
    pub fn new(main: Jcl) -> Data {
        Data { archives: vec![main], overrides: HashMap::new() }
    }

    pub fn add_archive(&mut self, jcl: Jcl) {
        self.archives.push(jcl);
    }

    pub fn add_override(&mut self, name: &str, bytes: Vec<u8>) {
        self.overrides.insert(name.to_ascii_uppercase(), bytes);
    }

    pub fn get(&self, name: &str) -> Option<&[u8]> {
        if let Some(b) = self.overrides.get(&name.to_ascii_uppercase()) {
            return Some(b);
        }
        self.archives.iter().rev().find_map(|a| a.get(name))
    }

    /// All entry names (uppercase) across archives and overrides.
    pub fn names(&self) -> Vec<String> {
        let mut v: Vec<String> = self.archives.iter().flat_map(|a| a.names().map(str::to_string)).collect();
        v.extend(self.overrides.keys().cloned());
        v.sort();
        v.dedup();
        v
    }

    pub fn exists(&self, name: &str) -> bool {
        self.get(name).is_some()
    }

    pub fn load(&self, name: &str) -> Result<&[u8]> {
        self.get(name).ok_or_else(|| DataError::Missing(name.to_string()))
    }

    /// True when the registered (full) game data is present, which the
    /// original checked by looking for the last circuit.
    pub fn is_registered(&self) -> bool {
        self.exists("map07.sec")
    }
}

/// Little-endian cursor over a byte slice, used by the format parsers.
pub struct Reader<'a> {
    pub buf: &'a [u8],
    pub pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Reader { buf, pos: 0 }
    }

    pub fn bytes(&mut self, n: usize) -> Option<&'a [u8]> {
        let b = self.buf.get(self.pos..self.pos + n)?;
        self.pos += n;
        Some(b)
    }

    pub fn u8(&mut self) -> Option<u8> {
        Some(self.bytes(1)?[0])
    }

    pub fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.bytes(2)?.try_into().ok()?))
    }

    pub fn i16(&mut self) -> Option<i16> {
        Some(self.u16()? as i16)
    }

    pub fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.bytes(4)?.try_into().ok()?))
    }

    pub fn i32(&mut self) -> Option<i32> {
        Some(self.u32()? as i32)
    }
}
