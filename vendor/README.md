# Vendored dependencies

Copies of the crates SDS Anim Studio builds against, so a fresh clone builds
with plain `cargo build`. Test data, documentation assets and build output of
the originals are left out.

| Folder | Source | License |
|---|---|---|
| `aether/nif`, `aether/core`, `aether/codecs` | Aether — Skyrim NIF / particle editor by the SDS author (NIF reader and writer, mesh and particle core, DEFLATE/LZ4 codecs) | GPL-3.0 (see the repository `LICENSE`) |
| `serde-hkx` | [serde-hkx](https://github.com/SARDONYX-sard/serde-hkx) by SARDONYX — the Havok HKX (de)serializer, version 2.0.0 | MIT OR Apache-2.0 (`serde-hkx/LICENSE-MIT`, `serde-hkx/LICENSE-APACHE`) |

They are excluded from the studio workspace (`exclude = ["vendor"]` in the
root `Cargo.toml`) and pulled in as path dependencies.
