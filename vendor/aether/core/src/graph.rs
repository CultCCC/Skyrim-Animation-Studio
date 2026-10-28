//! Particle-graph document model.
//!
//! The graph mirrors how a Skyrim particle effect is authored, not how it is
//! stored: emitter controllers link to emitters by wire (the NIF serializer
//! resolves `Modifier Name` strings), and modifier stacking order is derived
//! from canonical `NiPSysModifierOrder` values at export time.

use crate::curve::{Curve, Track};
use crate::json::Json;
use crate::value::{SocketType, Value};
use std::collections::HashMap;

/// Stable node identifier within one graph document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub u64);

/// Node category — drives header colors in the UI (Blender-style).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    System,
    Data,
    Emitter,
    Force,
    Appearance,
    Controller,
    Shader,
    Output,
}

impl Category {
    pub fn name(self) -> &'static str {
        match self {
            Category::System => "System",
            Category::Data => "Data",
            Category::Emitter => "Emitter",
            Category::Force => "Force",
            Category::Appearance => "Appearance",
            Category::Controller => "Controller",
            Category::Shader => "Shader",
            Category::Output => "Output",
        }
    }
}

/// All node kinds the editor can author today.
///
/// Names map 1:1 to NIF block concepts; see `docs/` and the old project's
/// knowledge base for the format background.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NodeKind {
    SceneOutput,
    ParticleSystem,
    ParticleData,
    BoxEmitter,
    SphereEmitter,
    CylinderEmitter,
    MeshEmitter,
    EmitterCtlr,
    UpdateCtlr,
    GravityModifier,
    RotationModifier,
    SimpleColorModifier,
    ScaleModifier,
    DragModifier,
    BombModifier,
    ColliderModifier,
    SubTexModifier,
    SpawnModifier,
    LodModifier,
    ModifierActiveCtlr,
    /// One keyed float track driving one parameter of the wired
    /// modifier (the NiPSysModifierFloatCtlr family — twelve block
    /// types, one concept).
    ParamCtlr,
    /// NiVisController in the system's chain: a bool track on the
    /// system's hidden flag.
    VisibilityCtlr,
    /// BSPSysMultiTargetEmitterCtlr — emitter controller spawning at
    /// every placed reference of the master particle system.
    MultiEmitterCtlr,
    InheritVelocityModifier,
    RecycleBoundModifier,
    WindModifier,
    EffectShader,
    AlphaProperty,
}

pub const ALL_NODE_KINDS: &[NodeKind] = &[
    NodeKind::SceneOutput,
    NodeKind::ParticleSystem,
    NodeKind::ParticleData,
    NodeKind::BoxEmitter,
    NodeKind::SphereEmitter,
    NodeKind::CylinderEmitter,
    NodeKind::MeshEmitter,
    NodeKind::EmitterCtlr,
    NodeKind::UpdateCtlr,
    NodeKind::GravityModifier,
    NodeKind::RotationModifier,
    NodeKind::SimpleColorModifier,
    NodeKind::ScaleModifier,
    NodeKind::DragModifier,
    NodeKind::BombModifier,
    NodeKind::ColliderModifier,
    NodeKind::SubTexModifier,
    NodeKind::SpawnModifier,
    NodeKind::LodModifier,
    NodeKind::ModifierActiveCtlr,
    NodeKind::ParamCtlr,
    NodeKind::VisibilityCtlr,
    NodeKind::MultiEmitterCtlr,
    NodeKind::InheritVelocityModifier,
    NodeKind::RecycleBoundModifier,
    NodeKind::WindModifier,
    NodeKind::EffectShader,
    NodeKind::AlphaProperty,
];

/// One input or output socket in a node schema.
#[derive(Debug, Clone, Copy)]
pub struct SocketSchema {
    pub name: &'static str,
    pub ty: SocketType,
    /// Inputs only: whether several wires may land here (e.g. `Modifiers`).
    pub multi: bool,
}

const fn sock(name: &'static str, ty: SocketType) -> SocketSchema {
    SocketSchema {
        name,
        ty,
        multi: false,
    }
}

const fn multi(name: &'static str, ty: SocketType) -> SocketSchema {
    SocketSchema {
        name,
        ty,
        multi: true,
    }
}

/// Static description of a node kind.
pub struct NodeSchema {
    pub kind: NodeKind,
    /// Stable id used in project files.
    pub id: &'static str,
    pub title: &'static str,
    pub category: Category,
    pub inputs: &'static [SocketSchema],
    pub outputs: &'static [SocketSchema],
    /// Ordered parameter list: `(key, default value)`.
    pub params: fn() -> Vec<(&'static str, Value)>,
    /// Keys worth showing directly in the node body — the few a user reaches
    /// for while wiring. Everything else lives in the Inspector, so nodes stay
    /// readable at a glance instead of becoming walls of fields. Empty means
    /// "show every param" (used by nodes that only have a couple).
    pub body_params: &'static [&'static str],
    /// Short help shown in tooltips.
    pub help: &'static str,
}

impl NodeSchema {
    /// Params to render inside the node body, in schema order.
    pub fn is_body_param(&self, key: &str) -> bool {
        self.body_params.is_empty() || self.body_params.contains(&key)
    }
}

pub const FORCE_TYPES: &[&str] = &["Planar", "Spherical"];
pub const VELOCITY_TYPES: &[&str] = &["Use Normals", "Random", "Use Direction"];
pub const EMIT_FROM_TYPES: &[&str] =
    &["Vertices", "Face Center", "Edge Center", "Face Surface", "Edge Surface"];
/// The blend half of `AlphaFlags` (bits 0-8) as an artist reads it,
/// plus the escape hatch.
///
/// The first three are the words vanilla ships -- measured over the
/// SSE install, 1,674 particle-system `NiAlphaProperty` blocks carry
/// six distinct flag words and every one of them is one of these three
/// blend halves. `Custom` is what makes the control TOTAL rather than
/// merely sufficient: it exposes the enable bit and both factors, so
/// any word the format can hold can be authored and none can be lost
/// by rewriting it as the nearest preset.
pub const BLEND_PRESETS: &[&str] = &["Additive", "Alpha Blend", "Opaque", "Custom"];

/// nif.xml `AlphaFunction`, the source/destination blend factors.
///
/// The field is FOUR bits and the enum defines eleven values, so the
/// last five are spellable in a file and mean nothing. They are listed
/// rather than clamped away: a value the format can hold must survive
/// a round trip, the same rule `widen_range` follows for numbers.
pub const ALPHA_FUNCTIONS: &[&str] = &[
    "ONE",
    "ZERO",
    "SRC_COLOR",
    "INV_SRC_COLOR",
    "DST_COLOR",
    "INV_DST_COLOR",
    "SRC_ALPHA",
    "INV_SRC_ALPHA",
    "DST_ALPHA",
    "INV_DST_ALPHA",
    "SRC_ALPHA_SAT",
    "11 (undefined)",
    "12 (undefined)",
    "13 (undefined)",
    "14 (undefined)",
    "15 (undefined)",
];

/// nif.xml `TestFunction`, bits 10-12 of the same word. Three bits,
/// eight values, all defined.
pub const TEST_FUNCTIONS: &[&str] = &[
    "ALWAYS",
    "LESS",
    "EQUAL",
    "LESSEQUAL",
    "GREATER",
    "NOTEQUAL",
    "GREATEREQUAL",
    "NEVER",
];

/// What a Param Track can animate, in display order. Index-paired with
/// [`PARAM_CTLR_BLOCKS`]: entry i names the parameter, and the block
/// type the compiler emits for it. The pairing being positional is what
/// keeps "add a parameter" a two-line change.
pub const PARAM_TARGETS: &[&str] = &[
    "Speed",
    "Gravity Strength",
    "Initial Radius",
    "Life Span",
    "Declination",
    "Declination Variation",
    "Planar Angle",
    "Planar Angle Variation",
    "Rotation Speed",
    "Rotation Speed Variation",
    "Rotation Angle",
    "Rotation Angle Variation",
];

/// The node a `Param Track` choice reaches, for each [`PARAM_TARGETS`]
/// entry.
///
/// The track turns a knob on the node wired into its input, and which
/// node that must be is a property of the CHOICE, not of the track --
/// so the picker groups by this and the reader can see what to wire
/// before making the choice rather than after. `Emitter` is the family
/// of four rather than one title: the emitter parameters live on
/// `NiPSysEmitter`, so any of the four emitter kinds accepts them.
pub const PARAM_TARGET_OWNERS: &[&str] = &[
    "Emitter",  // Speed
    "Gravity",  // Gravity Strength
    "Emitter",  // Initial Radius
    "Emitter",  // Life Span
    "Emitter",  // Declination
    "Emitter",  // Declination Variation
    "Emitter",  // Planar Angle
    "Emitter",  // Planar Angle Variation
    "Rotation", // Rotation Speed
    "Rotation", // Rotation Speed Variation
    "Rotation", // Rotation Angle
    "Rotation", // Rotation Angle Variation
];

/// The NIF block type for each [`PARAM_TARGETS`] entry.
pub const PARAM_CTLR_BLOCKS: &[&str] = &[
    "NiPSysEmitterSpeedCtlr",
    "NiPSysGravityStrengthCtlr",
    "NiPSysEmitterInitialRadiusCtlr",
    "NiPSysEmitterLifeSpanCtlr",
    "NiPSysEmitterDeclinationCtlr",
    "NiPSysEmitterDeclinationVarCtlr",
    "NiPSysEmitterPlanarAngleCtlr",
    "NiPSysEmitterPlanarAngleVarCtlr",
    "NiPSysInitialRotSpeedCtlr",
    "NiPSysInitialRotSpeedVarCtlr",
    "NiPSysInitialRotAngleCtlr",
    "NiPSysInitialRotAngleVarCtlr",
];
pub const CLAMP_MODES: &[&str] = &[
    "Wrap Both",
    "Clamp Both",
    "Clamp U / Wrap V",
    "Wrap U / Clamp V",
];
pub const DECAY_TYPES: &[&str] = &["None", "Linear", "Exponential"];
pub const SYMMETRY_TYPES: &[&str] = &["Spherical", "Cylindrical", "Planar"];
pub const COLLIDER_SHAPES: &[&str] = &["Sphere", "Plane"];

fn no_params() -> Vec<(&'static str, Value)> {
    Vec::new()
}

/// Presentation metadata for one parameter: what the UI shows and enforces.
/// One table, next to the schema — widgets and the Inspector both read it,
/// so a range or a tooltip exists exactly once.
#[derive(Clone, Copy)]
pub struct ParamMeta {
    /// Human label shown instead of the raw key; `None` keeps the key.
    pub label: Option<&'static str>,
    /// Hover text: what the value does in game terms, units, gotchas.
    pub tooltip: &'static str,
    /// Hard clamp bounds where the format or the RE'd runtime defines one.
    pub min: Option<f32>,
    pub max: Option<f32>,
}

/// Look up presentation metadata for `(node kind, param key)`.
///
/// Semantics come from the runtime RE (docs/knowledge/particle-runtime.md),
/// nif.xml field docs and NifSkope, in that order of trust — every tooltip
/// names its source in plain words. Generated from the four-agent node
/// parameter audit (2026-08-06) and hand-merged with the RE range table.
#[rustfmt::skip]
/// Body sections for a kind's parameters, in display order: the node
/// shows the PRIMARY params (everything not listed here) always, and
/// each named section as a collapsed group — progressive disclosure
/// without hiding anything behind a different panel. One table, one
/// place: the UI, the layout height estimate and the section test all
/// derive from it. Keys must exist in the kind's schema — the
/// sections test pins every entry against the real param list.
/// The effect shader's float variables the node models, as
/// (parameter, variable), in nif.xml's `EffectShaderControlledVariable`
/// numbering: 0 Emissive Multiple, 5 Alpha Transparency, 6 U Offset,
/// 7 U Scale, 8 V Offset, 9 V Scale. The ONE table the clip reader, the
/// decompiler, the compiler, clip binding and the preview all read, so a
/// variable added here reaches every one of them. UV scroll is ~6% of
/// all clip tracks in the corpus's behavior-bound particle files.
pub const EFFECT_SHADER_FLOAT_KEYS: &[(&str, u32)] = &[
    ("Emissive Scale Keys", 0),
    ("Alpha Keys", 5),
    ("U Offset Keys", 6),
    ("U Scale Keys", 7),
    ("V Offset Keys", 8),
    ("V Scale Keys", 9),
];

pub fn param_sections(kind: NodeKind) -> &'static [(&'static str, &'static [&'static str])] {
    use NodeKind as K;
    match kind {
        K::EffectShader => &[
            (
                "Keys",
                &[
                    "Emissive Scale Keys",
                    "Alpha Keys",
                    "Color Keys",
                    "U Offset Keys",
                    "U Scale Keys",
                    "V Offset Keys",
                    "V Scale Keys",
                ],
            ),
            (
                "Falloff",
                &[
                    "Use Falloff",
                    "Falloff Start Angle",
                    "Falloff Stop Angle",
                    "Falloff Start Opacity",
                    "Falloff Stop Opacity",
                ],
            ),
            ("Soft", &["Soft Effect", "Soft Falloff Depth"]),
            (
                "UV",
                &[
                    "UV Offset U",
                    "UV Offset V",
                    "UV Scale U",
                    "UV Scale V",
                    "Texture Clamp",
                ],
            ),
            (
                "Greyscale",
                &["Greyscale Texture", "Greyscale Color", "Greyscale Alpha"],
            ),
            ("Lighting", &["Double Sided", "Lighting Influence"]),
        ],
        K::ParticleData => &[
            (
                "Subtexture",
                &["Subtexture Rows", "Subtexture Cols", "Atlas Offsets"],
            ),
            (
                "Aspect",
                &[
                    "Aspect Ratio",
                    "Velocity Aspect",
                    "Rotate To Velocity",
                    "Speed To Aspect",
                    "Aspect 2",
                    "Aspect Speed 1",
                    "Aspect Speed 2",
                ],
            ),
            (
                "Strips",
                &[
                    "Strip Trails",
                    "Max Point Count",
                    "Start Cap Size",
                    "End Cap Size",
                    "Z Prepass",
                    "Update Delta Time",
                ],
            ),
        ],
        K::ParticleSystem => &[(
            "World Transform",
            &["World Scale", "World Offset", "World Rotation"],
        )],
        // The sphere is rotation-invariant, so it has no Emitter
        // Rotation param at all -- the sections test catches a key
        // that names nothing.
        K::BoxEmitter | K::CylinderEmitter => &[(
            "Anchor",
            &["Emitter Offset", "Emitter Rotation"],
        )],
        K::SphereEmitter => &[("Anchor", &["Emitter Offset"])],
        // The whole flag word is here, grouped so the three rows an
        // artist touches stay primary and the rest is one click away.
        // Not hidden: every bit of this word is authorable, and the
        // corpus holds a mod that uses factors no preset spells.
        K::AlphaProperty => &[
            (
                "Custom blending",
                &["Blending", "Source Blend", "Destination Blend"],
            ),
            ("Test", &["Test Function"]),
            ("Bethesda bits", &["Clone Unique", "Editor Alpha Threshold"]),
        ],
        _ => &[],
    }
}

/// The section a parameter renders under, or `None` for the primary
/// (always visible) rows.
pub fn param_section(kind: NodeKind, key: &str) -> Option<&'static str> {
    param_sections(kind)
        .iter()
        .find(|(_, keys)| keys.contains(&key))
        .map(|(name, _)| *name)
}

/// The switch a parameter depends on: `(key, why)` when `key` has to
/// be on for this parameter to mean anything.
///
/// One table, beside the schema, because more than one surface needs
/// the same answer: the widget greys itself out and says why, and the
/// generated reference can print the condition in the parameter's own
/// row. What it encodes is the FORMAT's rule, not a preference -- the
/// strip parameters live in `BSStripPSysData`, and with `Strip Trails`
/// off the export writes a plain `NiPSysData`, which has no field to
/// put them in. Left editable they were a control that forgot what the
/// user typed: the value could neither act nor be saved, so it came
/// back at the default on the next open.
/// A condition another parameter on the same node must meet before
/// this one is stored at all.
///
/// A control that offers what the format cannot hold is one that
/// forgets what the user typed, and this has now been measured twice:
/// the strip parameters, and a collider's dimensions, which read back
/// as the schema DEFAULT because the block they belong to is the other
/// shape.
#[derive(Clone, Copy)]
pub struct Gate {
    /// The parameter that decides it.
    pub param: &'static str,
    /// The option that parameter must hold; `None` means "the switch
    /// must be on".
    pub option: Option<&'static str>,
    /// Why, in the user's terms.
    pub why: &'static str,
}

impl Gate {
    /// Does this node meet the condition?
    pub fn met(&self, node: &GraphNode) -> bool {
        match self.option {
            None => node.bool_or(self.param, false),
            Some(want) => matches!(
                node.param(self.param),
                Some(Value::Enum(opts, i)) if opts.get(*i) == Some(&want)
            ),
        }
    }

    /// How to name it in a sentence: "Strip Trails", "Shape = Sphere".
    pub fn label(&self) -> String {
        match self.option {
            None => self.param.to_string(),
            Some(want) => format!("{} = {want}", self.param),
        }
    }
}

pub fn param_requires(kind: NodeKind, key: &str) -> Option<Gate> {
    const STRIP: Gate = Gate {
        param: "Strip Trails",
        option: None,
        why: "Only with Strip Trails on: this value is stored in BSStripPSysData, and a plain particle system has no field for it.",
    };
    // A collider is written as ONE of two block types, and each carries
    // only its own dimensions: `NiPSysSphericalCollider` has a radius,
    // `NiPSysPlanarCollider` a width, a height and two axes. Measured
    // over the fixtures: editing the wrong one reads back as the schema
    // default, because the field is not in the file at all.
    const SPHERE: Gate = Gate {
        param: "Shape",
        option: Some("Sphere"),
        why: "Only for a Sphere: the radius is a field of NiPSysSphericalCollider, and a plane has no room for it.",
    };
    const PLANE: Gate = Gate {
        param: "Shape",
        option: Some("Plane"),
        why: "Only for a Plane: width, height and the two axes are fields of NiPSysPlanarCollider, and a sphere has no room for them.",
    };
    // The three preset blend halves are exact flag words, so while one
    // of them is chosen the enable bit and both factors are DERIVED and
    // a value typed here would be overwritten on export. Same shape as
    // the two gates above -- a control that cannot store what you type.
    const CUSTOM_BLEND: Gate = Gate {
        param: "Blend",
        option: Some("Custom"),
        why: "Only with Blend = Custom: the other three presets are fixed flag words, so they set the enable bit and both factors themselves.",
    };
    match (kind, key) {
        (
            NodeKind::ParticleData,
            "Max Point Count" | "Start Cap Size" | "End Cap Size" | "Z Prepass"
            | "Update Delta Time",
        ) => Some(STRIP),
        (NodeKind::ColliderModifier, "Radius") => Some(SPHERE),
        (NodeKind::ColliderModifier, "Width" | "Height" | "X Axis" | "Y Axis") => Some(PLANE),
        (
            NodeKind::AlphaProperty,
            "Blending" | "Source Blend" | "Destination Blend",
        ) => Some(CUSTOM_BLEND),
        _ => None,
    }
}

/// The heading a choice sits under, when a parameter's options are
/// worth grouping.
///
/// A flat list of twelve reads as twelve unrelated things; the reader
/// then has to know already that `Speed` means an emitter and
/// `Gravity Strength` means a Gravity node, which is exactly what they
/// came to the picker to find out. Positional with the option list,
/// like every other table paired with one here.
///
/// Display order follows the GROUPS, not the option indices, so a
/// group's members need not be contiguous -- the index is what the
/// compiler pairs against `PARAM_CTLR_BLOCKS`, and reordering two
/// hand-paired tables to please a menu is how such a pairing rots.
pub fn param_option_group(kind: NodeKind, key: &str, index: usize) -> Option<&'static str> {
    match (kind, key) {
        (NodeKind::ParamCtlr, "Parameter") => PARAM_TARGET_OWNERS.get(index).copied(),
        _ => None,
    }
}

/// The NIF block type a choice makes the export write, where the
/// choice is really a decision about the block's TYPE.
///
/// Worth saying out loud for exactly these: the control looks like a
/// setting and is a fork in what lands in the file, which is the one
/// thing a node body otherwise never shows. Named against the typed
/// views, so a typo fails `every_named_block_type_exists` rather than
/// reaching a hover.
pub fn param_option_writes(kind: NodeKind, key: &str, index: usize) -> Option<&'static str> {
    match (kind, key) {
        (NodeKind::ParamCtlr, "Parameter") => PARAM_CTLR_BLOCKS.get(index).copied(),
        // The two collider block types, which is also why their
        // dimension parameters are gated against each other.
        (NodeKind::ColliderModifier, "Shape") => {
            ["NiPSysSphericalCollider", "NiPSysPlanarCollider"]
                .get(index)
                .copied()
        }
        _ => None,
    }
}

/// What ONE option of a choice means, for its row in the picker.
///
/// The parameter's own tooltip explains the choice; this explains each
/// answer, at the moment the reader is deciding between them. Russian
/// lives in `graph_ru::param_option_note_ru`, and a note without its
/// Russian line fails `ru_corpus_is_complete`.
pub fn param_option_note(kind: NodeKind, key: &str, index: usize) -> Option<&'static str> {
    use NodeKind as K;
    Some(match (kind, key, index) {
        (K::AlphaProperty, "Blend", 0) => "SRC_ALPHA -> ONE (0x000D): adds its light to what is behind it. Black is invisible and draw order stops mattering -- fire, sparks, magic glows.",
        (K::AlphaProperty, "Blend", 1) => "SRC_ALPHA -> INV_SRC_ALPHA (0x00ED): ordinary transparency, covering what is behind in proportion to alpha -- smoke, dust, mist. Sorted back to front, so overlapping systems can pop.",
        (K::AlphaProperty, "Blend", 2) => "Blending off (0x0000): the shape draws solid. The alpha test still works, for crisp cut-out edges.",
        (K::AlphaProperty, "Blend", 3) => "Set the enable bit and both factors yourself, below. For any word the three presets cannot spell; vanilla SSE never needs it on a particle system.",

        // The factors. The same list serves both sides; Destination
        // adds what the ENGINE does with its value, since that is the
        // factor the effect shader branches on (1.5.97 0x1412D5B81).
        (K::AlphaProperty, "Destination Blend", 0) => "Multiply by 1: keep what is behind at full strength and add on top. This is the value the effect shader takes its ADDITIVE path for.",
        (K::AlphaProperty, "Destination Blend", 2) => "Multiply by the incoming colour: darken what is behind by the shape's colour. The effect shader takes its MODULATING path for this value -- dirt and shadow decals.",
        (K::AlphaProperty, "Destination Blend", 7) => "Multiply by 1 - incoming alpha: what is behind fades out as the shape covers it. Ordinary transparency; the engine's default path.",
        (K::AlphaProperty, "Source Blend" | "Destination Blend", 0) => "Multiply by 1: the colour at full strength.",
        (K::AlphaProperty, "Source Blend" | "Destination Blend", 1) => "Multiply by 0: that side contributes nothing.",
        (K::AlphaProperty, "Source Blend" | "Destination Blend", 2) => "Multiply by the incoming (shape's) colour.",
        (K::AlphaProperty, "Source Blend" | "Destination Blend", 3) => "Multiply by 1 - the incoming colour.",
        (K::AlphaProperty, "Source Blend" | "Destination Blend", 4) => "Multiply by the colour already on screen.",
        (K::AlphaProperty, "Source Blend" | "Destination Blend", 5) => "Multiply by 1 - the colour already on screen.",
        (K::AlphaProperty, "Source Blend" | "Destination Blend", 6) => "Multiply by the incoming alpha. Vanilla's source factor on every particle system.",
        (K::AlphaProperty, "Source Blend" | "Destination Blend", 7) => "Multiply by 1 - the incoming alpha.",
        (K::AlphaProperty, "Source Blend" | "Destination Blend", 8) => "Multiply by the alpha already on screen.",
        (K::AlphaProperty, "Source Blend" | "Destination Blend", 9) => "Multiply by 1 - the alpha already on screen.",
        (K::AlphaProperty, "Source Blend" | "Destination Blend", 10) => "Multiply by min(incoming alpha, 1 - screen alpha), saturated.",
        (K::AlphaProperty, "Source Blend" | "Destination Blend", 11..=15) => "Not a defined factor: the field is four bits wide and nif.xml names eleven values. Listed so a file that carries one keeps it; it does nothing useful.",

        // Every one of these is stored and none is read: the engine never
        // extracts bits 10-12 and its shaders compare GREATER.
        (K::AlphaProperty, "Test Function", 0) => "Keep every pixel. Stored only -- the game always compares GREATER.",
        (K::AlphaProperty, "Test Function", 1) => "Keep pixels whose alpha is below the threshold. Stored only -- the game always compares GREATER.",
        (K::AlphaProperty, "Test Function", 2) => "Keep pixels whose alpha equals the threshold. Stored only -- the game always compares GREATER.",
        (K::AlphaProperty, "Test Function", 3) => "Keep pixels at or below the threshold. Stored only -- the game always compares GREATER.",
        (K::AlphaProperty, "Test Function", 4) => "Keep pixels whose alpha is above the threshold. What vanilla writes, and the only comparison the game performs.",
        (K::AlphaProperty, "Test Function", 5) => "Keep pixels whose alpha differs from the threshold. Stored only -- the game always compares GREATER.",
        (K::AlphaProperty, "Test Function", 6) => "Keep pixels at or above the threshold. Stored only -- the game always compares GREATER.",
        (K::AlphaProperty, "Test Function", 7) => "Keep no pixel. Stored only -- the game always compares GREATER.",
        _ => return None,
    })
}

/// The `NiAlphaProperty` flag word this node describes.
///
/// The ONE encoder. The compiler writes it, the preview reads its
/// blend mode out of it, and [`set_alpha_flags`] is its exact inverse
/// -- which is what lets a null edit stay byte-identical without the
/// compiler having to copy the source's word back. Every one of the
/// sixteen bits is a parameter, so nothing here is approximated.
pub fn alpha_flags(node: &GraphNode) -> u16 {
    let mut flags = match node.enum_index("Blend") {
        0 => 0x000D, // blend on, SRC_ALPHA -> ONE
        1 => 0x00ED, // blend on, SRC_ALPHA -> INV_SRC_ALPHA
        2 => 0x0000, // blending off
        _ => {
            let src = (node.enum_index("Source Blend") as u16) & 0xF;
            let dst = (node.enum_index("Destination Blend") as u16) & 0xF;
            (node.bool_or("Blending", true) as u16) | (src << 1) | (dst << 5)
        }
    };
    if node.bool_or("Alpha Test", false) {
        flags |= 1 << 9;
    }
    flags |= ((node.enum_index("Test Function") as u16) & 7) << 10;
    if !node.bool_or("Sort Particles", true) {
        flags |= 1 << 13;
    }
    if node.bool_or("Clone Unique", false) {
        flags |= 1 << 14;
    }
    if node.bool_or("Editor Alpha Threshold", false) {
        flags |= 1 << 15;
    }
    flags
}

/// Read a flag word into the node's parameters -- the exact inverse of
/// [`alpha_flags`], pinned by an exhaustive test over all 65,536 words.
///
/// A blend half that is one of the three preset words takes that
/// preset, so the ordinary file opens on the control an artist wants;
/// anything else is `Custom` with the bits shown as they are.
pub fn set_alpha_flags(node: &mut GraphNode, flags: u16) {
    let preset = match flags & 0x01FF {
        0x000D => 0,
        0x00ED => 1,
        0x0000 => 2,
        _ => 3,
    };
    node.set_param("Blend", Value::Enum(BLEND_PRESETS, preset));
    node.set_param("Blending", Value::Bool(flags & 1 != 0));
    node.set_param(
        "Source Blend",
        Value::Enum(ALPHA_FUNCTIONS, ((flags >> 1) & 0xF) as usize),
    );
    node.set_param(
        "Destination Blend",
        Value::Enum(ALPHA_FUNCTIONS, ((flags >> 5) & 0xF) as usize),
    );
    node.set_param("Alpha Test", Value::Bool(flags & (1 << 9) != 0));
    node.set_param(
        "Test Function",
        Value::Enum(TEST_FUNCTIONS, ((flags >> 10) & 7) as usize),
    );
    node.set_param("Sort Particles", Value::Bool(flags & (1 << 13) == 0));
    node.set_param("Clone Unique", Value::Bool(flags & (1 << 14) != 0));
    node.set_param("Editor Alpha Threshold", Value::Bool(flags & (1 << 15) != 0));
}

/// What to say in the body of a node that has NO parameters at all.
///
/// An empty box reads as broken rather than as deliberate -- it was
/// reported as exactly that. The reason is per KIND and belongs beside
/// the schema it explains, so it lives here; a kind that grows one is
/// caught by `every_paramless_kind_says_why`.
pub fn no_params_note(kind: NodeKind) -> Option<&'static str> {
    match kind {
        // Measured, both ways. In the FORMAT it is a bare
        // `NiTimeController` with no fields of its own. In the RUNTIME
        // even the inherited ones are dead: `NiPSysUpdateCtlr::Update`
        // (1.5.97 0x140D0BA30) opens by forcing the cycle type to
        // CLAMP and overwriting the key range with (-FLT_MAX,
        // +FLT_MAX), so start, stop and loop cannot survive one frame.
        // The vanilla install agrees -- 1,729 blocks, frequency 1.0,
        // phase 0, start 0, stop 0, no chain, on every one.
        NodeKind::UpdateCtlr => Some(
            "No settings: the game overwrites this controller's timing every frame. \
             Its presence is what matters — without it the system never simulates.",
        ),
        _ => None,
    }
}

/// Parameters that are not the node's own value at all: they are the
/// transform of a SCENE NODE the file already has.
///
/// A force, an emitter volume and a system are all PLACED by a node,
/// and the graph mirrors that node's position so the simulation and
/// the UI can work in one space. It does not own it — and the scene is
/// the one thing this editor deliberately does not model, so an edit
/// here has nowhere to go: the splice takes a paired block's Ptrs from
/// the source, and rewriting a real scene node's transform is what
/// once renamed "PSmoke-Emitter" and deleted its children.
///
/// **Measured over the fixtures: 191 of 239 anchors serve MORE THAN
/// ONE modifier**, and every one of the 93 Drag anchors does — the
/// campfire's three drags all stand on `Drag01`. So even writing the
/// node would be wrong: it would move forces the user did not touch.
///
/// Returns where to do it instead. The answer is not "you cannot" —
/// the viewport gizmo and the Scene panel move the node itself, and
/// that is the thing the user means.
///
/// Only for a node that CAME FROM A FILE. Authored from scratch there
/// is no scene node yet, the compiler synthesizes one from this very
/// value, and the field is the author's to set — which is why the
/// rule is asked of a node rather than of the schema alone.
pub fn param_scene_owned(kind: NodeKind, key: &str) -> Option<&'static str> {
    use NodeKind as K;
    const ANCHOR: &str = "This is the position of the scene node this is PLACED BY, not a value of its own — and that node is usually shared: measured over the fixtures, 191 of 239 anchors serve more than one modifier and every Drag anchor does, so writing it would move forces you never touched. Move the node itself: select it in the Scene panel or drag it with the viewport gizmo. (16 of those 239 have no anchor object at all; the file keeps their empty pointer, so this cannot be typed in for them either.)";
    const SYSTEM: &str = "This is the transform of the scene node the system hangs under, not a value of its own. Move the node itself: select it in the Scene panel or drag it with the viewport gizmo.";
    match (kind, key) {
        (K::GravityModifier, "Gravity Offset")
        | (K::DragModifier, "Drag Offset")
        | (K::BombModifier, "Bomb Offset")
        | (K::ColliderModifier, "Collider Offset" | "Collider Rotation")
        | (K::InheritVelocityModifier, "Inherit Offset")
        | (K::RecycleBoundModifier, "Anchor Offset")
        | (
            K::BoxEmitter | K::SphereEmitter | K::CylinderEmitter,
            "Emitter Offset" | "Emitter Rotation",
        ) => Some(ANCHOR),
        (K::ParticleSystem, "World Offset" | "World Rotation" | "World Scale") => Some(SYSTEM),
        _ => None,
    }
}

pub fn param_meta(kind: NodeKind, key: &str) -> Option<ParamMeta> {
    use NodeKind as K;
    Some(match (kind, key) {
        // --- fields added by the parameter audit (2026-08-06) -----------
        (K::BoxEmitter | K::CylinderEmitter, "Emitter Rotation") => ParamMeta {
            label: None,
            tooltip: "Orientation of the emitter volume as X/Y/Z euler radians — the runtime rotates spawn positions and launch velocities by the anchor node's rotation (1.5.97 spawn code). A sideways cylinder makes a horizontal spark beam. Export bakes it into the anchor node.",
            min: None,
            max: None,
        },
        (K::ParticleData, "Aspect Ratio") => ParamMeta {
            label: None,
            tooltip: "Width-to-height ratio of every particle quad (per the NIF format docs it shapes the subtexture UV quads). Above 1 widens particles, below 1 makes tall streaks — rain and sparks.",
            min: Some(0.01),
            max: None,
        },
        (K::ParticleData, "Velocity Aspect") => ParamMeta {
            label: None,
            tooltip: "Orients and stretches each quad along its velocity (Aspect Flags bit 0 per nif.xml) — how vanilla does rain and flying sparks.",
            min: None,
            max: None,
        },
        (K::ParticleData, "Rotate To Velocity") => ParamMeta {
            label: None,
            tooltip: "Takes each particle's initial rotation from its velocity direction (Aspect Flags bit 1 per nif.xml).",
            min: None,
            max: None,
        },
        (K::ParticleData, "Speed To Aspect") => ParamMeta {
            label: None,
            tooltip: "Enables the speed-to-aspect ramp (Aspect Flags bit 8): faster particles stretch more, blending toward Aspect 2 between the two speed thresholds.",
            min: None,
            max: None,
        },
        (K::EmitterCtlr, "Birth Rate Keys") => ParamMeta {
            label: None,
            tooltip: "Keyed birth rate over the controller window — campfire smoke swells and dies down this way. Click the curve to edit it: drag keys, double-click empty space to add one, pick the interpolation. No keys = the constant Birth Rate. The preview plays the curve live.",
            min: None,
            max: None,
        },
        (K::EmitterCtlr, "Emitter Active Keys") => ParamMeta {
            label: None,
            tooltip: "On/off track gating emission inside the controller window — vanilla's pulsed and bursty emitters, without touching birth rate. The track STEPS: the format stores it CONST-interpolated, so a value holds until the next key rather than ramping to it. With ONE key it is a switch, not a curve, and shows as a checkbox — two thirds of vanilla's emitters are exactly that.",
            min: None,
            max: None,
        },
        (K::EffectShader, "Use Falloff") => ParamMeta {
            label: None,
            tooltip: "Master switch for the four falloff params (shader flag SLSF1 bit 6). Without it the game ignores falloff entirely — NifSkope's shader hard-codes falloff to 1.0 when the flag is clear. Enable it or the angles and opacities below are dead on arrival.",
            min: None,
            max: None,
        },
        (K::EffectShader, "Greyscale Texture") => ParamMeta {
            label: None,
            tooltip: "Palette ramp for the greyscale-to-palette trick vanilla fire is built on: the source texture's grey value picks a position in this ramp and the ramp supplies the final colour (the giant campfire's roaring flame works exactly this way). Needs the two Greyscale flags; empty = feature off.",
            min: None,
            max: None,
        },
        (K::EffectShader, "Greyscale Color") => ParamMeta {
            label: None,
            tooltip: "Recolour mode (SLSF1 bit 4): the ramp texture replaces the RGB output, looked up by the source texture's GREEN channel, brightness driven by Base Color's red component only (a quirk verified in NifSkope's shader). One greyscale fire tile becomes orange flame, blue magefire or green witchfire just by swapping ramps. Needs a Greyscale Texture set.",
            min: None,
            max: None,
        },
        (K::EffectShader, "Greyscale Alpha") => ParamMeta {
            label: None,
            tooltip: "The palette trick applied to opacity (SLSF1 bit 5): the ramp's alpha, looked up by the source texture's alpha, replaces the output alpha. Usually enabled together with Greyscale Color on vanilla fire and mist. Needs a Greyscale Texture set.",
            min: None,
            max: None,
        },
        (K::EffectShader, "UV Offset U") | (K::EffectShader, "UV Offset V") => ParamMeta {
            label: None,
            tooltip: "Shifts the texture across the effect. This is what vanilla flame animates for its endless upward scroll — float controllers target U/V Offset as controlled variables 6 and 8 (nif.xml).",
            min: None,
            max: None,
        },
        (K::EffectShader, "UV Scale U") | (K::EffectShader, "UV Scale V") => ParamMeta {
            label: None,
            tooltip: "Tiles the texture: 2.0 repeats it twice. Combine with Wrap clamp for tiling; animatable via controlled variables 7 and 9 (nif.xml).",
            min: None,
            max: None,
        },
        (K::EffectShader, "Double Sided") => ParamMeta {
            label: None,
            tooltip: "Draws both faces (SLSF2 bit 4). Camera-facing particles rarely need it, but effect meshes — flame cards, mist planes — show their backside without it.",
            min: None,
            max: None,
        },
        (K::EffectShader, "Lighting Influence") => ParamMeta {
            label: None,
            tooltip: "How strongly scene lighting affects the otherwise fully emissive effect, 0-255 (255 = full). Vanilla dust and mist darken in shadow with this. The preview does not simulate scene lights, so it only matters in game.",
            // A BYTE in the file (`BSEffectShaderProperty.lighting_influence`)
            // and the schema said nothing, so the control accepted 300 and
            // the export truncated it. The tooltip has said 0-255 since the
            // day it was written.
            min: Some(0.0),
            max: Some(255.0),
        },
        (K::AlphaProperty, "Sort Particles") => ParamMeta {
            label: None,
            tooltip: "Off writes the 'No Sorter' bit (AlphaFlags bit 13): the shape opts out of the engine's back-to-front alpha sorting, which some vanilla assets use to pin draw order between overlapping effects. Only visible in game — no viewer applies it.",
            min: None,
            max: None,
        },
        (K::AlphaProperty, "Alpha Test") => ParamMeta {
            label: None,
            tooltip: "Keeps only pixels whose alpha beats the Test Threshold and discards the rest — crisp cutout edges with zero sorting problems, the foliage-style treatment. The comparison is Test Function, though the engine ignores that field and always compares Greater. Also worth enabling alongside blending to stop nearly invisible fringe pixels from ghosting over other effects.",
            min: None,
            max: None,
        },
        (K::AlphaProperty, "Blend") => ParamMeta {
            label: None,
            tooltip: "How the effect combines with what's behind it. Additive (src-alpha → one) adds light: fire, sparks, magic glows — black pixels vanish and draw order stops mattering. Alpha Blend (src-alpha → inv-src-alpha) is normal transparency: smoke, dust — the engine depth-sorts these back to front, so overlapping systems can pop. Opaque turns blending off. Those three compile to the exact vanilla flag words (4109 / 4333 / 0) and are the only blend halves the SSE install ships. Custom unlocks the enable bit and both factors below for anything else the format can hold.",
            min: None,
            max: None,
        },
        (K::AlphaProperty, "Blending") => ParamMeta {
            label: None,
            tooltip: "AlphaFlags bit 0, the blend ENABLE. Off with factors still stored is a real vanilla shape (0x00EC, 17 blocks in the SSE install): the exporter left the factor fields behind and the game draws the shape opaque.",
            min: None,
            max: None,
        },
        (K::AlphaProperty, "Source Blend") => ParamMeta {
            label: None,
            tooltip: "AlphaFlags bits 1-4: what the shape's own colour is multiplied by. nif.xml's AlphaFunction; vanilla uses SRC_ALPHA everywhere. Values 11-15 fit the field and mean nothing — they are listed so a file that carries one survives a round trip, not because they do anything.",
            min: None,
            max: None,
        },
        (K::AlphaProperty, "Destination Blend") => ParamMeta {
            label: None,
            tooltip: "AlphaFlags bits 5-8: what the pixel already on screen is multiplied by. This is the factor the effect shader actually branches on (1.5.97 0x140D5B81): ONE picks the additive path, SRC_COLOR the modulating one, and the pair src=DST_COLOR with dst=INV_SRC_ALPHA picks a third. Anything else renders as ordinary src-alpha blending.",
            min: None,
            max: None,
        },
        (K::AlphaProperty, "Test Function") => ParamMeta {
            label: None,
            tooltip: "AlphaFlags bits 10-12, nif.xml's TestFunction. Stored faithfully and INERT in SkyrimSE 1.5.97: nothing in the binary reads these three bits, and the shaders hard-code the GREATER comparison (alpha − ref < 0 → discard). Vanilla writes GREATER on every alpha property that has the test on.",
            min: None,
            max: None,
        },
        (K::AlphaProperty, "Clone Unique") => ParamMeta {
            label: None,
            tooltip: "AlphaFlags bit 14, a Bethesda flag: the property is not shared when the object is cloned. Zero on every particle-system alpha in the corpus; it shows up on MESH alphas like weapon blood, which nif.xml notes as its usual home.",
            min: None,
            max: None,
        },
        (K::AlphaProperty, "Editor Alpha Threshold") => ParamMeta {
            label: None,
            tooltip: "AlphaFlags bit 15: nif.xml says the Test Threshold is controlled externally. Zero on every particle-system alpha in the corpus; vanilla sets it on banners and a few DLC decals.",
            min: None,
            max: None,
        },
        (K::AlphaProperty, "Test Threshold") => ParamMeta {
            label: None,
            tooltip: "Alpha cutoff 0-255: a pixel survives when its texture alpha exceeds threshold/255 (Greater test; the game compares against threshold divided by 255 per NifSkope/BodySlide). 128 is the vanilla default. Lower keeps more of the soft edge; higher shrinks the sprite. Stored as one byte in the NIF, so 255 is the ceiling. Only does anything while Alpha Test is on.",
            min: None,
            max: None,
        },
        (K::BombModifier, "Bomb Offset") => ParamMeta {
            label: None,
            tooltip: "World position of the bomb's anchor node — the centre the force radiates from (or toward, with a negative Delta V). The Zodiac sword swirls its leaves around anchors on the blade. Export recreates the anchor node.",
            min: None,
            max: None,
        },
        (K::ColliderModifier, "Active") => ParamMeta {
            label: None,
            tooltip: "The game skips inactive modifiers entirely — the Markarth warhammer ships all three of its collider managers switched off. Imported as authored.",
            min: None,
            max: None,
        },
        (K::ColliderModifier, "Shape") => ParamMeta {
            label: None,
            tooltip: "Sphere (NiPSysSphericalCollider) or bounded plane rectangle (NiPSysPlanarCollider). Spheres collide from BOTH sides — a particle inside reflects off the boundary from within, which is how a sphere contains an effect (1.5.97 RE).",
            min: None,
            max: None,
        },
        (K::ColliderModifier, "Bounce") => ParamMeta {
            label: None,
            tooltip: "Velocity kept after the reflection — the WHOLE reflected vector is scaled by this (1.5.97: v' = bounce * reflect(v)). 1 = perfectly elastic, 0 = stop dead on the surface.",
            min: Some(0.0),
            max: Some(2.0),
        },
        (K::ColliderModifier, "Die On Collide") => ParamMeta {
            label: None,
            tooltip: "Remove the particle at the hit point instead of reflecting it — kill planes and culling volumes.",
            min: None,
            max: None,
        },
        (K::ColliderModifier, "Spawn On Collide") => ParamMeta {
            label: None,
            tooltip: "Calls the wired spawn modifier on impact — but that call is a STUB in SkyrimSE 1.5.97 (same dead vfunc as spawn-on-death), so nothing spawns in game. Round-trips faithfully.",
            min: None,
            max: None,
        },
        (K::ColliderModifier, "Radius") => ParamMeta {
            label: None,
            tooltip: "Sphere shape only: collision radius in game units, scaled by the anchor node's world scale in game.",
            min: Some(0.0),
            max: None,
        },
        (K::ColliderModifier, "Width") => ParamMeta {
            label: None,
            tooltip: "Plane shape only: rectangle extent along the X axis vector. Hits outside the rectangle pass through.",
            min: Some(0.0),
            max: None,
        },
        (K::ColliderModifier, "Height") => ParamMeta {
            label: None,
            tooltip: "Plane shape only: rectangle extent along the Y axis vector.",
            min: Some(0.0),
            max: None,
        },
        (K::ColliderModifier, "X Axis") => ParamMeta {
            label: None,
            tooltip: "Plane shape only: the rectangle's first in-plane axis, in the anchor's local space. The plane normal is cross(X, Y).",
            min: None,
            max: None,
        },
        (K::ColliderModifier, "Y Axis") => ParamMeta {
            label: None,
            tooltip: "Plane shape only: the rectangle's second in-plane axis, in the anchor's local space.",
            min: None,
            max: None,
        },
        (K::ColliderModifier, "Collider Offset") => ParamMeta {
            label: None,
            tooltip: "World position of the collider's anchor node — sphere centre or plane origin. The warhammer parks a radius-1740 deflector sphere 1824 units below the head so its top surface forms a floor. Export recreates the anchor node.",
            min: None,
            max: None,
        },
        (K::ColliderModifier, "Collider Rotation") => ParamMeta {
            label: None,
            tooltip: "Orientation of the anchor node as X/Y/Z euler radians — rotates the plane axes (and nothing on spheres). Export bakes it into the anchor.",
            min: None,
            max: None,
        },
        (K::BombModifier, "Axis") => ParamMeta {
            label: None,
            tooltip: "Used only by Cylindrical and Planar symmetry: the cylinder's centre line, or the plane's push normal. Rotates with the wired Bomb Object; ignored for Spherical. From nif.xml, verified in the 1.5.97 binary.",
            min: None,
            max: None,
        },
        (K::BombModifier, "Decay") => ParamMeta {
            label: None,
            tooltip: "Working radius in game units when Decay Type is Linear or Exponential: particles beyond it are untouched (hard cutoff in the 1.5.97 binary); Linear ramps the push to zero at it, Exponential is down to ~37% there. Ignored - infinite reach - when Decay Type is None.",
            min: Some(0.0),
            max: None,
        },
        (K::BombModifier, "Decay Type") => ParamMeta {
            label: None,
            tooltip: "How the push weakens with distance: None = full strength everywhere; Linear = fades to zero at Decay; Exponential = exp(-distance/Decay) with a hard cut past Decay. Distance is measured per symmetry - from the point, the axis line, or the plane (1.5.97 binary).",
            min: None,
            max: None,
        },
        (K::BombModifier, "Delta V") => ParamMeta {
            label: None,
            tooltip: "Strength of the push in game units per second squared, applied CONTINUOUSLY every frame while the modifier is active - despite the name it is not a one-shot impulse (1.5.97 binary: velocity += direction x DeltaV x dt). Gate it with an Active Toggle or a sequence for an actual bang. Negative values implode.",
            min: None,
            max: None,
        },
        (K::BombModifier, "Symmetry") => ParamMeta {
            label: None,
            tooltip: "Shape of the blast: Spherical pushes radially away from the Bomb Object; Cylindrical pushes away from the Axis line through it; Planar pushes along the Axis, away from the plane on both sides. From nif.xml, directions verified in the 1.5.97 binary.",
            min: None,
            max: None,
        },
        (K::BoxEmitter | K::SphereEmitter | K::CylinderEmitter, "Declination") => ParamMeta {
            label: None,
            tooltip: "Launch tilt in radians measured from the emitter's up axis (+Z): 0 fires straight up, ~1.571 (π/2) fires sideways, ~3.142 (π) fires straight down. The game's angle lookup tables wrap past 2π rather than clamping, so oversized values roll around (1.5.97 runtime).",
            min: None,
            max: None,
        },
        (K::BoxEmitter | K::SphereEmitter | K::CylinderEmitter, "Declination Variation") => ParamMeta {
            label: None,
            tooltip: "Random tilt spread in radians, full range: each particle gets Declination ± this entire value (the game rolls a −1..1 factor here, unlike Speed/Life Span which only spread half). Pair ~3.142 with any declination to spray in every direction.",
            min: Some(0.0),
            max: None,
        },
        (K::BoxEmitter, "Depth") => ParamMeta {
            label: None,
            tooltip: "Full box extent along the anchor's local Z axis, in game units — the vertical dimension for an unrotated anchor (Z is up in Skyrim). Particles span ±Depth/2 around the anchor.",
            min: Some(0.0),
            max: None,
        },
        (K::BoxEmitter | K::SphereEmitter | K::CylinderEmitter, "Emitter Offset") => ParamMeta {
            label: None,
            tooltip: "World position of the emitter volume in game units. Vanilla files park emitters on a dedicated '<name>-Emitter' helper node — the giant campfire's flames sit ~117 units up. Import fills this from that node; export recreates the anchor node when it is non-zero.",
            min: None,
            max: None,
        },
        (K::BoxEmitter, "Height") => ParamMeta {
            label: None,
            tooltip: "Full box extent along the anchor's local Y axis, in game units. Careful: Skyrim is Z-up, so with an unrotated anchor this is a horizontal dimension — the vertical extent is Depth.",
            min: Some(0.0),
            max: None,
        },
        (K::BoxEmitter | K::SphereEmitter | K::CylinderEmitter | K::MeshEmitter, "Initial Color") => ParamMeta {
            label: None,
            tooltip: "RGBA tint each particle is born with, multiplied with the particle texture; alpha is the starting opacity. A Simple Color Modifier overwrites this every frame from its own stops, so it only shows on systems without one.",
            min: None,
            max: None,
        },
        (K::BoxEmitter | K::SphereEmitter | K::CylinderEmitter | K::MeshEmitter, "Initial Radius") => ParamMeta {
            label: None,
            tooltip: "Base particle size in game units — the billboard's radius at spawn (also multiplied by the system's scale). With a Scale Modifier the drawn extent is this radius × the curve value (NifSkope draws sizes·radius), so the curve rescales it rather than replacing it.",
            min: Some(0.0),
            max: None,
        },
        (K::BoxEmitter | K::SphereEmitter | K::CylinderEmitter | K::MeshEmitter, "Life Span") => ParamMeta {
            label: None,
            tooltip: "How long each particle lives, in seconds. The engine clamps it to a minimum of 1e-6, and a particle whose scheduled age already exceeds its lifespan is never created at all (1.5.97 spawn code).",
            min: Some(0.0),
            max: None,
        },
        (K::BoxEmitter | K::SphereEmitter | K::CylinderEmitter | K::MeshEmitter, "Life Span Variation") => ParamMeta {
            label: None,
            tooltip: "Random spread on Life Span in seconds — half range: particles live Life Span ± half this value (the game rolls (rand−0.5)·variation, same half-range rule as Speed).",
            min: Some(0.0),
            max: None,
        },
        (K::BoxEmitter | K::SphereEmitter | K::CylinderEmitter, "Planar Angle") => ParamMeta {
            label: None,
            tooltip: "Compass direction of the launch in radians, rotating around the up axis (0 = the emitter's local +X). Has no visible effect while Declination is 0 — a straight-up jet looks the same from every azimuth.",
            min: None,
            max: None,
        },
        (K::BoxEmitter | K::SphereEmitter | K::CylinderEmitter, "Planar Angle Variation") => ParamMeta {
            label: None,
            tooltip: "Random spread on Planar Angle in radians, full ± range (−1..1 roll in the 1.5.97 engine). ~3.142 gives an even cone all the way around — the usual setting for fountains, fire and smoke.",
            min: Some(0.0),
            max: None,
        },
        (K::BoxEmitter | K::SphereEmitter | K::CylinderEmitter | K::MeshEmitter, "Radius Variation") => ParamMeta {
            label: None,
            tooltip: "Random size spread, full range: particles spawn at Initial Radius ± this entire value (−1..1 roll per the 1.5.97 spawn code — not the half-range rule Speed uses). Keep it at or below the radius so sizes stay positive.",
            min: Some(0.0),
            max: None,
        },
        (K::BoxEmitter | K::SphereEmitter | K::CylinderEmitter, "Speed") => ParamMeta {
            label: None,
            tooltip: "Starting speed of each particle in game units per second (about 70 units to a meter), fired along the direction set by Declination and Planar Angle. Set 0 to spawn particles at rest and let gravity or wind modifiers move them.",
            min: None,
            max: None,
        },
        (K::BoxEmitter | K::SphereEmitter | K::CylinderEmitter, "Speed Variation") => ParamMeta {
            label: None,
            tooltip: "Random spread on Speed — but only half of it each way: the game rolls speed + (rand−0.5)·variation, so a value of 10 means ±5. Reverse-engineered from the 1.5.97 spawn code; note the angle and radius variations use the full range instead.",
            min: Some(0.0),
            max: None,
        },
        (K::BoxEmitter, "Width") => ParamMeta {
            label: None,
            tooltip: "Full box extent along the anchor's local X axis, in game units — particles appear uniformly within ±Width/2 of the anchor center (uniform distribution per the 1.5.97 box spawn code).",
            min: Some(0.0),
            max: None,
        },
        (K::CylinderEmitter, "Height") => ParamMeta {
            label: None,
            tooltip: "Full cylinder height in game units along the anchor's local Z axis (vertical unless the anchor node is rotated), centered on the anchor: particles span ±Height/2.",
            min: Some(0.0),
            max: None,
        },
        (K::CylinderEmitter, "Radius") => ParamMeta {
            label: None,
            tooltip: "Cylinder radius in game units, in the anchor's local X/Y plane. Like the sphere, the game picks distance as rand·radius with no area correction, so particles bunch toward the axis (1.5.97 cylinder spawn code) — vanilla effects were authored against this.",
            min: Some(0.0),
            max: None,
        },
        (K::DragModifier, "Axis") => ParamMeta {
            label: None,
            tooltip: "Drag only slows the velocity component ALONG this axis — motion across it is untouched (1.5.97 @ 0x140D150B0; it is not omnidirectional damping). Stack one drag per axis for isotropic braking, the vanilla pattern. Import bakes the drag object's rotation into this axis.",
            min: None,
            max: None,
        },
        (K::GravityModifier, "Gravity Offset") => ParamMeta {
            label: None,
            tooltip: "World position of the gravity object — the point spherical gravity pulls toward (negative Strength pushes away from it; the warhammer radiates its sparks from a node at the hammer head). Export recreates the anchor node.",
            min: None,
            max: None,
        },
        (K::DragModifier, "Drag Offset") => ParamMeta {
            label: None,
            tooltip: "World position of the drag object — the centre the Range / Range Falloff distances measure from. A drag with NO object at all does nothing in game; export always anchors one.",
            min: None,
            max: None,
        },
        (K::ParticleSystem, "World Scale") => ParamMeta {
            label: None,
            tooltip: "Accumulated scene-graph scale of this system's node. The game zeroes a world-space system's rotation/translation but KEEPS the scale product and multiplies all rendered particle positions and sizes by it (1.5.97 @ 0x140D039B0) — the Markarth warhammer runs at 0.3667, which is why its authored speed 360 reads compact in game. Export wraps the system in a scaled parent node.",
            min: Some(0.001),
            max: None,
        },
        (K::ParticleSystem, "World Offset") => ParamMeta {
            label: None,
            tooltip: "World translation of this system's node. A NON-world-space system's particles live in the node's frame, and the game renders them under the node's FULL world matrix — the aedric scepter parks nine systems on a node at (0, 50, 0) with a rotation, which is where its glow actually sits. World-space systems keep 0 here: the game zeroes their translation (1.5.97 @ 0x140D039B0). Export recreates the transformed parent.",
            min: None,
            max: None,
        },
        (K::ParticleSystem, "World Rotation") => ParamMeta {
            label: None,
            tooltip: "World rotation of this system's node, XYZ euler radians — same convention as Emitter Rotation. Applies to non-world-space systems only; the game zeroes it for world-space ones.",
            min: None,
            max: None,
        },
        (K::DragModifier, "Percentage") => ParamMeta {
            label: Some("Drag Amount"),
            tooltip: "Fraction of the along-axis velocity removed per tick; the engine scales it by 30 x frame time (authored against 30 fps ticks, so it is frame-rate independent) and caps the removal at 100%. 0.05, the vanilla default, roughly halves axis speed every half second. Verified in the 1.5.97 binary.",
            min: Some(0.0),
            max: Some(1.0),
        },
        (K::DragModifier, "Range") => ParamMeta {
            label: None,
            tooltip: "Distance from the Drag Object (game units) within which particles feel full drag. The huge default means 'everywhere'. Verified in the 1.5.97 binary.",
            min: None,
            max: None,
        },
        (K::DragModifier, "Range Falloff") => ParamMeta {
            label: None,
            tooltip: "Drag fades linearly from full at Range down to nothing at this distance; particles beyond it are untouched. Must be greater than Range to do anything. Verified in the 1.5.97 binary.",
            min: None,
            max: None,
        },
        (K::EffectShader, "Base Color") => ParamMeta {
            label: None,
            tooltip: "RGBA tint multiplied over the texture and vertex colour. Watch the alpha slider: the game's effect shader SQUARES base-colour alpha (verified in NifSkope's Skyrim effect shader), so 0.5 here shows up as 0.25 opacity in game. Animatable in sequences via BSEffectShaderPropertyColorController.",
            min: None,
            max: None,
        },
        (K::EffectShader, "Base Color Scale") => ParamMeta {
            label: None,
            tooltip: "Brightness multiplier applied to RGB only — it never touches alpha (verified in NifSkope's effect shader math). 1 = as-authored; higher values overbrighten for hot glows, and nif.xml allows up to 360. This is the 'Emissive Multiple' that float controllers animate (controlled variable 0).",
            min: Some(0.0),
            max: None,
        },
        (K::EffectShader, "Falloff Start Angle") => ParamMeta {
            label: None,
            tooltip: "Not degrees or radians — this stores the COSINE of the angle between the surface and your view (per nif.xml, confirmed in NifSkope's shader). 1.0 = facing the camera dead-on, 0.0 = seen edge-on. At this cosine the alpha multiplier equals Falloff Start Opacity. Only has any effect in game when the Use Falloff shader flag is set — see the flag note on this node.",
            min: None,
            max: None,
        },
        (K::EffectShader, "Falloff Start Opacity") => ParamMeta {
            label: None,
            tooltip: "Alpha multiplier applied when the surface faces the camera (at the Start Angle). 0-1. Start 1 / Stop 0 is the classic rim fade — the effect thins out at glancing angles; swap the two values to get a rim-only glow instead.",
            min: Some(0.0),
            max: Some(1.0),
        },
        (K::EffectShader, "Falloff Stop Angle") => ParamMeta {
            label: None,
            tooltip: "Cosine of the view angle where alpha reaches Falloff Stop Opacity (0.0 = edge-on). Between Stop and Start the game smoothsteps between the two opacities (NifSkope's Skyrim effect shader). Keep it below Falloff Start Angle.",
            min: None,
            max: None,
        },
        (K::EffectShader, "Falloff Stop Opacity") => ParamMeta {
            label: None,
            tooltip: "Alpha multiplier at the Stop Angle (usually edge-on). 0-1. With Start Opacity 1 and this at 0, edges melt away smoothly — the standard treatment for ghostly or gaseous effects.",
            min: Some(0.0),
            max: Some(1.0),
        },
        (K::EffectShader, "Emissive Scale Keys") => ParamMeta {
            label: None,
            tooltip: "Free-running BSEffectShaderPropertyFloatController on variable 0 (Emissive Multiple). No keys = no free track; sequence-driven tracks live in the sequences and are carried automatically.",
            min: None,
            max: None,
        },
        (K::EffectShader, "Alpha Keys") => ParamMeta {
            label: None,
            tooltip: "Free-running float controller on variable 5 (Alpha Transparency) - what fades falling snow in and out. Same curve editor as Birth Rate Keys, tangents included.",
            min: None,
            max: None,
        },
        (K::EffectShader, "U Offset Keys") => ParamMeta {
            label: None,
            tooltip: "Float controller on variable 6 (U Offset): slides the texture along U over time, added to UV Offset U. The texture scroll most spell and ambient clips drive - a clip usually owns this track, and then its lane is in the Curves panel under that clip.",
            min: None,
            max: None,
        },
        (K::EffectShader, "U Scale Keys") => ParamMeta {
            label: None,
            tooltip: "Float controller on variable 7 (U Scale): stretches or tiles the texture along U over time, replacing UV Scale U while it plays.",
            min: None,
            max: None,
        },
        (K::EffectShader, "V Offset Keys") => ParamMeta {
            label: None,
            tooltip: "Float controller on variable 8 (V Offset): slides the texture along V over time - the most common clip track on a particle shader after alpha (3,320 in the corpus's behavior-driven effects). A steady ramp is a steady scroll.",
            min: None,
            max: None,
        },
        (K::EffectShader, "V Scale Keys") => ParamMeta {
            label: None,
            tooltip: "Float controller on variable 9 (V Scale): stretches or tiles the texture along V over time, replacing UV Scale V while it plays.",
            min: None,
            max: None,
        },
        (K::EffectShader, "Color Keys") => ParamMeta {
            label: None,
            tooltip: "Free-running colour controller (Emissive Color): three channels, drawn red, green and blue. The illusion spell glows animate through this. On the plot a key is dragged in TIME and its components are typed below — one handle cannot mean three values.",
            min: None,
            max: None,
        },
        (K::EffectShader, "Soft Effect") => ParamMeta {
            label: None,
            tooltip: "SLSF1 Soft_Effect (0x40000000): fades the surface out as it approaches what is behind it, over Soft Falloff Depth. The bit is INDEPENDENT of the depth - vanilla ships 1,169 blocks with a depth set and this off - so it is its own switch here.",
            min: None,
            max: None,
        },
        (K::EffectShader, "Soft Falloff Depth") => ParamMeta {
            label: None,
            tooltip: "Soft-particle fade distance in game units: where the flat particle card cuts through world geometry, the game fades alpha over this depth so you don't see a hard intersection line through the ground. 0 = off; vanilla effects commonly use ~100 (the nif.xml default). Aether's exporter sets the Soft Effect shader flag automatically when this is above 0. Expect the preview to differ from the game here — neither NifSkope nor BodySlide actually implements this fade (their shader leaves the uniform unused).",
            min: Some(0.0),
            max: None,
        },
        (K::EffectShader, "Source Texture") => ParamMeta {
            label: None,
            tooltip: "The DDS drawn on every particle. Its RGB is multiplied by the vertex colour and Base Color; its alpha carries the sprite's shape. Path is Data-relative (textures\\\\...). If the file is missing the game falls back to flat white — the effect renders as a tinted square, not invisible (confirmed in NifSkope's effect-shader source).",
            min: None,
            max: None,
        },
        (K::EffectShader, "Texture Clamp") => ParamMeta {
            label: None,
            tooltip: "What happens past the texture's edges: Wrap tiles the image, Clamp stretches the last edge pixel. Wrap Both is the vanilla default (clamp byte 3 in the NIF). Use Clamp for one-shot gradients and non-tiling sprites so the opposite edge doesn't bleed through when UVs scale or scroll.",
            min: None,
            max: None,
        },
        (K::EmitterCtlr, "Birth Rate") => ParamMeta {
            label: None,
            tooltip: "Particles emitted per second. The Skyrim runtime (reverse-engineered from the 1.5.97 executable) keeps a fractional accumulator across frames, so rates below one-per-frame still emit — 0.5 means one particle every two seconds, nothing is lost to rounding. The game also scales this by its global particle-density setting. Saved as the pose value of a float interpolator; files with keyed rate tracks currently import as the key average with a warning.",
            min: Some(0.0),
            max: None,
        },
        (K::EmitterCtlr, "Loop") => ParamMeta {
            label: None,
            tooltip: "On: the controller repeats its Start-Stop window forever (written as controller flags 72, the vanilla looping value). Off: plays the window once and clamps (flags 76). These are the only two flag values vanilla particle controllers use.",
            min: None,
            max: None,
        },
        (K::EmitterCtlr, "Start Time") => ParamMeta {
            label: None,
            tooltip: "When the controller window opens, in seconds. Emission only happens inside the Start-Stop window; with Loop on, time wraps back into the window forever. Negative is meaningful and common: the window opens before t=0, so the effect is already mid-cycle when it appears.",
            // NOT `min: 0.0`. The runtime treats this as the start of a
            // window on the animation clock, never as an absolute time:
            // `NiTimeController::ComputeScaledTime` (1.5.97 0x140C6D5E0)
            // does `fmod(t - start, stop - start) + start` for LOOP and
            // clamps into [start, stop] afterwards, so any real value
            // works. Measured over the configured corpus: 3,937 of
            // 59,360 controllers ship a NEGATIVE start time -- the
            // Zodiac weapons among them, at -0.667, which is what makes
            // their effect look already-running when the weapon is
            // drawn.
            min: None,
            max: None,
        },
        (K::EmitterCtlr, "Stop Time") => ParamMeta {
            label: None,
            tooltip: "When the controller window closes, in seconds. With Loop off the emitter plays the window once and goes silent at this time — a one-shot burst (use the preview's Replay to re-run it). With Loop on this is the cycle length end.",
            min: None,
            max: None,
        },
        (K::GravityModifier, "Axis") => ParamMeta {
            label: None,
            tooltip: "Direction of the pull when Force Type is Planar; (0, 0, -1) is straight down. Ignored for Spherical, which always pulls toward the wired Gravity Object. With World Aligned on this stays a fixed world direction; off, it turns with the Gravity Object (read out of the SkyrimSE 1.5.97 update loop).",
            min: None,
            max: None,
        },
        (K::GravityModifier, "Decay") => ParamMeta {
            label: None,
            tooltip: "Distance falloff: strength is multiplied by exp(-distance x Decay), with distance measured from the Gravity Object (along the axis for Planar, straight-line for Spherical). 0 = no falloff, the vanilla norm. Small values bite hard: 0.01 already drops the force to 37% at 100 units. Verified in the 1.5.97 binary.",
            min: Some(0.0),
            max: None,
        },
        (K::GravityModifier, "Force Type") => ParamMeta {
            label: None,
            tooltip: "Planar: constant pull along the Axis everywhere, like ordinary gravity. Spherical: every particle is pulled toward the Gravity Object's position - a point attractor; give Strength a negative value to turn it into a repulsor. From nif.xml, formulas verified in the 1.5.97 binary.",
            min: None,
            max: None,
        },
        (K::GravityModifier, "Strength") => ParamMeta {
            label: None,
            tooltip: "Acceleration of the pull in game units per second squared - but the engine secretly multiplies it by 1.6, so 9.8 really accelerates at ~15.7 u/s2 (found only in the 1.5.97 binary, nothing in the file format hints at it). Negative values push the opposite way.",
            min: None,
            max: None,
        },
        (K::GravityModifier, "Turbulence") => ParamMeta {
            label: None,
            tooltip: "Random per-axis jitter added to velocity every frame. The engine multiplies Turbulence x Turbulence Scale x 500 to get the amplitude, so tiny values are normal - think 0.01-0.1 for drifting smoke. Reverse-engineered from the 1.5.97 binary.",
            min: Some(0.0),
            max: None,
        },
        (K::GravityModifier, "Turbulence Scale") => ParamMeta {
            label: None,
            tooltip: "Extra multiplier on Turbulence (effective jitter = Turbulence x Scale x 500, per the 1.5.97 binary). Usually left at 1 with Turbulence as the single knob.",
            min: None,
            max: None,
        },
        (K::GravityModifier, "World Aligned") => ParamMeta {
            label: None,
            tooltip: "On (the vanilla default): the Axis is a fixed world direction - (0, 0, -1) stays 'down' no matter how the effect is rotated. Off: the Axis rides the Gravity Object's rotation and turns as it turns. Verified in the 1.5.97 update loop.",
            min: None,
            max: None,
        },
        (K::LodModifier, "End Emit Scale") => ParamMeta {
            label: None,
            tooltip: "Birth-rate multiplier once the model is at or past End distance - 0.2 means one fifth as many particles far away; 1.0 never thins. Ramps linearly from 1 between Begin and End. Verified in the 1.5.97 binary.",
            min: Some(0.0),
            max: Some(1.0),
        },
        (K::LodModifier, "End Size") => ParamMeta {
            label: None,
            tooltip: "Size multiplier for NEWLY spawned particles at or past End distance - the 1.5.97 spawn code multiplies each new particle's radius by it. Often set above 1 so the fewer far particles still fill the same volume.",
            min: Some(0.0),
            max: None,
        },
        (K::LodModifier, "LOD Begin Distance") => ParamMeta {
            label: None,
            tooltip: "Where the distance-thinning starts. NOT game units: the engine compares it against the model's normalized fade measure (0 = close, full detail; rising toward 1 as the model fades out) - which is why vanilla values are fractions like 0.1. Closer than this the system runs at 100%. Read out of the 1.5.97 binary.",
            min: Some(0.0),
            max: None,
        },
        (K::LodModifier, "LOD End Distance") => ParamMeta {
            label: None,
            tooltip: "Where the thinning bottoms out on the same 0-1 fade scale; the ramp from Begin to End is linear, and past End the multipliers stay clamped at the End values (1.5.97 binary). Models flagged Ignore Fade skip this modifier entirely.",
            min: Some(0.0),
            max: None,
        },
        (K::MeshEmitter, "Declination") => ParamMeta {
            label: None,
            tooltip: "Not used by mesh emitters: the launch direction comes from Velocity Type (normals/random/axis), so this tilt angle is ignored. It only matters on box, sphere and cylinder emitters; kept because the underlying NiPSysEmitter block still stores it (nif.xml).",
            min: None,
            max: None,
        },
        (K::MeshEmitter, "Declination Variation") => ParamMeta {
            label: None,
            tooltip: "Not used by mesh emitters — the mesh owns the launch direction (Velocity Type). Kept because the underlying NiPSysEmitter block still stores it (nif.xml).",
            min: Some(0.0),
            max: None,
        },
        (K::MeshEmitter, "Emission Axis") => ParamMeta {
            label: None,
            tooltip: "Direction vector used only when Velocity Type is Use Direction — every particle launches along it at Speed (nif.xml: 'the emission axis if VELOCITY_USE_DIRECTION'). Ignored for Use Normals and Random.",
            min: None,
            max: None,
        },
        (K::MeshEmitter, "Emission Type") => ParamMeta {
            label: None,
            tooltip: "Which points of the mesh spawn particles (nif.xml EmitFrom): exact vertices, face centers, edge centers, or random points across the faces/edges. Face Surface (the default) is the usual even spread; Aether's preview currently samples vertices, face centers and face surfaces.",
            min: None,
            max: None,
        },
        (K::MeshEmitter, "Emitter Meshes") => ParamMeta {
            label: None,
            tooltip: "Comma-separated names of shapes in the imported model whose geometry emits particles. Hidden shapes count — vanilla rigs deliberately use invisible Emit* meshes for this (the Morthal sword's leaf stream). Exports write the named shapes back; a name that matches nothing warns at export and emits nothing in game.",
            min: None,
            max: None,
        },
        (K::MeshEmitter, "Planar Angle") => ParamMeta {
            label: None,
            tooltip: "Not used by mesh emitters — the mesh owns the launch direction (Velocity Type). Kept because the underlying NiPSysEmitter block still stores it (nif.xml).",
            min: None,
            max: None,
        },
        (K::MeshEmitter, "Planar Angle Variation") => ParamMeta {
            label: None,
            tooltip: "Not used by mesh emitters — the mesh owns the launch direction (Velocity Type). Kept because the underlying NiPSysEmitter block still stores it (nif.xml).",
            min: Some(0.0),
            max: None,
        },
        (K::MeshEmitter, "Speed") => ParamMeta {
            label: None,
            tooltip: "Starting speed of each particle in game units per second (about 70 units to a meter), along the direction chosen by Velocity Type (surface normal, random, or the Emission Axis). Set 0 to spawn particles at rest on the surface.",
            min: None,
            max: None,
        },
        (K::MeshEmitter, "Speed Variation") => ParamMeta {
            label: None,
            tooltip: "Random spread on Speed — but only half of it each way: the game rolls speed + (rand−0.5)·variation, so a value of 10 means ±5. Reverse-engineered from the 1.5.97 spawn code.",
            min: Some(0.0),
            max: None,
        },
        (K::MeshEmitter, "Velocity Type") => ParamMeta {
            label: None,
            tooltip: "How each particle picks its launch direction (nif.xml VelocityType): Use Normals fires along the surface normal at the sampled point (sparks off a blade), Random scatters in all directions, Use Direction fires everything along the Emission Axis. Declination/Planar Angle are ignored on mesh emitters.",
            min: None,
            max: None,
        },
        (K::ParamCtlr, "Parameter") => ParamMeta {
            label: None,
            tooltip: "Which knob the track turns — the NIF block type carries the choice (NiPSysEmitterSpeedCtlr, NiPSysGravityStrengthCtlr, ...). Emitter parameters bake into particles at spawn, so the track changes NEW particles; Gravity Strength is read live every update and bites everything at once (RE 0x140D190C0).",
            min: None,
            max: None,
        },
        (K::ParamCtlr, "Keys") => ParamMeta {
            label: None,
            tooltip: "A keyed float, sampled at controller-local time by the track's OWN interpolation and written RAW into the target (RE 0x140D27390 — no scaling, no clamp, no restore when the controller stops). Empty when the track is sequence-managed: the keys then live in the sequence, and a free-running copy here would fight the player.",
            min: None,
            max: None,
        },
        (K::ParamCtlr, "Start Time") => ParamMeta {
            label: None,
            tooltip: "Controller window start, seconds. The looping clock wraps inside [Start, Stop].",
            min: None,
            max: None,
        },
        (K::ParamCtlr, "Stop Time") => ParamMeta {
            label: None,
            tooltip: "Controller window end, seconds.",
            min: None,
            max: None,
        },
        (K::ParamCtlr, "Loop") => ParamMeta {
            label: None,
            tooltip: "Wrap the clock at Stop Time (controller flags 72) instead of clamping there (76).",
            min: None,
            max: None,
        },
        (K::VisibilityCtlr, "Keys") => ParamMeta {
            label: None,
            tooltip: "A stepped on/off track. Hidden systems keep SIMULATING — only the drawing stops, exactly like the game's hidden flag.",
            min: None,
            max: None,
        },
        (K::VisibilityCtlr, "Start Time") => ParamMeta {
            label: None,
            tooltip: "Controller window start, seconds.",
            min: None,
            max: None,
        },
        (K::VisibilityCtlr, "Stop Time") => ParamMeta {
            label: None,
            tooltip: "Controller window end, seconds.",
            min: None,
            max: None,
        },
        (K::VisibilityCtlr, "Loop") => ParamMeta {
            label: None,
            tooltip: "Wrap the clock at Stop Time instead of clamping there.",
            min: None,
            max: None,
        },
        (K::MultiEmitterCtlr, "Birth Rate") => ParamMeta {
            label: None,
            tooltip: "Particles per second when the keys are empty — the float interpolator's pose value.",
            min: Some(0.0),
            max: None,
        },
        (K::MultiEmitterCtlr, "Birth Rate Keys") => ParamMeta {
            label: None,
            tooltip: "Keyed birth rate. This controller's clock is ALWAYS cyclic in game (RE 0x140D582F0 computes its own fmod wrap), and it never consults an active track.",
            min: None,
            max: None,
        },
        (K::MultiEmitterCtlr, "Max Emitters") => ParamMeta {
            label: None,
            tooltip: "How many placed anchors emit at once, nearest to the camera first (RE 0x140D5E430). In game every reference whose addon node points at this master is an anchor; the preview has one, which the game's own arithmetic reduces to a plain emitter controller.",
            min: Some(1.0),
            max: Some(100.0),
        },
        (K::MultiEmitterCtlr, "Start Time") => ParamMeta {
            label: None,
            tooltip: "Controller window start, seconds.",
            min: None,
            max: None,
        },
        (K::MultiEmitterCtlr, "Stop Time") => ParamMeta {
            label: None,
            tooltip: "Controller window end, seconds.",
            min: None,
            max: None,
        },
        (K::MultiEmitterCtlr, "Loop") => ParamMeta {
            label: None,
            tooltip: "Kept for the controller flags on export; the game's own update wraps regardless.",
            min: None,
            max: None,
        },
        (K::InheritVelocityModifier, "Chance To Inherit") => ParamMeta {
            label: None,
            tooltip: "Per-spawn odds, percent: rand() × 100 < Chance, strict (RE 0x140D269E0). 100 = every particle.",
            min: Some(0.0),
            max: Some(100.0),
        },
        (K::InheritVelocityModifier, "Velocity Multiplier") => ParamMeta {
            label: None,
            tooltip: "How much of the anchor node's world velocity the particle takes at spawn. The storm atronach's cloak uses 1.0 — the swirl moves with the atronach.",
            min: None,
            max: None,
        },
        (K::InheritVelocityModifier, "Velocity Variation") => ParamMeta {
            label: None,
            tooltip: "RELATIVE spread: the multiplier is drawn uniformly from [max(0, M − M·V), M + M·V] — variation 0.4 on multiplier 0.5 gives 0.3..0.7, never negative (RE 0x140D269E0).",
            min: Some(0.0),
            max: None,
        },
        (K::InheritVelocityModifier, "Inherit Offset") => ParamMeta {
            label: None,
            tooltip: "Where the anchor node sits, psys-relative. Its POSITION does not matter to the effect — only its motion does — but the file stores the anchor and round-trips it.",
            min: None,
            max: None,
        },
        (K::InheritVelocityModifier, "Inherit Node") => ParamMeta {
            label: None,
            tooltip: "The scene node whose motion is inherited, by name (set on import). The preview finite-differences its world position per frame — play a sequence that moves it to see the effect; a static node inherits zero, exactly like the game.",
            min: None,
            max: None,
        },
        (K::RecycleBoundModifier, "Bound Offset") => ParamMeta {
            label: None,
            tooltip: "Offset from the anchor to the box's MIN corner — the game anchors the corner there, not the centre (RE 0x140D25DE0).",
            min: None,
            max: None,
        },
        (K::RecycleBoundModifier, "Bound Extent") => ParamMeta {
            label: None,
            tooltip: "Box size per axis. Particles wrap toroidally into [corner, corner + extent): out the top, in the bottom. Velocity, age and trails are untouched — blowing fog loops through a room this way.",
            min: Some(0.0),
            max: None,
        },
        (K::RecycleBoundModifier, "Anchor Offset") => ParamMeta {
            label: None,
            tooltip: "Where the bound's anchor node sits, psys-relative. The corner = anchor + Bound Offset.",
            min: None,
            max: None,
        },
        (K::WindModifier, "Strength") => ParamMeta {
            label: None,
            tooltip: "Multiplies the WEATHER's wind: acceleration = windSpeed × 100 × Strength units/s² on the horizontal plane (RE 0x140D3D410). Indoors the game forces the wind to zero, so this does nothing there. The preview's stand-in wind lives on the toolbar.",
            min: None,
            max: None,
        },
        (K::ModifierActiveCtlr, "Keys") => ParamMeta {
            label: None,
            tooltip: "On/off switch track, times in seconds within the controller window. The value holds until the next key — switches are instant, not blended, because the format stores this CONST-interpolated (9,771 of 9,771 vanilla NiBoolData blocks). An empty track exports as always-on. The vanilla campfire pulses its smoke drag this way for gusts. Exports as a NiBoolData key timeline.",
            min: None,
            max: None,
        },
        (K::ModifierActiveCtlr, "Loop") => ParamMeta {
            label: None,
            tooltip: "On: repeat the key window forever (controller flags 72, the vanilla looping value). Off: play the window once and hold the last key's state (flags 76).",
            min: None,
            max: None,
        },
        (K::ModifierActiveCtlr, "Start Time") => ParamMeta {
            label: None,
            tooltip: "When the key track's window opens, in seconds. Keys are sampled in controller-local time inside the Start-Stop window.",
            min: Some(0.0),
            max: None,
        },
        (K::ModifierActiveCtlr, "Stop Time") => ParamMeta {
            label: None,
            tooltip: "When the window closes, in seconds. With Loop on the window repeats, so the key pattern cycles — a 0-1s on/off track becomes a steady 1-second pulse.",
            min: None,
            max: None,
        },
        (K::ParticleData, "Has Color") => ParamMeta {
            label: None,
            tooltip: "Allocates a per-particle color. Required for Color Over Life to have any effect — without it the only tint comes from the shader's Base Color. Costs a little memory per particle, so vanilla turns it off for plain single-color effects.",
            min: None,
            max: None,
        },
        (K::ParticleData, "Has Rotations") => ParamMeta {
            label: Some("Has Spin"),
            tooltip: "Allocates a per-particle spin angle (the file's 'Has Rotation Angles' marker — particles spin flat against the camera). Turn on when using the Rotation modifier or random initial rotation; off leaves every quad upright. Matches NifSkope's field, not the older quaternion 'Rotations' array.",
            min: None,
            max: None,
        },
        (K::ParticleData, "Max Particles") => ParamMeta {
            label: None,
            tooltip: "Hard cap on simultaneously live particles — written as the data block's vertex count, a 16-bit field (max 65535). When the buffer is full the emitter skips spawns until deaths free slots. Size it around Birth Rate x Life Span with some headroom; vanilla systems sit in the tens to low hundreds.",
            min: None,
            max: None,
        },
        (K::ParticleData, "Aspect 2") => ParamMeta {
            label: None,
            tooltip: "Speed To Aspect only: the aspect ratio reached at Aspect Speed 2 — the runtime lerps from Aspect Ratio (at Speed 1) to this value as the particle's speed rises (1.5.97 quad fill). Values below 1 stretch the quad further along its motion.",
            min: None,
            max: None,
        },
        (K::ParticleData, "Aspect Speed 1") => ParamMeta {
            label: None,
            tooltip: "Speed To Aspect only: the speed at or below which the base Aspect Ratio applies. Between the two speeds the aspect lerps toward Aspect 2.",
            min: Some(0.0),
            max: None,
        },
        (K::ParticleData, "Aspect Speed 2") => ParamMeta {
            label: None,
            tooltip: "Speed To Aspect only: the speed at or above which Aspect 2 applies fully. Vanilla streak effects use e.g. 30-60 with a small Aspect 2 for long motion trails.",
            min: Some(0.0),
            max: None,
        },
        (K::ParticleData, "Strip Trails") => ParamMeta {
            label: None,
            tooltip: "Renders each particle as a ribbon trail along its path instead of a camera-facing billboard (BSStripParticleSystem + BSStripPSysData, the weapon-trail apparatus). The preview draws the ribbons live — crossed double-strips at the game's 1/60 s commit cadence — and the exported file keeps the real strip blocks.",
            min: None,
            max: None,
        },
        (K::ParticleData, "Max Point Count") => ParamMeta {
            label: None,
            tooltip: "Strip trails only: maximum number of path points each ribbon keeps — longer trails need more points (vanilla-style content uses 28-56). Ignored when Strip Trails is off.",
            min: Some(2.0),
            max: Some(1024.0),
        },
        (K::ParticleData, "Start Cap Size") => ParamMeta {
            label: None,
            tooltip: "Strip trails only: size of the ribbon's leading end cap. 0 tapers the trail head to a point (the usual authored value).",
            min: Some(0.0),
            max: None,
        },
        (K::ParticleData, "End Cap Size") => ParamMeta {
            label: None,
            tooltip: "Strip trails only: size of the ribbon's trailing end cap. 0 tapers the tail to a point (the usual authored value).",
            min: Some(0.0),
            max: None,
        },
        (K::ParticleData, "Z Prepass") => ParamMeta {
            label: None,
            tooltip: "Strip trails only: render the ribbons in a depth prepass before the color pass, reducing self-overlap artifacts on dense trails. Off in all surveyed files.",
            min: None,
            max: None,
        },
        (K::ParticleData, "Update Delta Time") => ParamMeta {
            label: None,
            tooltip: "Strip trails only: seconds between strip path-point updates (BSPSysStripUpdateModifier, exported automatically with the strip apparatus). The stock value 0.033333 records a point every 30th of a second.",
            min: Some(0.001),
            max: Some(1.0),
        },
        (K::ParticleData, "Subtexture Cols") => ParamMeta {
            label: None,
            tooltip: "Columns of the texture atlas grid; cells are numbered left-to-right, then top-to-bottom, matching vanilla layouts (frost uses 2x2, the giant campfire's fire 4x4). The exporter generates the actual UV offset table from Rows x Cols for you.",
            min: None,
            max: None,
        },
        (K::ParticleData, "Atlas Offsets") => ParamMeta {
            label: None,
            tooltip: "Explicit subtexture quads (`U Uw V Vw; ...`), overriding the grid derived from Rows x Cols. Filled by import when the file's table is not a plain grid - 55 vanilla blocks carry a single full-texture quad. Leave empty otherwise.",
            min: None,
            max: None,
        },
        (K::ParticleData, "Subtexture Rows") => ParamMeta {
            label: None,
            tooltip: "Splits the shader's texture vertically into this many equal rows of atlas cells. Each particle picks a cell at spawn and the Subtexture modifier can step through them for flipbook animation. Rows x Cols must stay at or below 256 — the runtime frame index is a single byte, and the format docs call a 16x16 grid invalid.",
            min: None,
            max: None,
        },
        (K::ParticleSystem, "Name") => ParamMeta {
            label: None,
            tooltip: "Block name of this NiParticleSystem in the saved NIF (what NifSkope shows in the tree). Animation sequences (NiControllerManager) find their targets by this name, so keep it unique within the file and stable once sequences or other files reference it.",
            min: None,
            max: None,
        },
        (K::ParticleSystem, "World Space") => ParamMeta {
            label: None,
            tooltip: "On: particles are born into world space, so already-spawned particles stay behind when the effect's node moves — the right choice for smoke trails and embers. Off: particles live in object space and the whole plume moves rigidly with the node. Per the NIF format docs; vanilla effects almost always leave this on.",
            min: None,
            max: None,
        },
        (K::RotationModifier, "Angle Variation") => ParamMeta {
            label: None,
            tooltip: "Random spread on the starting angle, full plus-minus range in radians (1.5.97 spawn hook). 3.14159 (pi) makes the initial facing completely random - the usual choice for smoke and dust.",
            min: None,
            max: None,
        },
        (K::RotationModifier, "Axis") => ParamMeta {
            label: None,
            tooltip: "The shared rotation axis used for every particle when Random Axis is off. (0, 0, 1) spins about world Z. Only the spin component facing the camera is visible on billboard particles.",
            min: None,
            max: None,
        },
        (K::RotationModifier, "Random Axis") => ParamMeta {
            label: None,
            tooltip: "Each particle gets its own random 3D rotation axis at spawn (uniform-ish over the sphere, per the 1.5.97 spawn hook) instead of the shared Axis below. For camera-facing particles this mainly varies the apparent spin; vanilla leaves it on.",
            min: None,
            max: None,
        },
        (K::RotationModifier, "Random Speed Sign") => ParamMeta {
            label: None,
            tooltip: "Flips the spin direction on a 50/50 coin toss per particle (exact 0.5 threshold in the 1.5.97 binary). Turn it on for smoke - uniform same-direction spin reads as mechanical.",
            min: None,
            max: None,
        },
        (K::RotationModifier, "Rotation Angle") => ParamMeta {
            label: None,
            tooltip: "The sprite's starting angle at spawn, in radians. Mostly useful together with Angle Variation to avoid every particle being born upright.",
            min: None,
            max: None,
        },
        (K::RotationModifier, "Rotation Speed") => ParamMeta {
            label: None,
            tooltip: "How fast each particle's sprite spins, in radians per second (6.283 = one full turn per second) - the runtime integrates raw radians, confirmed against the 1.5.97 binary. Needs Rotations enabled on the Particle Data node or nothing spins.",
            min: None,
            max: None,
        },
        (K::RotationModifier, "Speed Variation") => ParamMeta {
            label: None,
            tooltip: "Random per-particle spread on the spin speed, covering the FULL range Speed - Variation to Speed + Variation (verified in the 1.5.97 spawn hook - unlike the emitter's Speed Variation, which is half-range). Radians per second.",
            min: Some(0.0),
            max: None,
        },
        (K::ScaleModifier, "Scales") => ParamMeta {
            label: None,
            tooltip: "Size samples spread evenly across the particle's life: first = birth, last = death, linear blend between (the engine's uniform table curve, 1.5.97 binary). The sample OVERWRITES the particle's size each frame - it is multiplied into the emitter's Initial Radius at draw time, so '0.2, 1.0, 0.4' grows then shrinks. It does not stack with other size effects.",
            min: None,
            max: None,
        },
        (K::SceneOutput, "Root Name") => ParamMeta {
            label: None,
            tooltip: "Name of the root node (a BSFadeNode, vanilla-effect style) in the exported NIF — the top entry you see in NifSkope. Purely a label; pick something recognizable so the file is easy to identify when attached in the Creation Kit or referenced by other tools.",
            min: None,
            max: None,
        },
        (K::SceneOutput, "Behavior Graph") => ParamMeta {
            label: None,
            tooltip: "The Havok behavior project that plays this effect's clips, as a path under meshes\\ (Magic\\BeginLoopEnd.hkx, GenericBehaviors\\Autoplay.hkx, ...). Written as the root's BGED extra data, which is the ONE place the game looks for it (1.5.97 0x1404E71F0). The graph decides which clip plays when: BeginLoopEnd plays mBegin, moves to mLoop on mBegin's `end` key, and plays mEnd when the spell ends. Empty = no behavior; the clips then only play if a script starts them.",
            min: None,
            max: None,
        },
        (K::SimpleColorModifier, "Color 1") => ParamMeta {
            label: None,
            tooltip: "Colour at birth. Its alpha only matters during the fade-in window, where alpha ramps from Color 1's alpha to Color 2's (alpha runs on its own stops, independent of the RGB stops - reverse-engineered from the 1.5.97 binary).",
            min: None,
            max: None,
        },
        (K::SimpleColorModifier, "Color 1 End %") => ParamMeta {
            label: None,
            tooltip: "RGB holds pure Color 1 until this fraction of life (0-1), then blends toward Color 2. Keep it at or below Color 2 Start %. Note: these labels follow the actual runtime behaviour (nifly's naming) - nif.xml's and NifSkope's stop names for this block are scrambled.",
            min: None,
            max: None,
        },
        (K::SimpleColorModifier, "Color 2") => ParamMeta {
            label: None,
            tooltip: "Mid-life colour, and the particle's core alpha between fade-in and fade-out. This is the colour your effect spends most of its life showing.",
            min: None,
            max: None,
        },
        (K::SimpleColorModifier, "Color 2 End %") => ParamMeta {
            label: None,
            tooltip: "RGB holds pure Color 2 until this fraction of life (0-1), then starts blending toward Color 3.",
            min: None,
            max: None,
        },
        (K::SimpleColorModifier, "Color 2 Start %") => ParamMeta {
            label: None,
            tooltip: "The Color 1 to Color 2 blend finishes here (0-1); from here the RGB is pure Color 2. Shown as 'Color 1 Start Percent' in NifSkope - its label, not this one, is the misleading one (runtime verified in 1.5.97).",
            min: None,
            max: None,
        },
        (K::SimpleColorModifier, "Color 3") => ParamMeta {
            label: None,
            tooltip: "Colour approaching death; its alpha is the end point of the fade-out ramp. Usually alpha 0 so particles dissolve instead of popping.",
            min: None,
            max: None,
        },
        (K::SimpleColorModifier, "Color 3 Start %") => ParamMeta {
            label: None,
            tooltip: "The Color 2 to Color 3 blend finishes here (0-1); from here to death the RGB is pure Color 3. Shown as 'Color 2 Start Percent' in NifSkope.",
            min: None,
            max: None,
        },
        (K::SimpleColorModifier, "Fade In End %") => ParamMeta {
            label: None,
            tooltip: "Alpha ramps from Color 1's alpha to Color 2's over this first fraction of the particle's life (0-1). Set it to EXACTLY 0 to switch fade-in off - the engine tests equality with zero (1.5.97 binary), which is how vanilla disables it.",
            min: None,
            max: None,
        },
        (K::SimpleColorModifier, "Fade Out Start %") => ParamMeta {
            label: None,
            tooltip: "From this fraction of life to death, alpha ramps from Color 2's alpha down to Color 3's. Set EXACTLY 0 to disable fade-out (the engine's equality-with-zero switch, 1.5.97 binary); alpha then holds Color 2's value to the end.",
            min: None,
            max: None,
        },
        (K::SpawnModifier, "Life Span") => ParamMeta {
            label: None,
            tooltip: "Lifespan handed to spawned children, seconds, in the Gamebryo spec. Dead field in SkyrimSE 1.5.97 - loaded, never read.",
            min: Some(0.0),
            max: None,
        },
        (K::SpawnModifier, "Life Span Variation") => ParamMeta {
            label: None,
            tooltip: "Random spread on the children's lifespan, seconds, in the Gamebryo spec. Dead field in SkyrimSE 1.5.97 - loaded, never read.",
            min: Some(0.0),
            max: None,
        },
        (K::SpawnModifier, "Max Num to Spawn") => ParamMeta {
            label: None,
            tooltip: "Most children per spawn event in the original Gamebryo spec. Loaded but never read by SkyrimSE 1.5.97 - kept for faithful round-trips.",
            min: None,
            max: None,
        },
        (K::SpawnModifier, "Min Num to Spawn") => ParamMeta {
            label: None,
            tooltip: "Fewest children per spawn event in the original Gamebryo spec. Loaded from the file but never read by SkyrimSE 1.5.97 (no code references) - kept for faithful round-trips.",
            min: None,
            max: None,
        },
        (K::SpawnModifier, "Percentage Spawned") => ParamMeta {
            label: None,
            tooltip: "Chance from 0 to 1 that a dying particle passes the respawn roll (nif.xml). In SkyrimSE 1.5.97 the roll still executes - and then nothing spawns; the whole spawn body is stubbed out in the engine.",
            min: Some(0.0),
            max: Some(1.0),
        },
        (K::SpawnModifier, "Spawn Dir Variation") => ParamMeta {
            label: None,
            tooltip: "Random cone around the parent's direction for spawned children in the Gamebryo spec. Dead field in SkyrimSE 1.5.97 - loaded, never read.",
            min: Some(0.0),
            max: None,
        },
        (K::SpawnModifier, "Spawn Generations") => ParamMeta {
            label: None,
            tooltip: "How many respawn generations are allowed; a particle whose generation has reached this number will not respawn (nif.xml). 0 disables spawning outright - even the engine's dice roll requires generation < this. Fresh particles are generation 0.",
            min: None,
            max: None,
        },
        (K::SpawnModifier, "Spawn On Death") => ParamMeta {
            label: None,
            tooltip: "Marks dying particles for respawning. In the exported NIF this flag actually lives on the automatic Age/Death modifier (nif.xml: NiPSysAgeDeathModifier's 'Spawn on Death'), which also points at this spawn block - Aether folds both into this node.",
            min: None,
            max: None,
        },
        (K::SpawnModifier, "Spawn Speed Variation") => ParamMeta {
            label: None,
            tooltip: "Random extra speed for spawned children in the Gamebryo spec. Dead field in SkyrimSE 1.5.97 - loaded, never read.",
            min: Some(0.0),
            max: None,
        },
        (K::SphereEmitter, "Radius") => ParamMeta {
            label: None,
            tooltip: "Sphere radius in game units. The game picks distance as rand·radius with no cube-root correction and sweeps the polar angle over a full turn, so particles bunch toward the center — deliberate 1.5.97 behavior that vanilla effects rely on, not a bug to compensate for.",
            min: Some(0.0),
            max: None,
        },
        (K::SubTexModifier, "End Frame") => ParamMeta {
            label: None,
            tooltip: "Last cell of the one-time start-to-end run; after it the animation loops over Loop Start Frame..End Frame forever. Keep it at cell count - 1 of the atlas (15 for a 4x4). From nif.xml, stepping verified in the 1.5.97 binary.",
            min: None,
            max: None,
        },
        (K::SubTexModifier, "Frame Count") => ParamMeta {
            label: None,
            tooltip: "How many frame steps a particle takes across its WHOLE lifespan - animation speed in frames per lifetime, not per second (16 frames over a 1 s lifespan = 16 fps). The engine advances at most one frame per update, so extreme counts are throttled by the game's frame rate (1.5.97 binary).",
            min: Some(0.0),
            max: None,
        },
        (K::SubTexModifier, "Frame Count Fudge") => ParamMeta {
            label: None,
            tooltip: "Random extra frames (0 up to this, additive) in each particle's budget, chosen at birth - neighbours animate at slightly different speeds. Verified in the 1.5.97 binary.",
            min: Some(0.0),
            max: None,
        },
        (K::SubTexModifier, "Loop Start Frame") => ParamMeta {
            label: None,
            tooltip: "Cell the animation returns to each time it wraps past End Frame; the loop entry is re-randomized (with its Fudge) on every wrap (1.5.97 binary). Lets you play an intro once, then cycle a sub-range.",
            min: None,
            max: None,
        },
        (K::SubTexModifier, "Loop Start Frame Fudge") => ParamMeta {
            label: None,
            tooltip: "Random extra (0 up to this, additive) on the loop entry, re-rolled at every wrap - staggers looping particles so they do not blink in unison.",
            min: Some(0.0),
            max: None,
        },
        (K::SubTexModifier, "Start Frame") => ParamMeta {
            label: None,
            tooltip: "Atlas cell a newborn particle shows first (0-based, counting left-to-right then down through the Particle Data grid). The value is truncated to a whole frame.",
            min: None,
            max: None,
        },
        (K::SubTexModifier, "Start Frame Fudge") => ParamMeta {
            label: None,
            tooltip: "Random extra added to each particle's start frame, drawn from 0 up to this value. Additive only - it never goes below Start Frame (every fudge here is value + random x fudge in the 1.5.97 binary, unlike the emitter's centred variations). Use it to de-sync particles born together.",
            min: Some(0.0),
            max: None,
        },

        _ => return None,
    })
}
fn emitter_base_params() -> Vec<(&'static str, Value)> {
    vec![
        ("Speed", Value::F32(30.0)),
        ("Speed Variation", Value::F32(0.0)),
        ("Declination", Value::F32(0.0)),
        ("Declination Variation", Value::F32(0.0)),
        ("Planar Angle", Value::F32(0.0)),
        ("Planar Angle Variation", Value::F32(0.0)),
        ("Initial Color", Value::Color4([1.0, 1.0, 1.0, 1.0])),
        ("Initial Radius", Value::F32(1.0)),
        ("Radius Variation", Value::F32(0.0)),
        ("Life Span", Value::F32(1.0)),
        ("Life Span Variation", Value::F32(0.0)),
        // NOT the anchor. `Emitter Offset` belongs to the VOLUME
        // emitters and is added by each of them below, because that is
        // where the format puts it: `emitter_object` is a field of
        // `NiPSysVolumeEmitter`, and `NiPSysMeshEmitter::sync` stops at
        // `sync_core`, one rung above it. A mesh emitter therefore has
        // nowhere to store an offset AND ignores one when spawning (its
        // sources are already world-space) -- offering the field made a
        // control that silently forgot what the user typed, reported
        // from a real session as a value that "reset itself".
    ]
}

const NO_SOCKETS: &[SocketSchema] = &[];
const OUT_SYSTEM: &[SocketSchema] = &[sock("System", SocketType::System)];
const OUT_MODIFIER: &[SocketSchema] = &[sock("Modifier", SocketType::Modifier)];
const OUT_CONTROLLER: &[SocketSchema] = &[sock("Controller", SocketType::Controller)];
const OUT_DATA: &[SocketSchema] = &[sock("Data", SocketType::Data)];
const OUT_SHADER: &[SocketSchema] = &[sock("Shader", SocketType::Shader)];
const OUT_ALPHA: &[SocketSchema] = &[sock("Alpha", SocketType::Alpha)];
const IN_SYSTEMS_MULTI: &[SocketSchema] = &[multi("Systems", SocketType::System)];
const IN_PARTICLE_SYSTEM: &[SocketSchema] = &[
    sock("Data", SocketType::Data),
    sock("Shader", SocketType::Shader),
    sock("Alpha", SocketType::Alpha),
    multi("Modifiers", SocketType::Modifier),
    multi("Controllers", SocketType::Controller),
];
const IN_EMITTER_OBJECT: &[SocketSchema] = &[sock("Emitter Object", SocketType::Object)];
const IN_GRAVITY_OBJECT: &[SocketSchema] = &[sock("Gravity Object", SocketType::Object)];
const IN_DRAG_OBJECT: &[SocketSchema] = &[sock("Drag Object", SocketType::Object)];
const IN_BOMB_OBJECT: &[SocketSchema] = &[sock("Bomb Object", SocketType::Object)];
const IN_EMITTER_CTLR: &[SocketSchema] = &[sock("Emitter", SocketType::Modifier)];
const IN_ACTIVE_CTLR: &[SocketSchema] = &[sock("Modifier", SocketType::Modifier)];

impl NodeKind {
    pub fn schema(self) -> NodeSchema {
        match self {
            NodeKind::SceneOutput => NodeSchema {
                kind: NodeKind::SceneOutput,
                id: "scene_output",
                title: "Scene Output",
                category: Category::Output,
                inputs: IN_SYSTEMS_MULTI,
                outputs: NO_SOCKETS,
                params: || {
                    vec![
                        ("Root Name", Value::Str("AetherFX".into())),
                        ("Behavior Graph", Value::Str(String::new())),
                    ]
                },
                body_params: &["Root Name", "Behavior Graph"],
                help: "Collects particle systems into the exported NIF scene root.",
            },
            NodeKind::ParticleSystem => NodeSchema {
                kind: NodeKind::ParticleSystem,
                id: "particle_system",
                title: "Particle System",
                category: Category::System,
                inputs: IN_PARTICLE_SYSTEM,
                outputs: OUT_SYSTEM,
                params: || {
                    vec![
                        ("Name", Value::Str("ParticleFX".into())),
                        ("World Space", Value::Bool(true)),
                        ("World Scale", Value::F32(1.0)),
                        ("World Offset", Value::Vec3([0.0; 3])),
                        ("World Rotation", Value::Vec3([0.0; 3])),
                    ]
                },
                body_params: &[],
                help: "NiParticleSystem — one renderable particle effect.",
            },
            NodeKind::ParticleData => NodeSchema {
                kind: NodeKind::ParticleData,
                id: "particle_data",
                title: "Particle Data",
                category: Category::Data,
                inputs: NO_SOCKETS,
                outputs: OUT_DATA,
                params: || {
                    vec![
                        ("Max Particles", Value::U32(100)),
                        ("Has Color", Value::Bool(true)),
                        ("Has Rotations", Value::Bool(false)),
                        ("Subtexture Rows", Value::U32(1)),
                        ("Atlas Offsets", Value::Str(String::new())),
                        ("Subtexture Cols", Value::U32(1)),
                        ("Aspect Ratio", Value::F32(1.0)),
                        ("Velocity Aspect", Value::Bool(false)),
                        ("Rotate To Velocity", Value::Bool(false)),
                        ("Speed To Aspect", Value::Bool(false)),
                        ("Aspect 2", Value::F32(0.0)),
                        ("Aspect Speed 1", Value::F32(0.0)),
                        ("Aspect Speed 2", Value::F32(0.0)),
                        ("Strip Trails", Value::Bool(false)),
                        ("Max Point Count", Value::U32(32)),
                        ("Start Cap Size", Value::F32(0.0)),
                        ("End Cap Size", Value::F32(0.0)),
                        ("Z Prepass", Value::Bool(false)),
                        ("Update Delta Time", Value::F32(0.033333)),
                    ]
                },
                body_params: &["Max Particles"],
                help: "NiPSysData — particle buffers, capacity and atlas layout.",
            },
            NodeKind::BoxEmitter => NodeSchema {
                kind: NodeKind::BoxEmitter,
                id: "box_emitter",
                title: "Box Emitter",
                category: Category::Emitter,
                inputs: IN_EMITTER_OBJECT,
                outputs: OUT_MODIFIER,
                params: || {
                    let mut p = emitter_base_params();
                    p.extend([
                        ("Emitter Offset", Value::Vec3([0.0, 0.0, 0.0])),
                        ("Emitter Rotation", Value::Vec3([0.0, 0.0, 0.0])),
                        ("Width", Value::F32(10.0)),
                        ("Height", Value::F32(10.0)),
                        ("Depth", Value::F32(10.0)),
                    ]);
                    p
                },
                body_params: &["Speed", "Life Span", "Width", "Height", "Depth"],
                help: "NiPSysBoxEmitter — emits from a box volume.",
            },
            NodeKind::SphereEmitter => NodeSchema {
                kind: NodeKind::SphereEmitter,
                id: "sphere_emitter",
                title: "Sphere Emitter",
                category: Category::Emitter,
                inputs: IN_EMITTER_OBJECT,
                outputs: OUT_MODIFIER,
                params: || {
                    let mut p = emitter_base_params();
                    p.extend([
                        ("Emitter Offset", Value::Vec3([0.0, 0.0, 0.0])),
                        ("Radius", Value::F32(10.0)),
                    ]);
                    p
                },
                body_params: &["Speed", "Life Span", "Radius"],
                help: "NiPSysSphereEmitter — emits from a sphere volume.",
            },
            NodeKind::CylinderEmitter => NodeSchema {
                kind: NodeKind::CylinderEmitter,
                id: "cylinder_emitter",
                title: "Cylinder Emitter",
                category: Category::Emitter,
                inputs: IN_EMITTER_OBJECT,
                outputs: OUT_MODIFIER,
                params: || {
                    let mut p = emitter_base_params();
                    p.extend([
                        ("Emitter Offset", Value::Vec3([0.0, 0.0, 0.0])),
                        ("Emitter Rotation", Value::Vec3([0.0, 0.0, 0.0])),
                        ("Radius", Value::F32(10.0)),
                        ("Height", Value::F32(10.0)),
                    ]);
                    p
                },
                body_params: &["Speed", "Life Span", "Radius", "Height"],
                help: "NiPSysCylinderEmitter — emits from a cylinder volume.",
            },
            NodeKind::MeshEmitter => NodeSchema {
                kind: NodeKind::MeshEmitter,
                id: "mesh_emitter",
                title: "Mesh Emitter",
                category: Category::Emitter,
                // No volume anchor: the sources are the named meshes.
                inputs: &[],
                outputs: OUT_MODIFIER,
                params: || {
                    let mut p = emitter_base_params();
                    p.extend([
                        ("Emitter Meshes", Value::Str(String::new())),
                        ("Velocity Type", Value::Enum(VELOCITY_TYPES, 0)),
                        ("Emission Type", Value::Enum(EMIT_FROM_TYPES, 3)),
                        ("Emission Axis", Value::Vec3([1.0, 0.0, 0.0])),
                    ]);
                    p
                },
                body_params: &["Speed", "Life Span", "Emitter Meshes"],
                help: "NiPSysMeshEmitter — emits from the surface of the named \
                       shapes of the imported model (comma-separated names). \
                       Exports write the named shapes back into the block.",
            },
            NodeKind::EmitterCtlr => NodeSchema {
                kind: NodeKind::EmitterCtlr,
                id: "emitter_ctlr",
                title: "Emitter Controller",
                category: Category::Controller,
                inputs: IN_EMITTER_CTLR,
                outputs: OUT_CONTROLLER,
                params: || {
                    vec![
                        ("Birth Rate", Value::F32(20.0)),
                        ("Birth Rate Keys", Value::Curve(Curve::empty(Track::Float))),
                        ("Emitter Active Keys", Value::Curve(Curve::constant(Track::Bool, 1.0))),
                        ("Start Time", Value::F32(0.0)),
                        ("Stop Time", Value::F32(3.0)),
                        ("Loop", Value::Bool(true)),
                    ]
                },
                body_params: &["Birth Rate", "Loop"],
                help: "NiPSysEmitterCtlr — owns Birth Rate and emitter on/off timing. \
                       Wire the emitter it controls into `Emitter`.",
            },
            NodeKind::UpdateCtlr => NodeSchema {
                kind: NodeKind::UpdateCtlr,
                id: "update_ctlr",
                title: "Update Controller",
                category: Category::Controller,
                inputs: NO_SOCKETS,
                outputs: OUT_CONTROLLER,
                params: no_params,
                body_params: &[],
                help: "NiPSysUpdateCtlr — required for the system to advance in game. \
                       It has no settings, and that is measured: in the format it is \
                       a bare NiTimeController with no fields of its own, and in the \
                       runtime NiPSysUpdateCtlr::Update (1.5.97 0x140D0BA30) opens by \
                       forcing the cycle type to CLAMP and overwriting the key range \
                       with (-FLT_MAX, +FLT_MAX), so Start Time, Stop Time and Loop \
                       cannot survive one frame. The vanilla install agrees: 1,729 \
                       blocks, every one frequency 1.0, phase 0, start 0, stop 0.",
            },
            NodeKind::GravityModifier => NodeSchema {
                kind: NodeKind::GravityModifier,
                id: "gravity",
                title: "Gravity",
                category: Category::Force,
                inputs: IN_GRAVITY_OBJECT,
                outputs: OUT_MODIFIER,
                params: || {
                    vec![
                        ("Axis", Value::Vec3([0.0, 0.0, -1.0])),
                        ("Strength", Value::F32(9.8)),
                        ("Decay", Value::F32(0.0)),
                        ("Force Type", Value::Enum(FORCE_TYPES, 0)),
                        ("Turbulence", Value::F32(0.0)),
                        ("Turbulence Scale", Value::F32(1.0)),
                        ("World Aligned", Value::Bool(true)),
                        ("Gravity Offset", Value::Vec3([0.0, 0.0, 0.0])),
                    ]
                },
                body_params: &["Axis", "Strength"],
                help: "NiPSysGravityModifier — planar or spherical force field.",
            },
            NodeKind::RotationModifier => NodeSchema {
                kind: NodeKind::RotationModifier,
                id: "rotation",
                title: "Rotation",
                category: Category::Appearance,
                inputs: NO_SOCKETS,
                outputs: OUT_MODIFIER,
                params: || {
                    vec![
                        ("Rotation Speed", Value::F32(0.0)),
                        ("Speed Variation", Value::F32(0.0)),
                        ("Rotation Angle", Value::F32(0.0)),
                        ("Angle Variation", Value::F32(0.0)),
                        ("Random Speed Sign", Value::Bool(false)),
                        ("Random Axis", Value::Bool(true)),
                        ("Axis", Value::Vec3([0.0, 0.0, 1.0])),
                    ]
                },
                body_params: &["Rotation Speed", "Random Axis"],
                help: "NiPSysRotationModifier — billboard spin. Requires rotation data \
                       enabled on Particle Data.",
            },
            NodeKind::SimpleColorModifier => NodeSchema {
                kind: NodeKind::SimpleColorModifier,
                id: "simple_color",
                title: "Color Over Life",
                category: Category::Appearance,
                inputs: NO_SOCKETS,
                outputs: OUT_MODIFIER,
                params: || {
                    vec![
                        ("Color 1", Value::Color4([1.0, 1.0, 1.0, 0.0])),
                        ("Color 2", Value::Color4([1.0, 1.0, 1.0, 1.0])),
                        ("Color 3", Value::Color4([1.0, 1.0, 1.0, 0.0])),
                        ("Fade In End %", Value::F32(0.2)),
                        ("Color 1 End %", Value::F32(0.3)),
                        ("Color 2 Start %", Value::F32(0.4)),
                        ("Color 2 End %", Value::F32(0.6)),
                        ("Color 3 Start %", Value::F32(0.7)),
                        ("Fade Out Start %", Value::F32(0.8)),
                    ]
                },
                body_params: &["Color 1", "Color 2", "Color 3"],
                help: "BSPSysSimpleColorModifier — three-stop life gradient with \
                       fade-in/out. Requires color data enabled on Particle Data.",
            },
            NodeKind::ScaleModifier => NodeSchema {
                kind: NodeKind::ScaleModifier,
                id: "scale",
                title: "Scale Over Life",
                category: Category::Appearance,
                inputs: NO_SOCKETS,
                outputs: OUT_MODIFIER,
                params: || vec![("Scales", Value::Str("1.0, 1.0".into()))],
                body_params: &[],
                help: "BSPSysScaleModifier — a size curve over particle life, stored as \
                       evenly spaced samples. Paint it directly, or reshape it \
                       through a few control points; right-click for presets.",
            },
            NodeKind::DragModifier => NodeSchema {
                kind: NodeKind::DragModifier,
                id: "drag",
                title: "Drag",
                category: Category::Force,
                inputs: IN_DRAG_OBJECT,
                outputs: OUT_MODIFIER,
                params: || {
                    vec![
                        ("Axis", Value::Vec3([0.0, 0.0, 1.0])),
                        ("Percentage", Value::F32(0.05)),
                        ("Range", Value::F32(3.4e38)),
                        ("Range Falloff", Value::F32(3.4e38)),
                        ("Drag Offset", Value::Vec3([0.0, 0.0, 0.0])),
                    ]
                },
                body_params: &["Percentage"],
                help: "NiPSysDragModifier — damps the velocity component along \
                       the axis, with distance falloff from the drag object.",
            },
            NodeKind::SubTexModifier => NodeSchema {
                kind: NodeKind::SubTexModifier,
                id: "subtex",
                title: "Atlas Frames",
                category: Category::Appearance,
                inputs: NO_SOCKETS,
                outputs: OUT_MODIFIER,
                params: || {
                    vec![
                        ("Start Frame", Value::F32(0.0)),
                        ("Start Frame Fudge", Value::F32(0.0)),
                        ("End Frame", Value::F32(15.0)),
                        ("Loop Start Frame", Value::F32(0.0)),
                        ("Loop Start Frame Fudge", Value::F32(0.0)),
                        ("Frame Count", Value::F32(16.0)),
                        ("Frame Count Fudge", Value::F32(0.0)),
                    ]
                },
                body_params: &["Start Frame", "End Frame", "Frame Count"],
                help: "BSPSysSubTexModifier — steps each particle through the \
                       atlas cells of Particle Data over its life: Frame Count \
                       frames per lifespan, start→end once, then looping from \
                       Loop Start Frame. Fudge adds 0..fudge random per particle.",
            },
            NodeKind::SpawnModifier => NodeSchema {
                kind: NodeKind::SpawnModifier,
                id: "spawn",
                title: "Spawn",
                category: Category::Emitter,
                inputs: NO_SOCKETS,
                outputs: OUT_MODIFIER,
                params: || {
                    vec![
                        ("Spawn On Death", Value::Bool(true)),
                        ("Spawn Generations", Value::U32(0)),
                        ("Percentage Spawned", Value::F32(1.0)),
                        ("Min Num to Spawn", Value::U32(1)),
                        ("Max Num to Spawn", Value::U32(1)),
                        ("Spawn Speed Variation", Value::F32(0.0)),
                        ("Spawn Dir Variation", Value::F32(0.0)),
                        ("Life Span", Value::F32(1.0)),
                        ("Life Span Variation", Value::F32(0.0)),
                    ]
                },
                body_params: &["Percentage Spawned", "Life Span"],
                help: "NiPSysSpawnModifier — referenced by Age/Death (and \
                       colliders) to respawn dying particles. NOTE: SkyrimSE \
                       1.5.97's engine stubs the spawn out — the fields load \
                       but no particle is ever spawned, so the preview keeps \
                       it inert to match the game.",
            },
            NodeKind::LodModifier => NodeSchema {
                kind: NodeKind::LodModifier,
                id: "lod",
                title: "LOD",
                category: Category::Appearance,
                inputs: NO_SOCKETS,
                outputs: OUT_MODIFIER,
                params: || {
                    vec![
                        ("LOD Begin Distance", Value::F32(0.1)),
                        ("LOD End Distance", Value::F32(0.7)),
                        ("End Emit Scale", Value::F32(0.2)),
                        ("End Size", Value::F32(1.0)),
                    ]
                },
                body_params: &["End Emit Scale"],
                help: "BSPSysLODModifier — scales emission and size down with \
                       camera distance (normalized: distance/36000 at default \
                       settings). The preview applies it live from the \
                       viewport camera.",
            },
            NodeKind::ModifierActiveCtlr => NodeSchema {
                kind: NodeKind::ModifierActiveCtlr,
                id: "active_ctlr",
                title: "Active Toggle",
                category: Category::Controller,
                inputs: IN_ACTIVE_CTLR,
                outputs: OUT_CONTROLLER,
                params: || {
                    vec![
                        ("Keys", Value::Curve(Curve::constant(Track::Bool, 1.0))),
                        ("Start Time", Value::F32(0.0)),
                        ("Stop Time", Value::F32(1.0)),
                        ("Loop", Value::Bool(true)),
                    ]
                },
                body_params: &["Keys"],
                help: "NiPSysModifierActiveCtlr — switches the wired modifier \
                       on and off over the controller window. The campfire \
                       pulses its drag this way for gusty smoke.",
            },
            NodeKind::ParamCtlr => NodeSchema {
                kind: NodeKind::ParamCtlr,
                id: "param_ctlr",
                title: "Param Track",
                category: Category::Controller,
                inputs: IN_ACTIVE_CTLR,
                outputs: OUT_CONTROLLER,
                params: || {
                    vec![
                        ("Parameter", Value::Enum(PARAM_TARGETS, 0)),
                        ("Keys", Value::Curve(Curve::constant(Track::Float, 1.0))),
                        ("Start Time", Value::F32(0.0)),
                        ("Stop Time", Value::F32(1.0)),
                        ("Loop", Value::Bool(true)),
                    ]
                },
                body_params: &["Parameter"],
                help: "A keyed float track driving one parameter of the wired \
                       modifier. The game \
                       samples it every update at controller-local time and \
                       writes the value RAW into the target — emitter params \
                       affect only particles spawned after the write; Gravity \
                       Strength bites immediately. Wire the emitter for \
                       emitter params, the Gravity node for strength, the \
                       Rotation node for the rotation params.",
            },
            NodeKind::VisibilityCtlr => NodeSchema {
                kind: NodeKind::VisibilityCtlr,
                id: "vis_ctlr",
                title: "Visibility",
                category: Category::Controller,
                inputs: NO_SOCKETS,
                outputs: OUT_CONTROLLER,
                params: || {
                    vec![
                        ("Keys", Value::Curve(Curve::constant(Track::Bool, 1.0))),
                        ("Start Time", Value::F32(0.0)),
                        ("Stop Time", Value::F32(1.0)),
                        ("Loop", Value::Bool(true)),
                    ]
                },
                body_params: &["Keys"],
                help: "NiVisController — flips the system's hidden flag from a \
                       stepped on/off track. The simulation keeps running \
                       while hidden, exactly like the game.",
            },
            NodeKind::MultiEmitterCtlr => NodeSchema {
                kind: NodeKind::MultiEmitterCtlr,
                id: "multi_emitter_ctlr",
                title: "Multi Emitter",
                category: Category::Controller,
                inputs: IN_EMITTER_CTLR,
                outputs: OUT_CONTROLLER,
                params: || {
                    vec![
                        ("Birth Rate", Value::F32(20.0)),
                        ("Birth Rate Keys", Value::Curve(Curve::empty(Track::Float))),
                        ("Max Emitters", Value::U32(4)),
                        ("Start Time", Value::F32(0.0)),
                        ("Stop Time", Value::F32(3.0)),
                        ("Loop", Value::Bool(true)),
                    ]
                },
                body_params: &["Birth Rate", "Max Emitters"],
                help: "BSPSysMultiTargetEmitterCtlr — the master-particle-system \
                       emitter: in game ONE simulating system spawns bursts at \
                       every placed reference whose addon node points at the \
                       master (nearest Max Emitters anchors to the camera). \
                       Exporting recreates the BSMasterParticleSystem root. In \
                       the preview there is one anchor, which the game's own \
                       arithmetic degenerates to a plain emitter controller; \
                       it ignores active keys by design (RE 0x140D582F0).",
            },
            NodeKind::InheritVelocityModifier => NodeSchema {
                kind: NodeKind::InheritVelocityModifier,
                id: "inherit_velocity",
                title: "Inherit Velocity",
                category: Category::Force,
                inputs: NO_SOCKETS,
                outputs: OUT_MODIFIER,
                params: || {
                    vec![
                        ("Chance To Inherit", Value::F32(100.0)),
                        ("Velocity Multiplier", Value::F32(0.5)),
                        ("Velocity Variation", Value::F32(0.0)),
                        ("Inherit Offset", Value::Vec3([0.0; 3])),
                        // The anchor's NAME, set on import — the preview
                        // tracks that node's motion to know what velocity
                        // there is to inherit.
                        ("Inherit Node", Value::Str(String::new())),
                    ]
                },
                body_params: &["Velocity Multiplier"],
                help: "BSPSysInheritVelocityModifier — at spawn, with Chance % \
                       odds, the particle adds the inherit object's own world \
                       velocity × uniform[max(0, M−M·V), M+M·V] (variation is \
                       RELATIVE to the multiplier; RE 0x140D269E0). The storm \
                       atronach's cloak follows the atronach this way. Still \
                       until the anchor node moves — play a sequence that \
                       moves it to see the effect.",
            },
            NodeKind::RecycleBoundModifier => NodeSchema {
                kind: NodeKind::RecycleBoundModifier,
                id: "recycle_bound",
                title: "Recycle Bound",
                category: Category::Force,
                inputs: NO_SOCKETS,
                outputs: OUT_MODIFIER,
                params: || {
                    vec![
                        ("Bound Offset", Value::Vec3([0.0; 3])),
                        ("Bound Extent", Value::Vec3([100.0, 100.0, 100.0])),
                        ("Anchor Offset", Value::Vec3([0.0; 3])),
                    ]
                },
                body_params: &["Bound Extent"],
                help: "BSPSysRecycleBoundModifier — particles leaving the box \
                       wrap toroidally back in (never killed; age, velocity \
                       and trails untouched). The box's MIN corner sits at \
                       anchor + Bound Offset and it spans +Extent per axis \
                       (RE 0x140D25DE0) — blowing fog loops through a room \
                       this way.",
            },
            NodeKind::WindModifier => NodeSchema {
                kind: NodeKind::WindModifier,
                id: "wind",
                title: "Wind",
                category: Category::Force,
                inputs: NO_SOCKETS,
                outputs: OUT_MODIFIER,
                params: || vec![("Strength", Value::F32(0.25))],
                body_params: &["Strength"],
                help: "BSWindModifier — acceleration along the WEATHER's wind, \
                       windSpeed × 100 × Strength units/s² on the horizontal \
                       plane, zero indoors (RE 0x140D3D4B0: the direction is \
                       global state, not a field). The preview uses the wind \
                       control on the toolbar.",
            },
            NodeKind::BombModifier => NodeSchema {
                kind: NodeKind::BombModifier,
                id: "bomb",
                title: "Bomb",
                category: Category::Force,
                inputs: IN_BOMB_OBJECT,
                outputs: OUT_MODIFIER,
                params: || {
                    vec![
                        ("Axis", Value::Vec3([0.0, 0.0, 1.0])),
                        ("Decay", Value::F32(0.0)),
                        ("Delta V", Value::F32(10.0)),
                        ("Decay Type", Value::Enum(DECAY_TYPES, 0)),
                        ("Symmetry", Value::Enum(SYMMETRY_TYPES, 0)),
                        ("Bomb Offset", Value::Vec3([0.0, 0.0, 0.0])),
                    ]
                },
                body_params: &["Axis", "Delta V"],
                help: "NiPSysBombModifier — radial force from an object.",
            },
            NodeKind::ColliderModifier => NodeSchema {
                kind: NodeKind::ColliderModifier,
                id: "collider",
                title: "Collider",
                category: Category::Force,
                inputs: NO_SOCKETS,
                outputs: OUT_MODIFIER,
                params: || {
                    vec![
                        ("Active", Value::Bool(true)),
                        ("Shape", Value::Enum(COLLIDER_SHAPES, 0)),
                        ("Bounce", Value::F32(1.0)),
                        ("Die On Collide", Value::Bool(false)),
                        ("Spawn On Collide", Value::Bool(false)),
                        ("Radius", Value::F32(10.0)),
                        ("Width", Value::F32(10.0)),
                        ("Height", Value::F32(10.0)),
                        ("X Axis", Value::Vec3([1.0, 0.0, 0.0])),
                        ("Y Axis", Value::Vec3([0.0, 1.0, 0.0])),
                        ("Collider Offset", Value::Vec3([0.0, 0.0, 0.0])),
                        ("Collider Rotation", Value::Vec3([0.0, 0.0, 0.0])),
                    ]
                },
                body_params: &["Shape", "Bounce", "Radius"],
                help: "NiPSysColliderManager — particles bounce off or die on \
                       a sphere or bounded plane.",
            },
            NodeKind::EffectShader => NodeSchema {
                kind: NodeKind::EffectShader,
                id: "effect_shader",
                title: "Effect Shader",
                category: Category::Shader,
                inputs: NO_SOCKETS,
                outputs: OUT_SHADER,
                params: || {
                    vec![
                        (
                            "Source Texture",
                            // A path that exists in vanilla, so a fresh node
                            // previews instead of showing a missing texture.
                            Value::TexturePath(
                                "textures\\effects\\gradients\\gradflame01.dds".into(),
                            ),
                        ),
                        ("Base Color", Value::Color4([1.0, 1.0, 1.0, 1.0])),
                        ("Base Color Scale", Value::F32(1.0)),
                        ("Soft Falloff Depth", Value::F32(0.0)),
                        ("Emissive Scale Keys", Value::Curve(Curve::empty(Track::Float))),
                        ("Alpha Keys", Value::Curve(Curve::empty(Track::Float))),
                        ("U Offset Keys", Value::Curve(Curve::empty(Track::Float))),
                        ("U Scale Keys", Value::Curve(Curve::empty(Track::Float))),
                        ("V Offset Keys", Value::Curve(Curve::empty(Track::Float))),
                        ("V Scale Keys", Value::Curve(Curve::empty(Track::Float))),
                        ("Color Keys", Value::Curve(Curve::empty(Track::Vec3))),
                        ("Soft Effect", Value::Bool(false)),
                        ("Falloff Start Angle", Value::F32(1.0)),
                        ("Falloff Stop Angle", Value::F32(0.0)),
                        ("Falloff Start Opacity", Value::F32(1.0)),
                        ("Falloff Stop Opacity", Value::F32(0.0)),
                        ("Use Falloff", Value::Bool(false)),
                        ("Texture Clamp", Value::Enum(CLAMP_MODES, 0)),
                        ("Greyscale Texture", Value::TexturePath(String::new())),
                        ("Greyscale Color", Value::Bool(false)),
                        ("Greyscale Alpha", Value::Bool(false)),
                        ("UV Offset U", Value::F32(0.0)),
                        ("UV Offset V", Value::F32(0.0)),
                        ("UV Scale U", Value::F32(1.0)),
                        ("UV Scale V", Value::F32(1.0)),
                        ("Double Sided", Value::Bool(false)),
                        ("Lighting Influence", Value::U32(255)),
                    ]
                },
                body_params: &["Source Texture", "Base Color"],
                help: "BSEffectShaderProperty — texture, tint, falloff, the \
                       greyscale-to-palette ramp vanilla fire is built on, and \
                       the UV transform its scroll controllers animate.",
            },
            NodeKind::AlphaProperty => NodeSchema {
                kind: NodeKind::AlphaProperty,
                id: "alpha",
                title: "Alpha Blend",
                category: Category::Shader,
                inputs: NO_SOCKETS,
                outputs: OUT_ALPHA,
                params: || {
                    vec![
                        ("Blend", Value::Enum(BLEND_PRESETS, 0)),
                        ("Alpha Test", Value::Bool(false)),
                        ("Test Threshold", Value::U32(128)),
                        ("Sort Particles", Value::Bool(true)),
                        // The rest of the 16-bit word. Together with
                        // the four above they cover every bit, which
                        // is what makes `alpha_flags` a bijection.
                        ("Blending", Value::Bool(true)),
                        ("Source Blend", Value::Enum(ALPHA_FUNCTIONS, 6)),
                        ("Destination Blend", Value::Enum(ALPHA_FUNCTIONS, 7)),
                        ("Test Function", Value::Enum(TEST_FUNCTIONS, 4)),
                        ("Clone Unique", Value::Bool(false)),
                        ("Editor Alpha Threshold", Value::Bool(false)),
                    ]
                },
                body_params: &["Blend"],
                help: "NiAlphaProperty — the whole AlphaFlags word: blend \
                       factors, the alpha test and the two Bethesda bits.",
            },
        }
    }

    pub fn from_stable_id(id: &str) -> Option<NodeKind> {
        ALL_NODE_KINDS.iter().copied().find(|k| k.schema().id == id)
    }
}

/// The block a node was authored from, carried BY the node.
///
/// A graph node is a lossy view of a NIF block: the compiler models a
/// subset of the fields and reads the rest back off the block the node
/// came from. Inside one open file that block is named by
/// [`GraphNode::source_block`]. Across files -- a copy, a library
/// item, a template -- there is no file to point into, and until this
/// existed every unmodelled field fell back to `Default`, which is
/// whatever the struct happens to say.
///
/// Measured before it existed, over 29 particle fixtures, on the BYTE
/// fields alone: a pasted system turned 39 `NiBoolInterpolator` "no
/// pose" sentinels into ON, woke 18 inactive gravity modifiers, reset
/// 14 authored alpha-test thresholds -- and dropped
/// `has_rotation_speeds`, which crashes the game outright because
/// `NiPSysRotationModifier::Func38` writes that array unguarded.
///
/// It is deliberately NOT `source_block`: that names a SLOT the splice
/// may overwrite, and a copy must never claim its original's slot.
/// This names only VALUES. The two are separate jobs and were only
/// ever one field by accident.
#[derive(Debug, Clone)]
pub struct AuthoredBlock {
    /// Index of the node's OWN block, into `blocks`.
    pub root: u32,
    /// The node's block and everything it OWNS, keyed by the index it
    /// had in the file it came from -- so a reader resolves a `Ref` the
    /// same way it would against the file, and the readers that walk a
    /// node's own subtree (its controller chain, its interpolator, that
    /// interpolator's data block) work unchanged.
    ///
    /// Child refs only. A `Ptr` points OUT of the node, into the scene,
    /// and a scene cannot travel with a copy -- which is correct: a
    /// pasted emitter is authored fresh under an identity anchor.
    pub blocks: std::collections::BTreeMap<u32, aether_nif::Block>,
    /// Which stream these bytes were written for: block layouts differ
    /// between Skyrim SE (bs100) and LE (bs83), so the bytes are only
    /// meaningful alongside the version that wrote them.
    pub sse: bool,
}

impl AuthoredBlock {
    /// Hex, because the project format is hand-rolled JSON with no
    /// base64 and a byte array would quadruple the file.
    fn hex(data: &[u8]) -> String {
        let mut out = String::with_capacity(data.len() * 2);
        for b in data {
            out.push(char::from_digit((b >> 4) as u32, 16).unwrap_or('0'));
            out.push(char::from_digit((b & 15) as u32, 16).unwrap_or('0'));
        }
        out
    }

    /// The node's own block.
    pub fn own(&self) -> Option<&aether_nif::Block> {
        self.blocks.get(&self.root)
    }

    /// Capture `root` and everything it owns out of `file`.
    ///
    /// Bounded: a runaway walk would put a whole scene in the clipboard.
    /// Truncating loses a field restore, never correctness — a reader
    /// that cannot find its block falls back to what it did before any
    /// of this existed.
    pub fn capture(file: &aether_nif::NifFile, root: u32) -> Option<AuthoredBlock> {
        let ctx = aether_nif::typed::Ctx::of(&file.header).ok()?;
        let mut blocks = std::collections::BTreeMap::new();
        let mut stack = vec![root];
        let mut bytes = 0usize;
        while let Some(i) = stack.pop() {
            if blocks.contains_key(&i) || blocks.len() >= 64 || bytes >= 256 * 1024 {
                continue;
            }
            let Some(b) = file.blocks.get(i as usize) else {
                continue;
            };
            bytes += b.data.len();
            blocks.insert(i, b.clone());
            if let Some(refs) = aether_nif::typed::block_refs(b, ctx) {
                for (r, kind) in refs {
                    if kind != aether_nif::typed::RefKind::Child {
                        continue;
                    }
                    if let Some(t) = r.get() {
                        stack.push(t as u32);
                    }
                }
            }
        }
        blocks.contains_key(&root).then_some(AuthoredBlock {
            root,
            blocks,
            sse: matches!(
                file.header.bs_version,
                aether_nif::header::BsStreamVersion::SkyrimSE
            ),
        })
    }

    fn from_hex(hex: &str) -> Option<Vec<u8>> {
        if hex.len() % 2 != 0 {
            return None;
        }
        let bytes: Vec<u8> = hex.as_bytes().to_vec();
        let mut out = Vec::with_capacity(hex.len() / 2);
        for pair in bytes.chunks_exact(2) {
            let hi = (pair[0] as char).to_digit(16)?;
            let lo = (pair[1] as char).to_digit(16)?;
            out.push((hi * 16 + lo) as u8);
        }
        Some(out)
    }
}

/// A node instance in a graph document.
#[derive(Debug, Clone)]
pub struct GraphNode {
    /// Schema params the SAVED document did not carry -- params added to
    /// the schema after it was written. They load at the schema default,
    /// which for a node with provenance is a value nobody chose; see
    /// [`Graph::adopt_unset_params`]. Transient: never serialized, and
    /// empty for every node not loaded from JSON.
    pub unset_params: Vec<&'static str>,
    pub kind: NodeKind,
    /// Canvas position, graph-space.
    pub pos: [f32; 2],
    /// Ordered `(key, value)` pairs matching the schema's param list.
    pub params: Vec<(&'static str, Value)>,
    /// Provenance: the block index this node was decompiled from, in
    /// the NIF the tab was imported from. `None` for nodes the user
    /// added, and for every node in a graph built from scratch.
    ///
    /// This lives ON THE NODE, not in a side map on `Decompiled`, for
    /// two reasons. It persists with the graph for free, so a project
    /// reload does not silently degrade the export splice to appending
    /// at the end. And `NodeId`s are REUSED after a delete, so a map
    /// keyed by node id would re-attach a dead node's slot to whatever
    /// the user adds next — a mis-attribution with no symptom until an
    /// export overwrites the wrong block.
    ///
    /// It is a hint, never a promise: a duplicated node carries a
    /// duplicate of its original's index, so the exporter treats the
    /// mapping as "first claim wins, the rest append".
    pub source_block: Option<u32>,
    /// The bytes this node was authored from, for the fields the graph
    /// does not model. See [`AuthoredBlock`]. Filled when a node is
    /// copied out of a graph that has its file open, and carried
    /// through the library and the project file.
    pub authored: Option<AuthoredBlock>,
}

/// Why [`Graph::reorder_link`] refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReorderError {
    NoSuchSlot,
    SamePlace,
}

/// Why [`Graph::change_kind`] refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKindError {
    NoSuchNode,
    SameKind,
    /// Different category or socket signature: existing wires could
    /// land on sockets that no longer mean the same thing.
    Incompatible,
}

/// The SEMANTIC swap families: kinds a user may reasonably turn into
/// one another. Same socket shape is NECESSARY (wires must stay
/// valid) but not SUFFICIENT — the first cut used category +
/// signature alone and the measured table offered `MeshEmitter ->
/// SpawnModifier` and `UpdateCtlr -> VisibilityCtlr`, pairs that share
/// a shape by accident and nothing else. Membership here is a
/// DECISION, and the change_kind test pins the whole table so a new
/// kind cannot slide into a family unexamined.
fn swap_family(kind: NodeKind) -> Option<u8> {
    use NodeKind as K;
    Some(match kind {
        // Volume emitters. The MESH emitter is out by its sockets (no
        // Emitter Object input), not by choice.
        K::BoxEmitter | K::SphereEmitter | K::CylinderEmitter => 0,
        // Anchored forces: they share the Axis/anchor vocabulary.
        K::GravityModifier | K::DragModifier | K::BombModifier => 1,
        // Per-particle modifiers: one stack slot, no inputs, and a
        // swap means "replace what this stack position does".
        K::RotationModifier
        | K::SimpleColorModifier
        | K::ScaleModifier
        | K::SubTexModifier
        | K::LodModifier => 2,
        // Controllers that drive one wired target: the emitter pair
        // (plain <-> multi) plus the per-modifier toggle and param
        // track, all one Modifier wire in.
        K::EmitterCtlr | K::MultiEmitterCtlr | K::ModifierActiveCtlr | K::ParamCtlr => 3,
        // Ambient influences with an optional anchor object.
        K::InheritVelocityModifier | K::RecycleBoundModifier | K::WindModifier => 4,
        _ => return None,
    })
}

/// The kinds a node of `kind` may change into IN PLACE: the same
/// semantic family (see [`swap_family`]) AND socket-for-socket the
/// same signature, so every existing wire stays valid by
/// construction. The signature check stays even though families are
/// hand-picked: it is the INVARIANT (wires survive), where the family
/// is the POLICY, and a family edit that broke the invariant should
/// fail loudly here rather than corrupt a graph.
pub fn compatible_kinds(kind: NodeKind) -> Vec<NodeKind> {
    let schema = kind.schema();
    let same_sig = |a: &'static [SocketSchema], b: &'static [SocketSchema]| {
        a.len() == b.len()
            && a.iter()
                .zip(b)
                .all(|(x, y)| x.ty == y.ty && x.multi == y.multi)
    };
    let Some(family) = swap_family(kind) else {
        return Vec::new();
    };
    ALL_NODE_KINDS
        .iter()
        .copied()
        .filter(|k| *k != kind)
        .filter(|k| swap_family(*k) == Some(family))
        .filter(|k| {
            let other = k.schema();
            same_sig(other.inputs, schema.inputs) && same_sig(other.outputs, schema.outputs)
        })
        .collect()
}

impl GraphNode {
    pub fn new(kind: NodeKind, pos: [f32; 2]) -> Self {
        GraphNode {
            kind,
            pos,
            params: (kind.schema().params)(),
            source_block: None,
            authored: None,
            unset_params: Vec::new(),
        }
    }

    pub fn param(&self, key: &str) -> Option<&Value> {
        self.params.iter().find(|(k, _)| *k == key).map(|(_, v)| v)
    }

    /// Typed parameter readers. Everything that consumes a graph (compiler,
    /// simulator, UI) goes through these, so "how a param is read" lives in
    /// one place and a schema change cannot silently diverge per consumer.
    pub fn f32_or(&self, key: &str, default: f32) -> f32 {
        match self.param(key) {
            Some(Value::F32(v)) => *v,
            _ => default,
        }
    }

    pub fn u32_or(&self, key: &str, default: u32) -> u32 {
        match self.param(key) {
            Some(Value::U32(v)) => *v,
            _ => default,
        }
    }

    pub fn bool_or(&self, key: &str, default: bool) -> bool {
        match self.param(key) {
            Some(Value::Bool(v)) => *v,
            _ => default,
        }
    }

    pub fn str_or(&self, key: &str, default: &str) -> String {
        match self.param(key) {
            Some(Value::Str(v)) | Some(Value::TexturePath(v)) => v.clone(),
            _ => default.to_string(),
        }
    }

    /// A keyed track parameter, or an empty track of `track`'s kind.
    ///
    /// The kind is passed in rather than guessed so a missing
    /// parameter still answers with something the widget and the
    /// compiler can both act on — the same shape as every other
    /// accessor here.
    pub fn curve_or(&self, key: &str, track: crate::curve::Track) -> crate::curve::Curve {
        match self.param(key) {
            Some(Value::Curve(c)) => c.clone(),
            _ => crate::curve::Curve::empty(track),
        }
    }

    pub fn vec3_or(&self, key: &str, default: [f32; 3]) -> [f32; 3] {
        match self.param(key) {
            Some(Value::Vec3(v)) => *v,
            _ => default,
        }
    }

    pub fn color4_or(&self, key: &str, default: [f32; 4]) -> [f32; 4] {
        match self.param(key) {
            Some(Value::Color4(v)) => *v,
            _ => default,
        }
    }

    /// Selected index of an `Enum` param (0 when absent).
    pub fn enum_index(&self, key: &str) -> usize {
        match self.param(key) {
            Some(Value::Enum(_, i)) => *i,
            _ => 0,
        }
    }

    pub fn set_param(&mut self, key: &str, value: Value) -> Option<Value> {
        for (k, v) in &mut self.params {
            if *k == key {
                return Some(std::mem::replace(v, value));
            }
        }
        None
    }
}

/// A wire between an output socket and an input socket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Link {
    pub from_node: NodeId,
    pub from_socket: usize,
    pub to_node: NodeId,
    pub to_socket: usize,
}

/// One particle-graph document.
#[derive(Debug, Clone, Default)]
pub struct Graph {
    pub nodes: HashMap<NodeId, GraphNode>,
    pub links: Vec<Link>,
    /// The document's animation clips, in file order (see `clip.rs`).
    pub clips: Vec<crate::clip::Clip>,
    next_id: u64,
}

/// Why a connection was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectError {
    UnknownNode,
    BadSocket,
    TypeMismatch { from: SocketType, to: SocketType },
    WouldCycle,
    InputOccupied,
    SelfLink,
    /// This exact wire already exists. Silently accepting it made
    /// `Command::Connect` report success having inserted nothing,
    /// while its revert removed the PRE-EXISTING wire — a Batch
    /// containing one deleted a wire it never added.
    Duplicate,
}

impl std::fmt::Display for ConnectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConnectError::UnknownNode => write!(f, "node no longer exists"),
            ConnectError::BadSocket => write!(f, "socket index out of range"),
            ConnectError::TypeMismatch { from, to } => {
                write!(f, "cannot connect {} to {}", from.name(), to.name())
            }
            ConnectError::WouldCycle => write!(f, "connection would create a cycle"),
            ConnectError::InputOccupied => write!(f, "input already has a connection"),
            ConnectError::SelfLink => write!(f, "cannot connect a node to itself"),
            ConnectError::Duplicate => write!(f, "this wire already exists"),
        }
    }
}

impl Graph {
    pub fn new() -> Self {
        Self::default()
    }

    /// A copy of this graph with `drop` and every link touching them
    /// removed, ids otherwise UNCHANGED.
    ///
    /// Ids are kept because the caller is looking at the same document
    /// through both graphs: a preview built from the filtered copy has to
    /// be able to name a node the canvas still shows. That is the
    /// difference from `fragment::extract`, which re-bases ids to `0..n`
    /// because a fragment is going somewhere else.
    ///
    /// `next_id` is carried over too, so a node added to the copy could
    /// never collide with one in the original.
    pub fn without_nodes(&self, drop: &[NodeId]) -> Graph {
        let mut out = Graph {
            nodes: HashMap::new(),
            links: Vec::new(),
            clips: Vec::new(),
            next_id: self.next_id,
        };
        for (id, node) in &self.nodes {
            if !drop.contains(id) {
                out.nodes.insert(*id, node.clone());
            }
        }
        // A clip keeps its tracks on the nodes that are still here: a
        // track on a muted node is muted with it.
        out.clips = self
            .clips
            .iter()
            .map(|c| {
                let mut c = c.clone();
                c.tracks.retain(|t| !drop.contains(&t.node));
                c
            })
            .collect();
        // Straight into the Vec, in source order: `connect` would
        // re-validate a state that was already legal, and the order of
        // `links` is the slot order, which the compiler turns into the
        // modifier stack.
        for l in &self.links {
            if out.nodes.contains_key(&l.from_node) && out.nodes.contains_key(&l.to_node) {
                out.links.push(*l);
            }
        }
        out
    }

    pub fn insert(&mut self, node: GraphNode) -> NodeId {
        let id = NodeId(self.next_id);
        self.next_id += 1;
        self.nodes.insert(id, node);
        id
    }

    /// Allocate a fresh id without inserting yet (the command system inserts).
    pub fn reserve_id(&mut self) -> NodeId {
        let id = NodeId(self.next_id);
        self.next_id += 1;
        id
    }

    /// Insert with a specific id (used by undo and project loading).
    pub fn insert_with_id(&mut self, id: NodeId, node: GraphNode) {
        self.next_id = self.next_id.max(id.0 + 1);
        self.nodes.insert(id, node);
    }

    /// Remove a node and all links touching it. Returns them for undo.
    pub fn remove(&mut self, id: NodeId) -> Option<(GraphNode, Vec<Link>)> {
        let node = self.nodes.remove(&id)?;
        let (dead, alive): (Vec<Link>, Vec<Link>) = self
            .links
            .drain(..)
            .partition(|l| l.from_node == id || l.to_node == id);
        self.links = alive;
        Some((node, dead))
    }

    /// Validate a prospective connection without mutating.
    pub fn check_connect(&self, link: Link) -> Result<(), ConnectError> {
        if link.from_node == link.to_node {
            return Err(ConnectError::SelfLink);
        }
        let from = self
            .nodes
            .get(&link.from_node)
            .ok_or(ConnectError::UnknownNode)?;
        let to = self
            .nodes
            .get(&link.to_node)
            .ok_or(ConnectError::UnknownNode)?;
        let out_schema = from
            .kind
            .schema()
            .outputs
            .get(link.from_socket)
            .ok_or(ConnectError::BadSocket)?;
        let in_schema = to
            .kind
            .schema()
            .inputs
            .get(link.to_socket)
            .ok_or(ConnectError::BadSocket)?;
        if !out_schema.ty.can_connect_to(in_schema.ty) {
            return Err(ConnectError::TypeMismatch {
                from: out_schema.ty,
                to: in_schema.ty,
            });
        }
        if !in_schema.multi
            && self
                .links
                .iter()
                .any(|l| l.to_node == link.to_node && l.to_socket == link.to_socket)
        {
            return Err(ConnectError::InputOccupied);
        }
        if self.links.contains(&link) {
            return Err(ConnectError::Duplicate);
        }
        if self.reaches(link.to_node, link.from_node) {
            return Err(ConnectError::WouldCycle);
        }
        Ok(())
    }

    /// Connect, validating first.
    /// Change a node's KIND in place, keeping its wires, position and
    /// provenance.
    ///
    /// Params become the TARGET schema's defaults overlaid with every
    /// same-named value the node already carried — Speed, Life Span
    /// and the rest of the shared emitter base survive a Box -> Sphere
    /// swap; Width/Height/Depth fall away and Radius arrives at its
    /// default. Returns the old `(kind, params)` so an undo command
    /// can restore them exactly.
    pub fn change_kind(
        &mut self,
        id: NodeId,
        new_kind: NodeKind,
    ) -> Result<(NodeKind, Vec<(&'static str, Value)>), ChangeKindError> {
        let node = self.nodes.get(&id).ok_or(ChangeKindError::NoSuchNode)?;
        if node.kind == new_kind {
            return Err(ChangeKindError::SameKind);
        }
        if !compatible_kinds(node.kind).contains(&new_kind) {
            return Err(ChangeKindError::Incompatible);
        }
        let node = self.nodes.get_mut(&id).expect("checked above");
        let old_kind = node.kind;
        let old_params = std::mem::take(&mut node.params);
        node.kind = new_kind;
        node.params = (new_kind.schema().params)();
        for (key, value) in &old_params {
            // set_param is a no-op for keys the target schema lacks,
            // which is exactly the carry-over rule.
            node.set_param(key, value.clone());
        }
        Ok((old_kind, old_params))
    }

    /// Move the `from_slot`-th wire of a MULTI input socket to sit at
    /// `to_slot`, keeping every other wire's relative order. Slot
    /// order is the order of `links`, which is what the compiler's
    /// stable class sort turns into the modifier stack — so for
    /// same-class modifiers this IS the order the game applies them
    /// in. Cross-class order is canonicalised by the format's Order
    /// field either way.
    pub fn reorder_link(
        &mut self,
        node: NodeId,
        socket: usize,
        from_slot: usize,
        to_slot: usize,
    ) -> Result<(usize, usize), ReorderError> {
        let slots: Vec<usize> = self
            .links
            .iter()
            .enumerate()
            .filter(|(_, l)| l.to_node == node && l.to_socket == socket)
            .map(|(i, _)| i)
            .collect();
        if from_slot >= slots.len() || to_slot >= slots.len() {
            return Err(ReorderError::NoSuchSlot);
        }
        if from_slot == to_slot {
            return Err(ReorderError::SamePlace);
        }
        // Remove the moved link, then insert at the target slot's
        // OLD global index: moving up that is where the target link
        // sits (the moved one lands before it); moving down the
        // target shifted left by the removal, so the same index is
        // the position right AFTER it. Only one element moves, so
        // links of every other socket keep their relative order.
        let remove_at = slots[from_slot];
        let insert_at = slots[to_slot].min(self.links.len() - 1);
        self.move_link(remove_at, insert_at);
        Ok((remove_at, insert_at))
    }

    /// Move one link within `links` by RAW index. The exact inverse
    /// of `move_link(a, b)` is `move_link(b, a)`, which is what an
    /// undo needs: `reorder_link`'s slot-space inverse restores the
    /// socket's own order but DISPLACES links of other sockets
    /// interleaved between the two indices, and every index-based
    /// restore recorded earlier (Disconnect, RemoveNode) then
    /// reinserts on the wrong side of a sibling. Measured: links
    /// [A->N, X->M, B->N] came back as [X->M, A->N, B->N], and with
    /// a Disconnect recorded before the reorder, undoing both
    /// REVERSED an unrelated system's modifier order — which the
    /// stable class sort carries straight into the exported bytes.
    pub fn move_link(&mut self, from: usize, to: usize) {
        if from >= self.links.len() || to >= self.links.len() || from == to {
            return;
        }
        let link = self.links.remove(from);
        self.links.insert(to, link);
    }

    pub fn connect(&mut self, link: Link) -> Result<(), ConnectError> {
        self.check_connect(link)?;
        self.links.push(link);
        Ok(())
    }

    pub fn disconnect(&mut self, link: Link) -> bool {
        let before = self.links.len();
        self.links.retain(|l| *l != link);
        self.links.len() != before
    }

    /// Disconnect and say WHERE the link sat. Since slot order became
    /// a real, exported property (the stable class sort turns wire
    /// order into stack order), an undo that re-appends a severed
    /// wire restores the wrong order — and a wrong order reaches
    /// the bytes.
    pub fn disconnect_indexed(&mut self, link: Link) -> Option<usize> {
        let at = self.links.iter().position(|l| *l == link)?;
        self.links.remove(at);
        Some(at)
    }

    /// Re-insert a link at its remembered index. REVERT-ONLY: this
    /// bypasses `check_connect`, because it restores a state that was
    /// already legal — validating it again could refuse (occupancy)
    /// and leave an undo half-applied.
    pub fn reconnect_at(&mut self, at: usize, link: Link) {
        self.links.insert(at.min(self.links.len()), link);
    }

    /// Is `to` reachable from `from` following links downstream?
    fn reaches(&self, from: NodeId, to: NodeId) -> bool {
        let mut stack = vec![from];
        let mut seen = std::collections::HashSet::new();
        while let Some(n) = stack.pop() {
            if n == to {
                return true;
            }
            if !seen.insert(n) {
                continue;
            }
            for l in &self.links {
                if l.from_node == n {
                    stack.push(l.to_node);
                }
            }
        }
        false
    }

    /// Links landing on a given input socket.
    pub fn incoming(&self, node: NodeId, socket: usize) -> impl Iterator<Item = &Link> {
        self.links
            .iter()
            .filter(move |l| l.to_node == node && l.to_socket == socket)
    }

    // ---- persistence ----

    pub fn to_json(&self) -> Json {
        let mut ids: Vec<&NodeId> = self.nodes.keys().collect();
        ids.sort();
        let nodes = ids
            .iter()
            .map(|id| {
                let n = &self.nodes[id];
                let params = Json::Obj(
                    n.params
                        .iter()
                        .map(|(k, v)| (k.to_string(), v.to_json()))
                        .collect(),
                );
                let obj = Json::obj()
                    .with("id", Json::Num(id.0 as f64))
                    .with("kind", Json::Str(n.kind.schema().id.to_string()))
                    .with(
                        "pos",
                        Json::Arr(vec![Json::Num(n.pos[0] as f64), Json::Num(n.pos[1] as f64)]),
                    )
                    .with("params", params);
                // Written only when there is one: a graph authored from
                // scratch should not carry a null on every node.
                let obj = match n.source_block {
                    Some(src) => obj.with("src", Json::Num(src as f64)),
                    None => obj,
                };
                match &n.authored {
                    Some(a) => obj.with(
                        "authored",
                        Json::obj()
                            .with("root", Json::Num(a.root as f64))
                            .with("sse", Json::Bool(a.sse))
                            .with(
                                "blocks",
                                Json::Arr(
                                    a.blocks
                                        .iter()
                                        .map(|(i, b)| {
                                            Json::obj()
                                                .with("i", Json::Num(*i as f64))
                                                .with(
                                                    "type",
                                                    Json::Str(b.type_name.clone()),
                                                )
                                                .with(
                                                    "hex",
                                                    Json::Str(AuthoredBlock::hex(&b.data)),
                                                )
                                        })
                                        .collect(),
                                ),
                            ),
                    ),
                    None => obj,
                }
            })
            .collect();
        let links = self
            .links
            .iter()
            .map(|l| {
                Json::Arr(vec![
                    Json::Num(l.from_node.0 as f64),
                    Json::Num(l.from_socket as f64),
                    Json::Num(l.to_node.0 as f64),
                    Json::Num(l.to_socket as f64),
                ])
            })
            .collect();
        let out = Json::obj()
            .with("nodes", Json::Arr(nodes))
            .with("links", Json::Arr(links));
        // Written only when there are clips: a graph authored from
        // scratch has none and should not carry an empty list.
        if self.clips.is_empty() {
            return out;
        }
        let clips = self
            .clips
            .iter()
            .map(|c| {
                let tracks = c
                    .tracks
                    .iter()
                    .map(|t| {
                        let obj = Json::obj()
                            .with("node", Json::Num(t.node.0 as f64))
                            .with("param", Json::Str(t.param.to_string()))
                            .with("curve", t.curve.to_json());
                        match t.source_slot {
                            Some(s) => obj.with("slot", Json::Num(s as f64)),
                            None => obj,
                        }
                    })
                    .collect();
                let text = c
                    .text_keys
                    .iter()
                    .map(|(t, s)| Json::Arr(vec![Json::Num(*t as f64), Json::Str(s.clone())]))
                    .collect();
                let obj = Json::obj()
                    .with("name", Json::Str(c.name.clone()))
                    .with("loop", Json::Bool(c.looping))
                    .with("start", Json::Num(c.start_time as f64))
                    .with("stop", Json::Num(c.stop_time as f64))
                    .with("freq", Json::Num(c.frequency as f64))
                    .with("text", Json::Arr(text))
                    .with("tracks", Json::Arr(tracks));
                match c.source_block {
                    Some(b) => obj.with("src", Json::Num(b as f64)),
                    None => obj,
                }
            })
            .collect();
        out.with("clips", Json::Arr(clips))
    }

    /// Give every param a saved document never wrote the value the
    /// SOURCE file holds, taken from `source_doc` -- a fresh decompile of
    /// that file -- by provenance.
    ///
    /// A value the save DID write is the user's and is never touched; a
    /// node without provenance has no file to ask and keeps the default.
    /// Every node's unset list is cleared either way: after this the
    /// document states every param, and the next save writes them all.
    pub fn adopt_unset_params(&mut self, source_doc: &Graph) {
        for node in self.nodes.values_mut() {
            let unset = std::mem::take(&mut node.unset_params);
            let Some(block) = node.source_block else { continue };
            let Some(from) = source_doc
                .nodes
                .values()
                .find(|n| n.kind == node.kind && n.source_block == Some(block))
            else {
                continue;
            };
            for key in unset {
                if let Some(v) = from.param(key) {
                    node.set_param(key, v.clone());
                }
            }
        }
    }

    pub fn from_json(json: &Json) -> Option<Graph> {
        let mut graph = Graph::new();
        for nj in json.get("nodes")?.as_arr()? {
            let id = NodeId(nj.get("id")?.as_u64()?);
            let kind = NodeKind::from_stable_id(nj.get("kind")?.as_str()?)?;
            let pos_arr = nj.get("pos")?.as_arr()?;
            let pos = [pos_arr.first()?.as_f32()?, pos_arr.get(1)?.as_f32()?];
            let mut node = GraphNode::new(kind, pos);
            node.source_block = nj.get("src").and_then(|v| v.as_u64()).map(|v| v as u32);
            // `and_then` throughout, like every other optional field: a
            // graph written before this existed, or one whose hex is
            // damaged, must load WITHOUT it rather than fail to load.
            node.authored = nj.get("authored").and_then(|a| {
                let mut blocks = std::collections::BTreeMap::new();
                for bj in a.get("blocks")?.as_arr()? {
                    let i = bj.get("i")?.as_u64()? as u32;
                    blocks.insert(
                        i,
                        aether_nif::Block {
                            type_name: bj.get("type")?.as_str()?.to_string(),
                            data: AuthoredBlock::from_hex(bj.get("hex")?.as_str()?)?,
                        },
                    );
                }
                let root = a.get("root")?.as_u64()? as u32;
                blocks.contains_key(&root).then_some(AuthoredBlock {
                    root,
                    blocks,
                    sse: a.get("sse").and_then(Json::as_bool).unwrap_or(true),
                })
            });
            let stored = nj.get("params").and_then(|p| p.entries()).unwrap_or(&[]);
            node.unset_params = node
                .params
                .iter()
                .map(|(k, _)| *k)
                .filter(|k| !stored.iter().any(|(sk, _)| sk == k))
                .collect();
            if let Some(params) = nj.get("params").and_then(|p| p.entries()) {
                for (key, vj) in params {
                    let proto = node.param(key).cloned();
                    if let Some(proto) = proto {
                        if let Some(v) = Value::from_json(vj, &proto) {
                            node.set_param(key, v);
                        }
                    }
                }
            }
            graph.insert_with_id(id, node);
        }
        for lj in json.get("links")?.as_arr()? {
            let a = lj.as_arr()?;
            if a.len() != 4 {
                return None;
            }
            let link = Link {
                from_node: NodeId(a[0].as_u64()?),
                from_socket: a[1].as_u64()? as usize,
                to_node: NodeId(a[2].as_u64()?),
                to_socket: a[3].as_u64()? as usize,
            };
            // Tolerate stale links instead of failing the whole load.
            let _ = graph.connect(link);
        }
        // Clips are optional, like every field added after the format was
        // first written; a damaged track is dropped, not the document.
        for cj in json.get("clips").and_then(Json::as_arr).unwrap_or(&[]) {
            let Some(name) = cj.get("name").and_then(Json::as_str) else { continue };
            let mut tracks = Vec::new();
            for tj in cj.get("tracks").and_then(Json::as_arr).unwrap_or(&[]) {
                let (Some(node), Some(param), Some(curve)) = (
                    tj.get("node").and_then(Json::as_u64).map(NodeId),
                    tj.get("param").and_then(Json::as_str),
                    tj.get("curve").and_then(crate::curve::Curve::from_json),
                ) else {
                    continue;
                };
                // The param key is the schema's own `&'static str`, found
                // on the node -- a track on a param the node no longer has
                // has nothing to drive.
                let Some(key) = graph
                    .nodes
                    .get(&node)
                    .and_then(|n| n.params.iter().find(|(k, _)| *k == param).map(|(k, _)| *k))
                else {
                    continue;
                };
                tracks.push(crate::clip::ClipTrack {
                    node,
                    param: key,
                    curve,
                    source_slot: tj.get("slot").and_then(Json::as_u64).map(|s| s as u32),
                });
            }
            let text_keys = cj
                .get("text")
                .and_then(Json::as_arr)
                .unwrap_or(&[])
                .iter()
                .filter_map(|k| {
                    let a = k.as_arr()?;
                    Some((a.first()?.as_f32()?, a.get(1)?.as_str()?.to_string()))
                })
                .collect();
            graph.clips.push(crate::clip::Clip {
                name: name.to_string(),
                looping: cj.get("loop").and_then(Json::as_bool).unwrap_or(false),
                start_time: cj.get("start").and_then(Json::as_f32).unwrap_or(0.0),
                stop_time: cj.get("stop").and_then(Json::as_f32).unwrap_or(0.0),
                frequency: cj.get("freq").and_then(Json::as_f32).unwrap_or(1.0),
                text_keys,
                tracks,
                source_block: cj.get("src").and_then(Json::as_u64).map(|b| b as u32),
            });
        }
        Some(graph)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_connection_rules() {
        let mut g = Graph::new();
        let sys = g.insert(GraphNode::new(NodeKind::ParticleSystem, [0.0, 0.0]));
        let data = g.insert(GraphNode::new(NodeKind::ParticleData, [0.0, 0.0]));
        let shader = g.insert(GraphNode::new(NodeKind::EffectShader, [0.0, 0.0]));

        // Data -> Data input: ok.
        g.connect(Link {
            from_node: data,
            from_socket: 0,
            to_node: sys,
            to_socket: 0,
        })
        .unwrap();
        // Shader -> Data input: type mismatch.
        let err = g
            .connect(Link {
                from_node: shader,
                from_socket: 0,
                to_node: sys,
                to_socket: 0,
            })
            .unwrap_err();
        assert!(matches!(err, ConnectError::TypeMismatch { .. }));
        // Second Data into a single-input socket: occupied.
        let data2 = g.insert(GraphNode::new(NodeKind::ParticleData, [0.0, 0.0]));
        let err = g
            .connect(Link {
                from_node: data2,
                from_socket: 0,
                to_node: sys,
                to_socket: 0,
            })
            .unwrap_err();
        assert!(matches!(err, ConnectError::InputOccupied));
    }

    #[test]
    fn multi_input_accepts_many() {
        let mut g = Graph::new();
        let sys = g.insert(GraphNode::new(NodeKind::ParticleSystem, [0.0, 0.0]));
        let e1 = g.insert(GraphNode::new(NodeKind::BoxEmitter, [0.0, 0.0]));
        let e2 = g.insert(GraphNode::new(NodeKind::GravityModifier, [0.0, 0.0]));
        // "Modifiers" is input index 3 on ParticleSystem.
        g.connect(Link {
            from_node: e1,
            from_socket: 0,
            to_node: sys,
            to_socket: 3,
        })
        .unwrap();
        g.connect(Link {
            from_node: e2,
            from_socket: 0,
            to_node: sys,
            to_socket: 3,
        })
        .unwrap();
        assert_eq!(g.incoming(sys, 3).count(), 2);
    }

    /// `cycle_rejected` used to live here and it never tested a cycle.
    ///
    /// It connected an emitter to a controller and then tried the
    /// reverse, asserting only `is_err()`. The reverse link is a socket
    /// TYPE MISMATCH — measured: `TypeMismatch { from: Controller, to:
    /// Object }` — so it was refused before the cycle check was ever
    /// consulted, and the test passed whether or not that check existed.
    /// Its own comment said "the cycle check fires first on matching
    /// types", which is exactly backwards.
    ///
    /// The truth is stronger and is what these two tests pin instead.
    ///
    /// NO CYCLE IS REACHABLE AT ALL. Measured over all 28 node kinds:
    /// the "some output of A can connect to some input of B" relation is
    /// a DAG, its transitive closure puts no kind on a cycle, and no
    /// kind can even link to itself. So `ConnectError::WouldCycle` and
    /// `ConnectError::SelfLink` cannot fire for any graph the editor can
    /// build.
    ///
    /// The guard stays — it costs one traversal and the schema is
    /// edited often — but the acyclicity is the property worth pinning,
    /// because a schema change that breaks it is also what would make
    /// the compiler's own recursive walks non-terminating. If this test
    /// ever fails, the right response is to write the cycle test that
    /// was missing all along, using the pair it names.
    #[test]
    fn the_socket_schema_admits_no_cycles() {
        let n = ALL_NODE_KINDS.len();
        let mut reach = vec![vec![false; n]; n];
        for (i, a) in ALL_NODE_KINDS.iter().enumerate() {
            for (j, b) in ALL_NODE_KINDS.iter().enumerate() {
                let (sa, sb) = (a.schema(), b.schema());
                reach[i][j] = sa
                    .outputs
                    .iter()
                    .any(|o| sb.inputs.iter().any(|inp| o.ty.can_connect_to(inp.ty)));
            }
        }
        for k in 0..n {
            for i in 0..n {
                for j in 0..n {
                    if reach[i][k] && reach[k][j] {
                        reach[i][j] = true;
                    }
                }
            }
        }
        let looping: Vec<_> = (0..n)
            .filter(|i| reach[*i][*i])
            .map(|i| ALL_NODE_KINDS[i])
            .collect();
        assert!(
            looping.is_empty(),
            "these kinds can now reach themselves, so a cycle is buildable and the \
             `WouldCycle` guard needs a real test: {looping:?}"
        );
        // And the relation is not empty, or the loop above proves nothing.
        let edges = reach.iter().flatten().filter(|x| **x).count();
        assert!(edges > 20, "only {edges} connectable kind pairs");
    }

    /// The traversal the guard rests on, tested directly since no graph
    /// the schema allows can exercise it end to end.
    ///
    /// Planted defect: make `reaches` return false always (or stop
    /// following links past the first hop) — the acyclicity test above
    /// cannot notice, because it never builds a graph.
    #[test]
    fn reachability_follows_links_downstream() {
        let mut g = Graph::new();
        let data = g.insert(GraphNode::new(NodeKind::ParticleData, [0.0, 0.0]));
        let sys = g.insert(GraphNode::new(NodeKind::ParticleSystem, [0.0, 0.0]));
        let out = g.insert(GraphNode::new(NodeKind::SceneOutput, [0.0, 0.0]));
        g.connect(Link {
            from_node: data,
            from_socket: 0,
            to_node: sys,
            to_socket: 0,
        })
        .unwrap();
        g.connect(Link {
            from_node: sys,
            from_socket: 0,
            to_node: out,
            to_socket: 0,
        })
        .unwrap();
        // Two hops, so a traversal that stops at depth one fails.
        assert!(g.reaches(data, out), "data -> sys -> out");
        assert!(g.reaches(data, sys));
        // ...and it is DIRECTED: upstream is not reachable.
        assert!(!g.reaches(out, data), "links are one-way");
        assert!(!g.reaches(sys, data));
    }

    #[test]
    fn graph_json_roundtrip() {
        let mut g = Graph::new();
        let sys = g.insert(GraphNode::new(NodeKind::ParticleSystem, [10.0, 20.0]));
        let data = g.insert(GraphNode::new(NodeKind::ParticleData, [-50.0, 0.0]));
        g.nodes
            .get_mut(&data)
            .unwrap()
            .set_param("Max Particles", Value::U32(500));
        g.connect(Link {
            from_node: data,
            from_socket: 0,
            to_node: sys,
            to_socket: 0,
        })
        .unwrap();

        let json = g.to_json();
        let restored = Graph::from_json(&json).unwrap();
        assert_eq!(restored.nodes.len(), 2);
        assert_eq!(restored.links.len(), 1);
        assert_eq!(
            restored.nodes[&data].param("Max Particles"),
            Some(&Value::U32(500))
        );
    }
}


#[cfg(test)]
mod meta_tests {
    use super::*;

    /// Every parameter of every node must carry presentation metadata —
    /// the editor promises a tooltip on everything, and a new param
    /// without one should fail here, not ship silent.
    #[test]
    fn every_param_has_metadata() {
        let mut naked = Vec::new();
        for kind in ALL_NODE_KINDS {
            for (key, _) in (kind.schema().params)() {
                match param_meta(*kind, key) {
                    Some(m) => assert!(
                        !m.tooltip.is_empty(),
                        "{kind:?} :: {key} has an empty tooltip"
                    ),
                    None => naked.push(format!("{kind:?} :: {key}")),
                }
            }
        }
        assert!(naked.is_empty(), "params without metadata: {naked:#?}");
    }
}
