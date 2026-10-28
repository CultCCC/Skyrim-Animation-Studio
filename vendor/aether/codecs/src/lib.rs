//! Std-only compression codecs, shared by whoever needs to open Bethesda
//! containers: `aether-core`'s BSA reader (zlib for LE archives, LZ4 for
//! SE) and `aether-esp`'s compressed records (zlib). Lives in its own
//! crate so the plugin core does not have to depend on the whole editor
//! core — layering is `nif ← core`, `codecs ← {core, esp}`.

pub mod inflate;
pub mod lz4;
