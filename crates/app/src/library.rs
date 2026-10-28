//! Animation library over the load order: every clip under
//! `meshes/**/animations/`, as a folder tree and as OAR mods → submods.

use sds_vfs::Vfs;
use std::collections::BTreeMap;

#[derive(Default)]
pub struct Node {
    pub children: BTreeMap<String, Node>,
    /// Indices into [`Library::anims`].
    pub files: Vec<usize>,
    pub count: usize,
}

#[derive(Default)]
pub struct Library {
    /// Normalised paths, sorted.
    pub anims: Vec<String>,
    pub tree: Node,
    pub oar: Vec<crate::oar::OarMod>,
}

impl Library {
    pub fn build(vfs: &Vfs) -> Library {
        let mut anims = vfs.paths(|p| p.starts_with("meshes/") && p.ends_with(".hkx") && p.contains("/animations/"));
        anims.sort();
        let mut tree = Node::default();
        for (i, p) in anims.iter().enumerate() {
            let rel = p.strip_prefix("meshes/").unwrap_or(p);
            let mut node = &mut tree;
            node.count += 1;
            let parts: Vec<&str> = rel.split('/').collect();
            for part in &parts[..parts.len() - 1] {
                node = node.children.entry(part.to_string()).or_default();
                node.count += 1;
            }
            node.files.push(i);
        }

        let oar = crate::oar::scan_vfs(vfs, &anims);
        Library { anims, tree, oar }
    }
}

/// File name part of a normalised path.
pub fn file_name(p: &str) -> &str {
    p.rsplit('/').next().unwrap_or(p)
}
