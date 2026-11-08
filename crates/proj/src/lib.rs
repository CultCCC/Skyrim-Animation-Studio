//! NewProjectilesTMP (fenix31415) configs for SDS Projectile Studio.
//!
//! - [`model`]: one `Data/HomingProjectiles/*.json`, lenient read, write in
//!   either condition format.
//! - [`world`]: the load order (records, spell → projectile, config files).
//! - [`validate`]: what the DLL will reject or ignore.
//! - [`chain`]: triggers unfolded into readable chains.
//! - [`pattern`]: spawn points of a figure, as the DLL computes them.

pub mod chain;
pub mod model;
pub mod pattern;
pub mod validate;
pub mod world;

use sds_records::FormKey;

/// A place in a config the editor can select.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Sel {
    Root,
    Trigger(usize),
    /// Function `i` of the TriggerFunctions owned by the inner selection.
    Func(Box<Sel>, usize),
    FormId(String),
    Homing(String),
    Group(String),
    Multicast(String),
    McItem(String, usize),
    Emitter(String),
    EmFunc(String, usize),
    Follower(String),
    /// A load-order record shown in a chain (read-only).
    Record(FormKey),
}

impl Sel {
    /// The editable thing this selection lives in (functions → their owner).
    pub fn owner(&self) -> &Sel {
        match self {
            Sel::Func(o, _) => o.owner(),
            other => other,
        }
    }
}

#[cfg(test)]
mod tests;
