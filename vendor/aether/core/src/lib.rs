//! `aether-core` — editor domain model for Aether, std-only.
//!
//! Contains the particle-graph document model, the undoable command system,
//! the project model, and built-in templates. UI crates render these models;
//! `aether-nif` serializes them to/from NIF blocks.

pub mod assets;
pub mod bc7;
pub mod behavior;
pub mod bsa;
pub mod clip;
pub mod collision;
pub mod command;
pub mod curve;
pub mod compile;
pub mod dds;
pub mod decompile;
pub mod graph_ru;
pub mod derive;
pub mod docpath;
pub mod fragment;
pub mod graph;
pub mod json;
// The codecs moved to their own crate so the plugin core can share them
// without depending on the whole editor core; re-exported here so the
// old `aether_core::inflate` / `::lz4` paths keep working.
pub use aether_codecs::{inflate, lz4};
pub mod layout;
pub mod mesh;
pub mod path;
pub mod png;
pub mod project;
pub mod scale_table;
pub mod scene_tree;
pub mod sequence;
pub mod shader_preview;
pub mod settings;
pub mod sim;
pub mod library;
pub mod lightning;
pub mod strip;
pub mod templates;
pub mod texture;
pub mod tga;
pub mod validate;
pub mod value;
