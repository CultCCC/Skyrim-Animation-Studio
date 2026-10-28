//! NIF header parsing/serialization for version 20.2.0.7 (Skyrim SE / LE).

use crate::error::{Error, Result};
use crate::stream::{Reader, Writer};

/// 20.2.0.7 packed as Gamebryo does it.
pub const VERSION_20_2_0_7: u32 = 0x1402_0007;
/// User version used by all Bethesda titles in this era.
pub const BS_USER_VERSION: u32 = 12;

/// Bethesda stream version distinguishes SE from LE at equal file version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BsStreamVersion {
    /// Skyrim Legendary Edition (32-bit), stream 83.
    SkyrimLE,
    /// Skyrim Special Edition (64-bit), stream 100.
    SkyrimSE,
    /// Anything else Bethesda-flavored we can still carry through untouched.
    Other(u32),
}

impl BsStreamVersion {
    pub fn from_raw(v: u32) -> Self {
        match v {
            83 => BsStreamVersion::SkyrimLE,
            100 => BsStreamVersion::SkyrimSE,
            other => BsStreamVersion::Other(other),
        }
    }

    pub fn raw(self) -> u32 {
        match self {
            BsStreamVersion::SkyrimLE => 83,
            BsStreamVersion::SkyrimSE => 100,
            BsStreamVersion::Other(v) => v,
        }
    }
}

/// Parsed NIF header. Field order mirrors the on-disk layout.
#[derive(Debug, Clone)]
pub struct Header {
    /// Original first line, e.g. `Gamebryo File Format, Version 20.2.0.7`.
    pub header_line: String,
    pub version: u32,
    /// 1 = little endian. Big endian is rejected at parse time.
    pub endian: u8,
    pub user_version: u32,
    pub num_blocks: u32,
    pub bs_version: BsStreamVersion,
    pub author: String,
    pub process_script: String,
    pub export_script: String,
    /// Distinct block type names, e.g. `NiNode`, `NiParticleSystem`.
    pub block_types: Vec<String>,
    /// One entry per block: index into `block_types`.
    pub block_type_index: Vec<u16>,
    /// One entry per block: byte size of the block payload.
    pub block_sizes: Vec<u32>,
    /// Shared string table used by `NiObjectNET::Name` and friends.
    pub strings: Vec<String>,
    pub max_string_length: u32,
    pub groups: Vec<u32>,
}

impl Header {
    /// Fresh SSE header for a new document.
    pub fn new_sse() -> Self {
        Header {
            header_line: "Gamebryo File Format, Version 20.2.0.7".to_string(),
            version: VERSION_20_2_0_7,
            endian: 1,
            user_version: BS_USER_VERSION,
            num_blocks: 0,
            bs_version: BsStreamVersion::SkyrimSE,
            author: "Aether".to_string(),
            process_script: String::new(),
            export_script: String::new(),
            block_types: Vec::new(),
            block_type_index: Vec::new(),
            block_sizes: Vec::new(),
            strings: Vec::new(),
            max_string_length: 0,
            groups: Vec::new(),
        }
    }

    /// The block type name for block `i`.
    pub fn block_type_name(&self, i: usize) -> Option<&str> {
        let ti = *self.block_type_index.get(i)? as usize;
        self.block_types.get(ti).map(|s| s.as_str())
    }

    /// Resolve a string table index (`0xFFFFFFFF` means "no string").
    pub fn string(&self, idx: u32) -> Option<&str> {
        if idx == u32::MAX {
            None
        } else {
            self.strings.get(idx as usize).map(|s| s.as_str())
        }
    }

    /// Index of `name` in the string table, adding it if missing.
    pub fn intern_string(&mut self, name: &str) -> u32 {
        if let Some(i) = self.strings.iter().position(|s| s == name) {
            return i as u32;
        }
        self.strings.push(name.to_string());
        self.max_string_length = self.max_string_length.max(name.len() as u32);
        (self.strings.len() - 1) as u32
    }

    /// Index of block type `name`, adding it if missing.
    pub fn intern_block_type(&mut self, name: &str) -> u16 {
        if let Some(i) = self.block_types.iter().position(|s| s == name) {
            return i as u16;
        }
        self.block_types.push(name.to_string());
        (self.block_types.len() - 1) as u16
    }

    pub fn parse(r: &mut Reader<'_>) -> Result<Self> {
        let header_line = r.line()?;
        if !header_line.contains("Gamebryo File Format")
            && !header_line.contains("NetImmerse File Format")
        {
            return Err(Error::NotANif);
        }
        let version = r.u32()?;
        if version != VERSION_20_2_0_7 {
            return Err(Error::UnsupportedVersion {
                header_line,
                version,
            });
        }
        let endian = r.u8()?;
        if endian != 1 {
            return Err(Error::BigEndian);
        }
        let user_version = r.u32()?;
        let num_blocks = r.u32()?;

        // BS header condition per nif.xml #BSSTREAMHEADER#: at file
        // version 20.2.0.7 it exists whenever user_version >= 3 — NOT
        // only >= 12. Six vanilla LE files (the farmhouse ferns,
        // testwhite01) ship user version 11 with bs 32, an old Gamebryo
        // export whose author string reads "mcarofano"; reading them
        // with the >= 12 rule landed the type table inside the author
        // string and produced an implausible SizedString length.
        let (bs_version, author, process_script, export_script) = if user_version >= 3 {
            let bs = r.u32()?;
            let author = r.export_string()?;
            let process = r.export_string()?;
            let export = r.export_string()?;
            (BsStreamVersion::from_raw(bs), author, process, export)
        } else {
            (
                BsStreamVersion::Other(0),
                String::new(),
                String::new(),
                String::new(),
            )
        };

        let num_block_types = r.u16()? as usize;
        let mut block_types = Vec::with_capacity(num_block_types);
        for _ in 0..num_block_types {
            block_types.push(r.sized_string()?);
        }

        let mut block_type_index = Vec::with_capacity(num_blocks as usize);
        for _ in 0..num_blocks {
            block_type_index.push(r.u16()?);
        }

        // Block sizes are present for versions >= 20.2.0.5 — always true here.
        let mut block_sizes = Vec::with_capacity(num_blocks as usize);
        for _ in 0..num_blocks {
            block_sizes.push(r.u32()?);
        }

        let num_strings = r.u32()? as usize;
        let max_string_length = r.u32()?;
        let mut strings = Vec::with_capacity(num_strings);
        for _ in 0..num_strings {
            strings.push(r.sized_string()?);
        }

        let num_groups = r.u32()? as usize;
        let mut groups = Vec::with_capacity(num_groups);
        for _ in 0..num_groups {
            groups.push(r.u32()?);
        }

        Ok(Header {
            header_line,
            version,
            endian,
            user_version,
            num_blocks,
            bs_version,
            author,
            process_script,
            export_script,
            block_types,
            block_type_index,
            block_sizes,
            strings,
            max_string_length,
            groups,
        })
    }

    pub fn write(&self, w: &mut Writer) {
        w.line(&self.header_line);
        w.u32(self.version);
        w.u8(self.endian);
        w.u32(self.user_version);
        w.u32(self.num_blocks);
        if self.user_version >= 3 {
            w.u32(self.bs_version.raw());
            w.export_string(&self.author);
            w.export_string(&self.process_script);
            w.export_string(&self.export_script);
        }
        w.u16(self.block_types.len() as u16);
        for t in &self.block_types {
            w.sized_string(t);
        }
        for &i in &self.block_type_index {
            w.u16(i);
        }
        for &s in &self.block_sizes {
            w.u32(s);
        }
        w.u32(self.strings.len() as u32);
        w.u32(self.max_string_length);
        for s in &self.strings {
            w.sized_string(s);
        }
        w.u32(self.groups.len() as u32);
        for &g in &self.groups {
            w.u32(g);
        }
    }
}
