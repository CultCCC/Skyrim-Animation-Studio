//! Reader for Bethesda's TES4-era BSA archives (versions 103/104/105).
//!
//! Vanilla Skyrim ships its textures inside archives, so an editor that only
//! looks at loose files finds almost nothing on a clean install. This is the
//! other half of the asset pipeline in [`crate::assets`].
//!
//! The format is documented in `docs/knowledge/bsa-format.md`, verified
//! against real archives and the engine's own loader. Two rules from it shape
//! everything here:
//!
//! * **The index is walked sequentially.** The folder record's `name_offset`
//!   is never used for seeking — it needs a correction term, points at the
//!   folder name rather than the file records, and at least one popular
//!   packer is known to corrupt it. Walking gives the same positions with no
//!   arithmetic at all.
//! * **Compression is an XOR, not a flag.** An entry is compressed when the
//!   archive's "compressed by default" flag differs from the entry's bit 30.
//!   Reading bit 30 alone hands out raw LZ4 frames as if they were DDS files
//!   for every entry of an archive packed with the default on.

use crate::inflate;
use crate::lz4;
use std::collections::HashMap;
use std::fmt;
use std::fs::File;
use std::path::{Path, PathBuf};

const TAG_BSA: u32 = 0x0041_5342; // "BSA\0"
const TAG_BTDX: u32 = 0x5844_5442; // "BTDX" — Fallout 4 BA2
const HEADER_SIZE: u64 = 0x24;

// Archive flags.
const FLAG_DIRECTORY_NAMES: u32 = 0x001;
const FLAG_FILE_NAMES: u32 = 0x002;
const FLAG_COMPRESSED_BY_DEFAULT: u32 = 0x004;
const FLAG_XBOX360: u32 = 0x040;
const FLAG_EMBEDDED_NAMES: u32 = 0x100;
const FLAG_XMEM: u32 = 0x200;

// File record `size_flags`.
const SIZE_MASK: u32 = 0x3FFF_FFFF;
const COMPRESSION_TOGGLE: u32 = 0x4000_0000;
/// Absolute offsets only go up to 2 GiB because bit 31 is a flag.
const OFFSET_MASK: u32 = 0x7FFF_FFFF;

#[derive(Debug)]
pub enum BsaError {
    Io(std::io::Error),
    /// The file is some other container entirely.
    NotABsa { tag: u32 },
    UnsupportedVersion(u32),
    /// Xbox 360 archives store records big-endian with reversed hashes.
    Xbox360,
    /// The XMem/LZX codec is not implemented.
    XMemCodec,
    /// Without both name tables the archive cannot be enumerated by name.
    NoNameTables { archive_flags: u32 },
    /// Structural contradiction — the stride or version branch would be wrong.
    Corrupt(String),
    Lz4(lz4::Lz4Error),
    Inflate(inflate::InflateError),
    NotFound(String),
}

impl fmt::Display for BsaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BsaError::Io(e) => write!(f, "{e}"),
            BsaError::NotABsa { tag } => {
                if *tag == TAG_BTDX {
                    write!(f, "this is a Fallout 4 BA2, not a TES4 BSA")
                } else {
                    write!(f, "not a BSA archive (tag {tag:#010x})")
                }
            }
            BsaError::UnsupportedVersion(v) => write!(f, "unsupported BSA version {v}"),
            BsaError::Xbox360 => write!(f, "Xbox 360 archives are not supported"),
            BsaError::XMemCodec => write!(f, "XMem-compressed archives are not supported"),
            BsaError::NoNameTables { archive_flags } => write!(
                f,
                "archive stores no name tables (flags {archive_flags:#05x}), so its \
                 contents cannot be listed"
            ),
            BsaError::Corrupt(what) => write!(f, "malformed archive: {what}"),
            BsaError::Lz4(e) => write!(f, "{e}"),
            BsaError::Inflate(e) => write!(f, "{e}"),
            BsaError::NotFound(name) => write!(f, "{name} is not in this archive"),
        }
    }
}

impl std::error::Error for BsaError {}

impl From<std::io::Error> for BsaError {
    fn from(e: std::io::Error) -> Self {
        BsaError::Io(e)
    }
}

impl From<BsaError> for std::io::Error {
    fn from(e: BsaError) -> Self {
        match e {
            BsaError::Io(e) => e,
            other => std::io::Error::new(std::io::ErrorKind::InvalidData, other.to_string()),
        }
    }
}

/// One file, located and pre-validated at index time.
#[derive(Debug, Clone, Copy)]
pub struct Entry {
    /// Absolute position of the entry's stored bytes.
    offset: u64,
    /// Bytes on disk, covering the optional embedded name and size prefix.
    stored: u32,
    compressed: bool,
}

impl Entry {
    /// Bytes this entry occupies in the archive. Not the file's real size
    /// when it is compressed — that is only known after decoding, so the
    /// name says stored rather than size.
    pub fn stored_size(self) -> u32 {
        self.stored
    }

    pub fn is_compressed(self) -> bool {
        self.compressed
    }
}

/// An opened archive: the whole index in memory, the data read on demand.
///
/// The index is negligible — measured at 0.011% of a 43 MB archive — so it is
/// parsed eagerly at open. Blobs never are: one vanilla 2048² texture is 4 MB
/// decompressed, and a browser that read them eagerly would be unusable.
#[derive(Debug)]
pub struct BsaArchive {
    path: PathBuf,
    file: File,
    version: u32,
    archive_flags: u32,
    /// Keyed by `folder/file`, lowercase with forward slashes — the same
    /// normalisation [`crate::assets`] uses on lookups.
    entries: HashMap<String, Entry>,
    /// Kept sorted so listings are stable between runs; rebuilding this from
    /// the map on every call would be real cost on a 30k-entry archive.
    names: Vec<String>,
}

impl BsaArchive {
    pub fn open(path: impl AsRef<Path>) -> Result<BsaArchive, BsaError> {
        let path = path.as_ref().to_path_buf();
        let file = File::open(&path)?;
        let file_len = file.metadata()?.len();
        let header = read_exact_at(&file, 0, HEADER_SIZE as usize)?;

        let tag = le_u32(&header, 0);
        if tag != TAG_BSA {
            return Err(BsaError::NotABsa { tag });
        }
        let version = le_u32(&header, 4);
        if !(103..=105).contains(&version) {
            return Err(BsaError::UnsupportedVersion(version));
        }
        let folder_record_offset = le_u32(&header, 8) as u64;
        let archive_flags = le_u32(&header, 12);
        let folder_count = le_u32(&header, 16) as usize;
        let file_count = le_u32(&header, 20) as usize;
        let total_folder_name_length = le_u32(&header, 24) as usize;
        let total_file_name_length = le_u32(&header, 28) as usize;

        if archive_flags & FLAG_XBOX360 != 0 {
            return Err(BsaError::Xbox360);
        }
        if archive_flags & FLAG_XMEM != 0 {
            return Err(BsaError::XMemCodec);
        }
        if archive_flags & (FLAG_DIRECTORY_NAMES | FLAG_FILE_NAMES)
            != (FLAG_DIRECTORY_NAMES | FLAG_FILE_NAMES)
        {
            return Err(BsaError::NoNameTables { archive_flags });
        }
        // The engine hard-codes a 36-byte header and never reads this field,
        // so an archive with junk in it still loads in game. Rejecting it
        // would make us stricter than Skyrim itself.
        let index_start = if folder_record_offset == HEADER_SIZE {
            folder_record_offset
        } else {
            HEADER_SIZE
        };

        // The offset field moved in v105 rather than merely widening: it sits
        // at +0x0C in v103/104 and at +0x10 in v105, with uninitialised
        // garbage in between. Keeping a 16-byte layout at a 24-byte stride
        // reads that garbage as the offset.
        let folder_record_size: usize = if version == 105 { 24 } else { 16 };

        let index_bytes = folder_count
            .checked_mul(folder_record_size)
            .ok_or_else(|| BsaError::Corrupt("folder table size overflows".into()))?;
        let table = read_exact_at(&file, index_start, index_bytes)?;

        let mut folder_file_counts = Vec::with_capacity(folder_count);
        let mut counted_files = 0usize;
        for i in 0..folder_count {
            let base = i * folder_record_size;
            let n = le_u32(&table, base + 8) as usize;
            counted_files += n;
            folder_file_counts.push(n);
        }
        if counted_files != file_count {
            return Err(BsaError::Corrupt(format!(
                "folders account for {counted_files} files but the header says {file_count} \
                 — the record stride or version branch is wrong"
            )));
        }

        // Walk folder names and file records together, in record order. The
        // names region is `total_folder_name_length + folder_count` bytes on
        // disk: the header's total counts the bzstring bodies but not their
        // length bytes.
        let walk_len = total_folder_name_length + folder_count + file_count * 16;
        let walk_start = index_start + index_bytes as u64;
        let walk = read_exact_at(&file, walk_start, walk_len)?;

        let mut cursor = 0usize;
        let mut folders: Vec<(String, Vec<(u32, u32)>)> = Vec::with_capacity(folder_count);
        let mut name_bytes_seen = 0usize;
        for &count in &folder_file_counts {
            // bzstring: one length byte, then that many bytes of which the
            // last is the NUL. Confusing it with the NUL-free bstring used
            // for embedded names is the classic off-by-one here.
            let len = *walk
                .get(cursor)
                .ok_or_else(|| BsaError::Corrupt("index ends inside a folder name".into()))?
                as usize;
            cursor += 1;
            let raw = walk
                .get(cursor..cursor + len)
                .ok_or_else(|| BsaError::Corrupt("index ends inside a folder name".into()))?;
            cursor += len;
            name_bytes_seen += len;
            let name = latin1(raw.strip_suffix(&[0]).unwrap_or(raw));

            let mut records = Vec::with_capacity(count);
            for _ in 0..count {
                let rec = walk
                    .get(cursor..cursor + 16)
                    .ok_or_else(|| BsaError::Corrupt("index ends inside a file record".into()))?;
                cursor += 16;
                records.push((le_u32(rec, 8), le_u32(rec, 12)));
            }
            folders.push((name, records));
        }
        if name_bytes_seen != total_folder_name_length {
            return Err(BsaError::Corrupt(format!(
                "folder names measured {name_bytes_seen} bytes, header says \
                 {total_folder_name_length}"
            )));
        }

        // The cursor is now exactly at the file-name block: one NUL-terminated
        // name per file record, in the same global order, with no per-file
        // offsets. A single desync silently renames every later entry, so the
        // count is checked rather than assumed.
        let name_block = read_exact_at(&file, walk_start + cursor as u64, total_file_name_length)?;
        let mut file_names: Vec<String> = Vec::with_capacity(file_count);
        let mut start = 0usize;
        for (i, b) in name_block.iter().enumerate() {
            if *b == 0 {
                if file_names.len() == file_count {
                    // Exactly `file_count` names is all the format promises.
                    // Some packers leave the tail of the block populated —
                    // stale names from a previous pack, or padding — and
                    // rejecting the archive over it threw away every file in
                    // it. The names consumed are still checked below, so a
                    // real desync (too FEW names) is still an error.
                    break;
                }
                file_names.push(latin1(&name_block[start..i]));
                start = i + 1;
            }
        }
        if file_names.len() != file_count {
            return Err(BsaError::Corrupt(format!(
                "file-name block holds {} names for {file_count} records",
                file_names.len()
            )));
        }

        let default_compressed = archive_flags & FLAG_COMPRESSED_BY_DEFAULT != 0;
        let mut entries = HashMap::with_capacity(file_count);
        let mut names = Vec::with_capacity(file_count);
        let mut next_name = 0usize;
        for (folder, records) in &folders {
            for (size_flags, data_offset) in records {
                let leaf = &file_names[next_name];
                next_name += 1;

                let stored = size_flags & SIZE_MASK;
                // Bit 31 of size_flags is a runtime memo the engine writes
                // into its own copy ("already checked for a loose override"),
                // and bit 31 of data_offset marks a secondary archive. Both
                // are masked off rather than mistaken for compression or
                // address bits.
                let offset = (data_offset & OFFSET_MASK) as u64;
                let compressed = default_compressed ^ (size_flags & COMPRESSION_TOGGLE != 0);

                if offset + stored as u64 > file_len {
                    return Err(BsaError::Corrupt(format!(
                        "{folder}\\{leaf} claims bytes past the end of the archive"
                    )));
                }

                let key = crate::assets::normalize(&format!("{folder}\\{leaf}"));
                names.push(key.clone());
                entries.insert(
                    key,
                    Entry {
                        offset,
                        stored,
                        compressed,
                    },
                );
            }
        }
        names.sort();
        names.dedup();

        Ok(BsaArchive {
            path,
            file,
            version,
            archive_flags,
            entries,
            names,
        })
    }

    pub fn version(&self) -> u32 {
        self.version
    }

    pub fn archive_flags(&self) -> u32 {
        self.archive_flags
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Look up an entry. `name` is normalised for you.
    pub fn entry(&self, name: &str) -> Option<Entry> {
        self.entries.get(&crate::assets::normalize(name)).copied()
    }

    /// Every entry's key, sorted.
    pub fn names(&self) -> &[String] {
        &self.names
    }

    /// Read one entry, decompressing if needed.
    pub fn read_entry(&self, name: &str) -> Result<Vec<u8>, BsaError> {
        let key = crate::assets::normalize(name);
        let entry = *self
            .entries
            .get(&key)
            .ok_or_else(|| BsaError::NotFound(key.clone()))?;

        let buf = read_exact_at(&self.file, entry.offset, entry.stored as usize)?;
        let mut pos = 0usize;
        let mut left = entry.stored as usize;

        if self.archive_flags & FLAG_EMBEDDED_NAMES != 0 && self.version >= 104 {
            // bstring: length byte then exactly that many bytes, no NUL. The
            // length byte itself counts against the stored size, so the
            // subtraction is len + 1 — dropping the +1 short-reads the last
            // file in the archive.
            let len = *buf
                .first()
                .ok_or_else(|| BsaError::Corrupt(format!("{key} has no embedded name")))?
                as usize;
            let consumed = 1 + len;
            if consumed > left {
                return Err(BsaError::Corrupt(format!(
                    "{key}'s embedded name runs past its data"
                )));
            }
            pos = consumed;
            left -= consumed;
        }

        if !entry.compressed {
            return Ok(buf[pos..pos + left].to_vec());
        }

        if left < 4 {
            return Err(BsaError::Corrupt(format!("{key} has no size prefix")));
        }
        let out_len = le_u32(&buf, pos) as usize;
        pos += 4;
        left -= 4;
        let payload = &buf[pos..pos + left];

        // SE archives carry LZ4 frames; LE and Oblivion-era ones carry zlib.
        // Both codecs are ours, so an LE archive works offline too.
        let data = match self.version {
            105 => {
                let (data, used) =
                    lz4::decode_frame(payload, out_len).map_err(BsaError::Lz4)?;
                // The consumed count proves the entry boundary was right; the
                // zlib path gets the same assurance from its Adler-32.
                if used != payload.len() {
                    return Err(BsaError::Corrupt(format!(
                        "{key}'s codec consumed {used} of {} payload bytes",
                        payload.len()
                    )));
                }
                data
            }
            _ => inflate::zlib_decompress(payload, out_len).map_err(BsaError::Inflate)?,
        };
        // The length check proves the codec produced the whole file. A
        // mismatch means we handed the decoder the wrong bytes, which must
        // never be served as content.
        if data.len() != out_len {
            return Err(BsaError::Corrupt(format!(
                "{key} decompressed to {} bytes, the archive says {out_len}",
                data.len()
            )));
        }
        Ok(data)
    }
}

impl crate::assets::Archive for BsaArchive {
    fn path(&self) -> &Path {
        &self.path
    }

    fn contains(&self, normalized: &str) -> bool {
        self.entries.contains_key(normalized)
    }

    fn read(&self, normalized: &str) -> std::io::Result<Vec<u8>> {
        self.read_entry(normalized).map_err(Into::into)
    }

    fn entries(&self) -> Vec<String> {
        self.names.clone()
    }

    /// A prefix is a CONTIGUOUS run in `names`, which is kept sorted, so
    /// this is two binary searches and a copy of what was asked for —
    /// not a copy of the archive's whole name table per call.
    ///
    /// Lexicographic order is what makes the run contiguous, and it is
    /// the order `names` is built in. Nothing else here may reorder it.
    fn entries_under(&self, prefix: &str) -> Vec<String> {
        if prefix.is_empty() {
            return self.names.clone();
        }
        let start = self.names.partition_point(|n| n.as_str() < prefix);
        self.names[start..]
            .iter()
            .take_while(|n| n.starts_with(prefix))
            .cloned()
            .collect()
    }
}

/// Positional read that does not disturb a file cursor.
///
/// `Archive::read` takes `&self`, so `Seek`+`Read` would force a mutex around
/// the file and serialise every texture-browser read behind one lock — which
/// is exactly what NifSkope does. `seek_read`/`read_at` take `&self` and are
/// both std, so nothing is serialised and nothing is added to the dep list.
fn read_exact_at(file: &File, offset: u64, len: usize) -> std::io::Result<Vec<u8>> {
    let mut buf = vec![0u8; len];
    let mut done = 0usize;
    while done < len {
        // Short reads are legal from both APIs; looping is not optional.
        let n = positional_read(file, &mut buf[done..], offset + done as u64)?;
        if n == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                format!("archive ends {} bytes into a {len}-byte read", done),
            ));
        }
        done += n;
    }
    Ok(buf)
}

#[cfg(windows)]
fn positional_read(file: &File, buf: &mut [u8], offset: u64) -> std::io::Result<usize> {
    use std::os::windows::fs::FileExt;
    file.seek_read(buf, offset)
}

#[cfg(unix)]
fn positional_read(file: &File, buf: &mut [u8], offset: u64) -> std::io::Result<usize> {
    use std::os::unix::fs::FileExt;
    file.read_at(buf, offset)
}

fn le_u32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

/// Names are single-byte, and archivers have been known to emit non-ASCII in
/// them. Latin-1 maps every byte to some character, so a stray byte becomes a
/// visible name rather than a decode failure that hides the whole folder.
fn latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|b| *b as char).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal but real v105 archive in memory.
    ///
    /// Written by hand rather than shipped as a fixture: a real archive is
    /// tens of megabytes, and building one here lets each test pin a specific
    /// structural rule (the v105 stride, the string encodings, the XOR).
    struct Builder {
        version: u32,
        archive_flags: u32,
        /// `(folder, leaf, plaintext, toggle_compression_bit, pre_encoded)`.
        /// `pre_encoded` lets a test supply a real codec stream — the builder
        /// has no compressor of its own, which is the point.
        files: Vec<(String, String, Vec<u8>, bool, Option<Vec<u8>>)>,
    }

    impl Builder {
        fn new(version: u32, archive_flags: u32) -> Builder {
            Builder {
                version,
                archive_flags: archive_flags | FLAG_DIRECTORY_NAMES | FLAG_FILE_NAMES,
                files: Vec::new(),
            }
        }

        fn file(mut self, folder: &str, leaf: &str, data: &[u8], toggle: bool) -> Builder {
            self.files
                .push((folder.into(), leaf.into(), data.to_vec(), toggle, None));
            self
        }

        /// A file whose stored payload is exactly `encoded` — used to put a
        /// genuine zlib stream into a v104 archive.
        fn encoded_file(
            mut self,
            folder: &str,
            leaf: &str,
            plain: &[u8],
            encoded: &[u8],
            toggle: bool,
        ) -> Builder {
            self.files.push((
                folder.into(),
                leaf.into(),
                plain.to_vec(),
                toggle,
                Some(encoded.to_vec()),
            ));
            self
        }

        fn build(&self) -> Vec<u8> {
            let record_size = if self.version == 105 { 24 } else { 16 };
            // Group by folder, preserving insertion order.
            let mut folders: Vec<(String, Vec<usize>)> = Vec::new();
            for (i, (folder, ..)) in self.files.iter().enumerate() {
                match folders.iter_mut().find(|(f, _)| f == folder) {
                    Some((_, list)) => list.push(i),
                    None => folders.push((folder.clone(), vec![i])),
                }
            }

            let total_folder_name_length: usize =
                folders.iter().map(|(f, _)| f.len() + 1).sum();
            let file_name_block: Vec<u8> = self
                .files
                .iter()
                .flat_map(|(_, leaf, ..)| {
                    leaf.bytes().chain(std::iter::once(0)).collect::<Vec<u8>>()
                })
                .collect();

            let index_len = HEADER_SIZE as usize
                + folders.len() * record_size
                + total_folder_name_length
                + folders.len()
                + self.files.len() * 16
                + file_name_block.len();

            // Lay the blobs out after the index, in record order.
            let default_compressed = self.archive_flags & FLAG_COMPRESSED_BY_DEFAULT != 0;
            let mut blobs: Vec<(u64, Vec<u8>, u32)> = Vec::new();
            let mut at = index_len as u64;
            for (folder, leaf, data, toggle, encoded) in &self.files {
                let compressed = default_compressed ^ toggle;
                let mut stored = Vec::new();
                if self.archive_flags & FLAG_EMBEDDED_NAMES != 0 && self.version >= 104 {
                    let full = format!("{folder}\\{leaf}");
                    stored.push(full.len() as u8);
                    stored.extend_from_slice(full.as_bytes());
                }
                if compressed {
                    stored.extend_from_slice(&(data.len() as u32).to_le_bytes());
                    match encoded {
                        Some(bytes) => stored.extend_from_slice(bytes),
                        None => stored.extend_from_slice(&lz4_frame_of(data)),
                    }
                } else {
                    stored.extend_from_slice(data);
                }
                let mut size_flags = stored.len() as u32;
                if *toggle {
                    size_flags |= COMPRESSION_TOGGLE;
                }
                blobs.push((at, stored.clone(), size_flags));
                at += stored.len() as u64;
            }

            let mut out = Vec::new();
            out.extend_from_slice(&TAG_BSA.to_le_bytes());
            out.extend_from_slice(&self.version.to_le_bytes());
            out.extend_from_slice(&(HEADER_SIZE as u32).to_le_bytes());
            out.extend_from_slice(&self.archive_flags.to_le_bytes());
            out.extend_from_slice(&(folders.len() as u32).to_le_bytes());
            out.extend_from_slice(&(self.files.len() as u32).to_le_bytes());
            // The header's folder-name total excludes the length bytes.
            out.extend_from_slice(&(total_folder_name_length as u32).to_le_bytes());
            out.extend_from_slice(&(file_name_block.len() as u32).to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes()); // file_flags
            out.extend_from_slice(&0u16.to_le_bytes()); // padding

            for (i, (_, list)) in folders.iter().enumerate() {
                out.extend_from_slice(&(0xFFFF_0000u64 + i as u64).to_le_bytes()); // hash
                out.extend_from_slice(&(list.len() as u32).to_le_bytes());
                if self.version == 105 {
                    // Real archives leave junk here; a reader that mistakes it
                    // for the offset field resolves zero folders.
                    out.extend_from_slice(&0x0072_0079u32.to_le_bytes());
                    out.extend_from_slice(&0u64.to_le_bytes()); // name_offset
                } else {
                    out.extend_from_slice(&0u32.to_le_bytes()); // name_offset
                }
            }

            for (folder, list) in &folders {
                // bzstring: the length byte counts the trailing NUL.
                out.push((folder.len() + 1) as u8);
                out.extend_from_slice(folder.as_bytes());
                out.push(0);
                for &i in list {
                    let (offset, _, size_flags) = &blobs[i];
                    out.extend_from_slice(&(0xEEEE_0000u64 + i as u64).to_le_bytes());
                    out.extend_from_slice(&size_flags.to_le_bytes());
                    out.extend_from_slice(&(*offset as u32).to_le_bytes());
                }
            }
            out.extend_from_slice(&file_name_block);
            assert_eq!(out.len(), index_len, "builder index length must be exact");
            for (_, stored, _) in &blobs {
                out.extend_from_slice(stored);
            }
            out
        }

        fn open(&self, name: &str) -> Result<(BsaArchive, PathBuf), BsaError> {
            let path = std::env::temp_dir().join(format!("aether-test-{name}.bsa"));
            std::fs::write(&path, self.build()).expect("writes fixture");
            BsaArchive::open(&path).map(|a| (a, path))
        }
    }

    /// A frame with one stored block — a legal LZ4 frame that needs no
    /// compressor, which keeps the builder honest without one.
    fn lz4_frame_of(data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&0x184D_2204u32.to_le_bytes());
        out.push(0x60);
        out.push(0x70);
        out.push(0x00);
        out.extend_from_slice(&(data.len() as u32 | 0x8000_0000).to_le_bytes());
        out.extend_from_slice(data);
        out.extend_from_slice(&0u32.to_le_bytes());
        out
    }

    #[test]
    fn a_v105_archive_indexes_every_file_under_its_full_path() {
        // The v105 folder record is 24 bytes with the name offset at +0x10 and
        // garbage at +0x0C; a reader that keeps the 16-byte layout at a
        // 24-byte stride resolves nothing.
        let (bsa, path) = Builder::new(105, 0)
            .file("textures\\effects", "fire.dds", b"FIREDATA", false)
            .file("textures\\effects", "smoke.dds", b"SMOKE", false)
            .file("meshes", "thing.nif", b"NIF", false)
            .open("v105-index")
            .expect("opens");
        assert_eq!(bsa.len(), 3);
        assert_eq!(
            bsa.names(),
            &[
                "meshes/thing.nif".to_string(),
                "textures/effects/fire.dds".to_string(),
                "textures/effects/smoke.dds".to_string(),
            ]
        );
        assert_eq!(bsa.read_entry("textures/effects/fire.dds").unwrap(), b"FIREDATA");
        assert_eq!(bsa.read_entry("meshes/thing.nif").unwrap(), b"NIF");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_v104_archive_uses_the_16_byte_folder_record() {
        // Same content, other stride. Getting this branch wrong is silent:
        // the file counts still add up, the names just come out as garbage.
        let (bsa, path) = Builder::new(104, 0)
            .file("textures", "a.dds", b"AAA", false)
            .open("v104-index")
            .expect("opens");
        assert_eq!(bsa.read_entry("textures/a.dds").unwrap(), b"AAA");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn compression_is_the_xor_of_the_archive_default_and_the_entry_bit() {
        // The highest-value rule in the format. An archive packed with
        // "compressed by default" and no per-entry bits has every entry
        // compressed; reading bit 30 alone would serve raw LZ4 frames.
        let (bsa, path) = Builder::new(105, FLAG_COMPRESSED_BY_DEFAULT)
            .file("textures", "packed.dds", b"COMPRESSED CONTENT", false)
            .file("textures", "plain.dds", b"RAW CONTENT", true)
            .open("xor")
            .expect("opens");

        assert!(bsa.entry("textures/packed.dds").unwrap().is_compressed());
        assert!(!bsa.entry("textures/plain.dds").unwrap().is_compressed());
        assert_eq!(
            bsa.read_entry("textures/packed.dds").unwrap(),
            b"COMPRESSED CONTENT"
        );
        assert_eq!(bsa.read_entry("textures/plain.dds").unwrap(), b"RAW CONTENT");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn embedded_names_are_skipped_including_their_length_byte() {
        // Subtracting `len` instead of `len + 1` is a live bug in one widely
        // copied reader; it corrupts uncompressed entries by exactly one byte.
        let (bsa, path) = Builder::new(105, FLAG_EMBEDDED_NAMES)
            .file("textures\\effects", "fire.dds", b"EXACTLY THIS", false)
            .open("embedded")
            .expect("opens");
        assert_eq!(
            bsa.read_entry("textures/effects/fire.dds").unwrap(),
            b"EXACTLY THIS"
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn embedded_names_and_compression_coexist() {
        // Order matters: the name comes first, the uncompressed-size u32
        // second, and both count against the stored size.
        let (bsa, path) = Builder::new(105, FLAG_EMBEDDED_NAMES | FLAG_COMPRESSED_BY_DEFAULT)
            .file("meshes", "a.nif", b"NIF CONTENT HERE", false)
            .open("embedded-compressed")
            .expect("opens");
        assert_eq!(bsa.read_entry("meshes/a.nif").unwrap(), b"NIF CONTENT HERE");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn lookups_ignore_case_and_slash_direction() {
        // Archives store lowercase backslash paths by convention, but that is
        // an archiver habit, not a guarantee, and callers pass NIF paths in
        // whatever form the file used.
        let (bsa, path) = Builder::new(105, 0)
            .file("Textures\\Effects", "Fire.DDS", b"X", false)
            .open("normalise")
            .expect("opens");
        assert!(bsa.entry("textures/effects/fire.dds").is_some());
        assert!(bsa.entry("Textures\\Effects\\Fire.DDS").is_some());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_ba2_is_rejected_with_a_message_that_names_the_format() {
        // Users will absolutely point this at a Fallout 4 archive.
        let path = std::env::temp_dir().join("aether-test-ba2.bsa");
        let mut bytes = TAG_BTDX.to_le_bytes().to_vec();
        bytes.extend_from_slice(&[0u8; 64]);
        std::fs::write(&path, bytes).expect("writes");
        let err = BsaArchive::open(&path).unwrap_err();
        assert!(err.to_string().contains("BA2"), "got {err}");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn an_archive_without_name_tables_is_refused_rather_than_mis_parsed() {
        // Without both name flags the on-disk layout differs and there is no
        // way to enumerate; guessing would produce plausible nonsense.
        let path = std::env::temp_dir().join("aether-test-nonames.bsa");
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&TAG_BSA.to_le_bytes());
        bytes.extend_from_slice(&105u32.to_le_bytes());
        bytes.extend_from_slice(&(HEADER_SIZE as u32).to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes()); // no name flags
        bytes.extend_from_slice(&[0u8; 20]);
        std::fs::write(&path, bytes).expect("writes");
        assert!(matches!(
            BsaArchive::open(&path),
            Err(BsaError::NoNameTables { .. })
        ));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_file_count_that_disagrees_with_the_folder_table_is_refused() {
        // This mismatch is how a wrong stride announces itself, and carrying
        // on would desync every name in the archive.
        let mut bytes = Builder::new(105, 0)
            .file("textures", "a.dds", b"A", false)
            .build();
        bytes[20..24].copy_from_slice(&7u32.to_le_bytes()); // file_count
        let path = std::env::temp_dir().join("aether-test-badcount.bsa");
        std::fs::write(&path, bytes).expect("writes");
        let err = BsaArchive::open(&path).unwrap_err();
        assert!(err.to_string().contains("stride"), "got {err}");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn an_entry_pointing_past_the_end_of_the_file_is_refused_at_open() {
        // The header must not be trusted over the file; catching this at
        // index time beats a confusing read error later.
        let mut bytes = Builder::new(105, 0)
            .file("textures", "a.dds", b"AAAA", false)
            .build();
        let len = bytes.len();
        // The single file record's offset is the last 4 bytes before the
        // file-name block; find it by value instead of by arithmetic.
        let blob_offset = (len - 4) as u32;
        let needle = blob_offset.to_le_bytes();
        let at = bytes
            .windows(4)
            .position(|w| w == needle)
            .expect("record offset present");
        bytes[at..at + 4].copy_from_slice(&(len as u32 + 1000).to_le_bytes());
        let path = std::env::temp_dir().join("aether-test-oob.bsa");
        std::fs::write(&path, bytes).expect("writes");
        let err = BsaArchive::open(&path).unwrap_err();
        assert!(err.to_string().contains("past the end"), "got {err}");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_v104_archive_decompresses_its_zlib_entries() {
        // Skyrim LE is a target game and its archives are zlib, not LZ4. The
        // payload here is a real encoder's output, so this proves the whole
        // chain — XOR, size prefix, inflate, Adler-32 — on the LE branch.
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        let plain = std::fs::read(dir.join("zlib_repetitive.bin")).expect("fixture");
        let encoded = std::fs::read(dir.join("zlib_repetitive.zlib")).expect("fixture");

        let (bsa, path) = Builder::new(104, FLAG_COMPRESSED_BY_DEFAULT)
            .encoded_file("textures", "le.dds", &plain, &encoded, false)
            .open("v104-zlib")
            .expect("opens");
        assert!(bsa.entry("textures/le.dds").unwrap().is_compressed());
        assert_eq!(bsa.read_entry("textures/le.dds").unwrap(), plain);
        let _ = std::fs::remove_file(path);
    }

    /// **A prefix range must be the prefix.**
    ///
    /// `entries_under` answers by binary-searching a sorted name table
    /// instead of scanning it, which is what took a horizon's archive
    /// listing from 73 ms to 6 ms. The failure mode of a range search is
    /// never loud: it drops the first or last name in the run, and what
    /// the caller sees is one missing LOD tile or one texture the browser
    /// cannot find — indistinguishable from the file not being there.
    ///
    /// So it is asserted against the definition it replaced, over prefixes
    /// chosen to sit on every boundary: a whole folder, a partial leaf, a
    /// name that is itself a prefix of a longer one (`terrain/` vs
    /// `terrain2/` — the case a naive `take_while` on `<=` gets wrong), the
    /// empty prefix, and one that matches nothing.
    #[test]
    fn a_prefix_listing_is_exactly_the_scan_it_replaces() {
        let bytes = Builder::new(105, 0)
            .file("meshes/terrain", "a.btr", b"a", false)
            .file("meshes/terrain", "b.btr", b"b", false)
            .file("meshes/terrain/objects", "c.bto", b"c", false)
            .file("meshes/terrain2", "d.btr", b"d", false)
            .file("meshes/terrainx", "e.btr", b"e", false)
            .file("meshes", "f.nif", b"f", false)
            .file("textures", "g.dds", b"g", false)
            .build();
        let dir = std::env::temp_dir().join("aether-bsa-prefix");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("prefix.bsa");
        std::fs::write(&path, &bytes).unwrap();
        let archive = BsaArchive::open(&path).unwrap();

        use crate::assets::Archive;
        let all = archive.entries();
        assert_eq!(all.len(), 7);
        for prefix in [
            "",
            "meshes/terrain",
            "meshes/terrain/",
            "meshes/terrain2/",
            "meshes/te",
            "meshes/terrain/objects/c.bto",
            "textures/",
            "zzz",
        ] {
            let scanned: Vec<String> = all
                .iter()
                .filter(|e| e.starts_with(prefix))
                .cloned()
                .collect();
            assert_eq!(
                archive.entries_under(prefix),
                scanned,
                "prefix {prefix:?} disagrees with the scan"
            );
        }
        // The one that would pass by accident if the run were empty.
        assert_eq!(archive.entries_under("meshes/terrain/").len(), 3);
        let _ = std::fs::remove_file(&path);
    }

}
