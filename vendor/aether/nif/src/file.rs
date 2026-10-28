//! Whole-file model: header + raw blocks + footer, with lossless round-trip.

use crate::error::{Error, Result};
use crate::header::Header;
use crate::stream::{Reader, Writer};

/// One block: its type name (resolved from the header) plus raw payload bytes.
///
/// Typed decoding happens on demand and re-encodes back into `data`, so a
/// block that is never touched is written back byte-identically.
#[derive(Debug, Clone)]
pub struct Block {
    pub type_name: String,
    pub data: Vec<u8>,
}

/// A parsed NIF file.
#[derive(Debug, Clone)]
pub struct NifFile {
    pub header: Header,
    pub blocks: Vec<Block>,
    /// Root block references from the footer (usually one: block 0).
    pub roots: Vec<i32>,
}

impl NifFile {
    /// Parse a NIF from bytes.
    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut r = Reader::new(data);
        let header = Header::parse(&mut r)?;

        let mut blocks = Vec::with_capacity(header.num_blocks as usize);
        for i in 0..header.num_blocks as usize {
            let size = header.block_sizes[i] as usize;
            let type_name = header
                .block_type_name(i)
                .ok_or(Error::BadBlockIndex(i))?
                .to_string();
            let payload = r.take(size)?;
            blocks.push(Block {
                type_name,
                data: payload.to_vec(),
            });
        }

        // Footer: root list.
        let num_roots = r.u32()? as usize;
        let mut roots = Vec::with_capacity(num_roots);
        for _ in 0..num_roots {
            roots.push(r.i32()?);
        }

        if !r.is_at_end() {
            return Err(Error::Corrupt {
                offset: r.pos(),
                what: format!("{} trailing bytes after footer", r.remaining()),
            });
        }

        Ok(NifFile {
            header,
            blocks,
            roots,
        })
    }

    /// Serialize back to bytes. Block sizes and counts in the header are
    /// recomputed from the actual block payloads.
    pub fn write(&self) -> Vec<u8> {
        let mut header = self.header.clone();
        header.num_blocks = self.blocks.len() as u32;
        header.block_sizes = self.blocks.iter().map(|b| b.data.len() as u32).collect();

        let mut w = Writer::new();
        header.write(&mut w);
        for b in &self.blocks {
            w.bytes(&b.data);
        }
        w.u32(self.roots.len() as u32);
        for &root in &self.roots {
            w.i32(root);
        }
        w.into_bytes()
    }

    /// Convenience: read from a file path.
    pub fn open(path: impl AsRef<std::path::Path>) -> Result<Self> {
        let data = std::fs::read(path)?;
        Self::parse(&data)
    }

    /// Convenience: write to a file path.
    pub fn save(&self, path: impl AsRef<std::path::Path>) -> Result<()> {
        std::fs::write(path, self.write())?;
        Ok(())
    }

    /// Indices of blocks whose type matches `name`.
    pub fn blocks_of_type<'a>(&'a self, name: &'a str) -> impl Iterator<Item = usize> + 'a {
        self.blocks
            .iter()
            .enumerate()
            .filter_map(move |(i, b)| (b.type_name == name).then_some(i))
    }

    /// A short human summary (used by CLI/dev tooling and tests).
    pub fn summary(&self) -> String {
        use std::collections::BTreeMap;
        let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
        for b in &self.blocks {
            *counts.entry(b.type_name.as_str()).or_default() += 1;
        }
        let mut out = format!(
            "{} | bs {} | {} blocks | {} strings\n",
            self.header.header_line,
            self.header.bs_version.raw(),
            self.blocks.len(),
            self.header.strings.len()
        );
        for (name, n) in counts {
            out.push_str(&format!("  {n:>3} x {name}\n"));
        }
        out
    }
}
