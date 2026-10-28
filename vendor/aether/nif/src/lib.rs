//! `aether-nif` — a pure-Rust reader/writer for Gamebryo/NetImmerse NIF files,
//! targeting Skyrim Special Edition (20.2.0.7, BS stream 100) and
//! Skyrim Legendary Edition (20.2.0.7, BS stream 83).
//!
//! Design goals:
//! - **Non-destructive round-trip of any SSE/LE NIF.** Every block is kept as
//!   raw bytes; typed views decode/encode on top. Blocks the editor does not
//!   understand are preserved byte-identically on save.
//! - **No dependencies.** The crate builds with the Rust standard library only.
//! - **Format truth from `nif.xml`.** Typed block layouts follow the niftools
//!   `nifxml` definitions for version 20.2.0.7.

pub mod error;
pub mod file;
pub mod header;
pub mod stream;
pub mod typed;

pub use error::{Error, Result};
pub use file::{Block, NifFile};
pub use header::{BsStreamVersion, Header};
