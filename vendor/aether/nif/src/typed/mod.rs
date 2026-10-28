//! Typed views over raw block payloads.
//!
//! Layouts follow nif.xml for file version 20.2.0.7, user version 12,
//! BS stream 83 (Skyrim LE) / 100 (Skyrim SE), cross-checked against nifly.
//! Every block type has ONE `sync` function used for both decode and encode
//! (symmetric streaming, nifly-style), so a layout exists in exactly one place.
//!
//! Decoding requires consuming the entire payload; a leftover byte means the
//! layout is wrong for that file and we refuse rather than guess. Because of
//! that rule, a byte-identical re-encode is guaranteed whenever fields are
//! untouched, keeping the editor's lossless round-trip invariant.

pub mod controllers;
pub mod havok;
pub mod geometry;
pub mod particles;
pub mod shaders;

use crate::error::{Error, Result};
use crate::file::Block;
use crate::header::{BsStreamVersion, Header};
use crate::stream::{Reader, Writer};

/// Serialization context: the few version facts block layouts depend on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ctx {
    /// true = Skyrim SE (bs100), false = Skyrim LE (bs83).
    pub sse: bool,
}

impl Ctx {
    pub fn of(header: &Header) -> Result<Self> {
        match header.bs_version {
            BsStreamVersion::SkyrimSE => Ok(Ctx { sse: true }),
            BsStreamVersion::SkyrimLE => Ok(Ctx { sse: false }),
            BsStreamVersion::Other(v) => Err(Error::Corrupt {
                offset: 0,
                what: format!("typed views need bs83/bs100, file has bs{v}"),
            }),
        }
    }

    pub fn sse() -> Self {
        Ctx { sse: true }
    }

    pub fn le() -> Self {
        Ctx { sse: false }
    }
}

/// Reference to another block. `-1` (`NONE`) means "no block".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockRef(pub i32);

/// A ref nobody has set points at NOTHING, not at block 0.
///
/// Derived, `Default` was `i32::default()` = 0, which is a valid index —
/// the file's ROOT, usually. Every block the compiler builds from
/// `..Default::default()` without touching its refs therefore claimed
/// the root: measured on a null edit, every `BSEffectShaderProperty`
/// and every `NiAlphaProperty` in the apparatus came back with
/// `controller: BSFadeNode`, which is not even a controller type.
/// Decoding always assigns explicitly, so this only ever affected
/// blocks built from scratch — i.e. everything the compiler emits.
impl Default for BlockRef {
    fn default() -> Self {
        BlockRef::NONE
    }
}

impl BlockRef {
    pub const NONE: BlockRef = BlockRef(-1);

    pub fn new(index: usize) -> Self {
        BlockRef(index as i32)
    }

    pub fn get(self) -> Option<usize> {
        (self.0 >= 0).then_some(self.0 as usize)
    }
}

/// Index into the header string table. `0xFFFFFFFF` (`NONE`) means "no string".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StringIdx(pub u32);

impl StringIdx {
    pub const NONE: StringIdx = StringIdx(u32::MAX);

    pub fn resolve<'h>(self, header: &'h Header) -> Option<&'h str> {
        header.string(self.0)
    }
}

impl Default for StringIdx {
    fn default() -> Self {
        StringIdx::NONE
    }
}

/// Symmetric field streamer: one implementation reads, the other writes, so
/// each block layout is written down exactly once in its `sync` function.
pub trait Nio {
    fn reading(&self) -> bool;
    fn u8(&mut self, v: &mut u8) -> Result<()>;
    fn u16(&mut self, v: &mut u16) -> Result<()>;
    fn u32(&mut self, v: &mut u32) -> Result<()>;
    fn u64(&mut self, v: &mut u64) -> Result<()>;
    fn i32(&mut self, v: &mut i32) -> Result<()>;
    fn f32(&mut self, v: &mut f32) -> Result<()>;
    fn sized_string(&mut self, v: &mut String) -> Result<()>;

    /// NIF `bool` at 20.2.0.7 is a single byte.
    fn bool8(&mut self, v: &mut bool) -> Result<()> {
        let mut b = u8::from(*v);
        self.u8(&mut b)?;
        *v = b != 0;
        Ok(())
    }

    fn block_ref(&mut self, v: &mut BlockRef) -> Result<()> {
        self.i32(&mut v.0)
    }

    /// A back-POINTER (nif.xml `Ptr`): the same four bytes on disk as a
    /// `Ref`, but the target does not belong to this block — it points
    /// back up the tree, or sideways at something another block owns.
    ///
    /// The distinction is invisible to reading and writing and exists
    /// for the visitors: an ownership walk must descend through `Ref`
    /// and stop at `Ptr`, or a modifier's `target` would make its own
    /// particle system look like its child. nifly draws the same line
    /// with separate `GetChildRefs` / `GetPtrs` virtuals per class.
    fn block_ptr(&mut self, v: &mut BlockRef) -> Result<()> {
        self.block_ref(v)
    }

    fn string_idx(&mut self, v: &mut StringIdx) -> Result<()> {
        self.u32(&mut v.0)
    }

    fn f32s(&mut self, v: &mut [f32]) -> Result<()> {
        for x in v.iter_mut() {
            self.f32(x)?;
        }
        Ok(())
    }

    /// A fixed run of raw bytes: nif.xml's `binary="true"` padding, and
    /// the MOPP tree, which is an opaque blob by design.
    fn bytes(&mut self, v: &mut [u8]) -> Result<()> {
        for x in v.iter_mut() {
            self.u8(x)?;
        }
        Ok(())
    }

    fn ref_list(&mut self, v: &mut Vec<BlockRef>) -> Result<()> {
        let mut n = v.len() as u32;
        self.u32(&mut n)?;
        if self.reading() {
            check_len(n)?;
            v.clear();
            v.resize(n as usize, BlockRef::NONE);
        }
        for r in v.iter_mut() {
            self.block_ref(r)?;
        }
        Ok(())
    }

    /// The `NiObjectNET` header — name, extra data, controller — named
    /// as ONE field so a visitor can find it.
    ///
    /// Every named block in the format starts with this: scene nodes,
    /// shapes, particle systems, properties, modifiers. One hook labels
    /// all of them; the alternative was a match over ninety types that
    /// resolves 2-27% of a file (what `MeshScene` already knew) and
    /// drifts the first time a type is added.
    fn object_net(&mut self, net: &mut ObjectNet, _ctx: Ctx) -> Result<()> {
        self.string_idx(&mut net.name)?;
        self.ref_list(&mut net.extra_data)?;
        self.block_ref(&mut net.controller)
    }

    /// The `NiAVObject` header, named as ONE field so a visitor can find
    /// it.
    ///
    /// `AvObject::sync` emitted its fields as anonymous scalars, so there
    /// was no way to read or write a block's transform generically — and
    /// the export splice quietly rebuilt them from
    /// `AvObject::default()`, which is how a null edit came to clear the
    /// hidden bit and reset authored scales. The default body IS the byte
    /// sequence it always was, so the reader and the writer are
    /// untouched; only the visitors override it.
    fn av_object(&mut self, av: &mut AvObject, _ctx: Ctx) -> Result<()> {
        // Through the sibling hook, not `ObjectNet::sync`: that takes
        // `&mut dyn Nio`, and re-casting `self` to a trait object inside
        // a default method of that same trait needs `Self: Sized`, which
        // would make this hook the one thing it exists not to be —
        // non-dispatchable.
        self.object_net(&mut av.net, _ctx)?;
        self.u32(&mut av.flags)?;
        self.f32s(&mut av.translation)?;
        self.f32s(&mut av.rotation)?;
        self.f32(&mut av.scale)?;
        self.block_ref(&mut av.collision)
    }

    /// `ref_list` for back-pointers.
    fn ptr_list(&mut self, v: &mut Vec<BlockRef>) -> Result<()> {
        let mut n = v.len() as u32;
        self.u32(&mut n)?;
        if self.reading() {
            check_len(n)?;
            v.clear();
            v.resize(n as usize, BlockRef::NONE);
        }
        for r in v.iter_mut() {
            self.block_ptr(r)?;
        }
        Ok(())
    }

    fn f32_list(&mut self, v: &mut Vec<f32>) -> Result<()> {
        let mut n = v.len() as u32;
        self.u32(&mut n)?;
        if self.reading() {
            check_len(n)?;
            v.clear();
            v.resize(n as usize, 0.0);
        }
        for x in v.iter_mut() {
            self.f32(x)?;
        }
        Ok(())
    }

    fn vec4_list(&mut self, v: &mut Vec<[f32; 4]>) -> Result<()> {
        let mut n = v.len() as u32;
        self.u32(&mut n)?;
        if self.reading() {
            check_len(n)?;
            v.clear();
            v.resize(n as usize, [0.0; 4]);
        }
        for x in v.iter_mut() {
            self.f32s(x)?;
        }
        Ok(())
    }
}

pub(crate) fn check_len(n: u32) -> Result<()> {
    // A list longer than this cannot fit in any real block payload; refuse
    // early instead of attempting a huge allocation on corrupt data.
    if n > 0x0100_0000 {
        return Err(Error::Corrupt {
            offset: 0,
            what: format!("list length {n} is implausible"),
        });
    }
    Ok(())
}

struct ReadNio<'a, 'b> {
    r: &'b mut Reader<'a>,
}

impl Nio for ReadNio<'_, '_> {
    fn reading(&self) -> bool {
        true
    }
    fn u8(&mut self, v: &mut u8) -> Result<()> {
        *v = self.r.u8()?;
        Ok(())
    }
    fn u16(&mut self, v: &mut u16) -> Result<()> {
        *v = self.r.u16()?;
        Ok(())
    }
    fn u32(&mut self, v: &mut u32) -> Result<()> {
        *v = self.r.u32()?;
        Ok(())
    }
    fn u64(&mut self, v: &mut u64) -> Result<()> {
        *v = self.r.u64()?;
        Ok(())
    }
    fn i32(&mut self, v: &mut i32) -> Result<()> {
        *v = self.r.i32()?;
        Ok(())
    }
    fn f32(&mut self, v: &mut f32) -> Result<()> {
        *v = self.r.f32()?;
        Ok(())
    }
    fn sized_string(&mut self, v: &mut String) -> Result<()> {
        *v = self.r.sized_string()?;
        Ok(())
    }
}

struct WriteNio<'b> {
    w: &'b mut Writer,
}

impl Nio for WriteNio<'_> {
    fn reading(&self) -> bool {
        false
    }
    fn u8(&mut self, v: &mut u8) -> Result<()> {
        self.w.u8(*v);
        Ok(())
    }
    fn u16(&mut self, v: &mut u16) -> Result<()> {
        self.w.u16(*v);
        Ok(())
    }
    fn u32(&mut self, v: &mut u32) -> Result<()> {
        self.w.u32(*v);
        Ok(())
    }
    fn u64(&mut self, v: &mut u64) -> Result<()> {
        self.w.bytes(&v.to_le_bytes());
        Ok(())
    }
    fn i32(&mut self, v: &mut i32) -> Result<()> {
        self.w.i32(*v);
        Ok(())
    }
    fn f32(&mut self, v: &mut f32) -> Result<()> {
        self.w.f32(*v);
        Ok(())
    }
    fn sized_string(&mut self, v: &mut String) -> Result<()> {
        self.w.sized_string(v);
        Ok(())
    }
}

/// A block type with a typed view. `TYPE_NAMES` lists the on-disk block type
/// names this layout covers (e.g. `NiNode` and `BSFadeNode` share a layout).
pub trait TypedBlock: Default {
    const TYPE_NAMES: &'static [&'static str];

    fn sync(&mut self, s: &mut dyn Nio, ctx: Ctx) -> Result<()>;

    /// Decode from a raw block. The entire payload must be consumed; leftover
    /// bytes mean the layout does not match the file and decoding fails.
    fn decode(block: &Block, ctx: Ctx) -> Result<Self> {
        if !Self::TYPE_NAMES.contains(&block.type_name.as_str()) {
            return Err(Error::Corrupt {
                offset: 0,
                what: format!(
                    "expected one of {:?}, found block type {}",
                    Self::TYPE_NAMES,
                    block.type_name
                ),
            });
        }
        let mut r = Reader::new(&block.data);
        let mut value = Self::default();
        value.sync(&mut ReadNio { r: &mut r }, ctx)?;
        if !r.is_at_end() {
            return Err(Error::Corrupt {
                offset: r.pos(),
                what: format!(
                    "{}: {} bytes left after decoding — layout mismatch",
                    block.type_name,
                    r.remaining()
                ),
            });
        }
        Ok(value)
    }

    /// Encode into a raw block payload using the first (canonical) type name.
    fn encode(&mut self, ctx: Ctx) -> Block {
        self.encode_as(Self::TYPE_NAMES[0], ctx)
    }

    /// Encode under a specific type name (for layout-sharing types).
    fn encode_as(&mut self, type_name: &str, ctx: Ctx) -> Block {
        debug_assert!(Self::TYPE_NAMES.contains(&type_name));
        let mut w = Writer::new();
        self.sync(&mut WriteNio { w: &mut w }, ctx)
            .expect("encode cannot fail: writing has no length checks");
        Block {
            type_name: type_name.to_string(),
            data: w.into_bytes(),
        }
    }
}

/// `NiObjectNET`: name + extra data list + controller chain head.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ObjectNet {
    pub name: StringIdx,
    pub extra_data: Vec<BlockRef>,
    pub controller: BlockRef,
}

impl ObjectNet {
    pub fn sync(&mut self, s: &mut dyn Nio, ctx: Ctx) -> Result<()> {
        s.object_net(self, ctx)
    }
}

/// `NiAVObject`: scene-graph object base (flags + transform + collision).
#[derive(Debug, Clone, PartialEq)]
pub struct AvObject {
    pub net: ObjectNet,
    pub flags: u32,
    pub translation: [f32; 3],
    /// Row-major 3x3 rotation matrix.
    pub rotation: [f32; 9],
    pub scale: f32,
    pub collision: BlockRef,
}

impl Default for AvObject {
    fn default() -> Self {
        AvObject {
            net: ObjectNet::default(),
            flags: 0x8000E,
            translation: [0.0; 3],
            rotation: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
            scale: 1.0,
            collision: BlockRef::NONE,
        }
    }
}

impl AvObject {
    pub fn sync(&mut self, s: &mut dyn Nio, ctx: Ctx) -> Result<()> {
        s.av_object(self, ctx)
    }
}

/// `NiBound`: center + radius.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Bound {
    pub center: [f32; 3],
    pub radius: f32,
}

impl Bound {
    pub fn sync(&mut self, s: &mut dyn Nio) -> Result<()> {
        s.f32s(&mut self.center)?;
        s.f32(&mut self.radius)
    }
}

// ===================== the typed-view registry =====================

/// Which kind of edge a block reference is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefKind {
    /// nif.xml `Ref`: this block owns the target.
    Child,
    /// nif.xml `Ptr`: a back-pointer, owned by someone else.
    Ptr,
}

/// A third `Nio`, alongside the reader and the writer: it runs a block's
/// own `sync` in WRITE mode but throws every byte away and keeps only
/// the block references.
///
/// That is what makes it free. Every `sync` already names each ref
/// exactly once, so one small visitor gives nifly's `GetChildRefs` +
/// `GetPtrs` behaviour across all 87 typed views with no per-type code —
/// and a typed block written tomorrow gets it the moment it exists,
/// instead of needing two more hand-written virtuals nobody remembers.
struct RefVisitor<'a> {
    out: &'a mut Vec<(BlockRef, RefKind)>,
}

impl Nio for RefVisitor<'_> {
    // Write mode: lengths and layout come from the value itself, exactly
    // as when encoding, so the walk sees the same fields the file does.
    fn reading(&self) -> bool {
        false
    }
    fn u8(&mut self, _: &mut u8) -> Result<()> {
        Ok(())
    }
    fn u16(&mut self, _: &mut u16) -> Result<()> {
        Ok(())
    }
    fn u32(&mut self, _: &mut u32) -> Result<()> {
        Ok(())
    }
    fn u64(&mut self, _: &mut u64) -> Result<()> {
        Ok(())
    }
    fn i32(&mut self, _: &mut i32) -> Result<()> {
        Ok(())
    }
    fn f32(&mut self, _: &mut f32) -> Result<()> {
        Ok(())
    }
    fn sized_string(&mut self, _: &mut String) -> Result<()> {
        Ok(())
    }
    fn block_ref(&mut self, v: &mut BlockRef) -> Result<()> {
        self.out.push((*v, RefKind::Child));
        Ok(())
    }
    fn block_ptr(&mut self, v: &mut BlockRef) -> Result<()> {
        self.out.push((*v, RefKind::Ptr));
        Ok(())
    }
}

/// The mutating twin of `RefVisitor`: every reference is handed to the
/// caller and replaced by what comes back. Everything else is untouched,
/// so re-encoding an unchanged block reproduces its bytes exactly.
struct RetargetVisitor<'a> {
    f: &'a mut dyn FnMut(BlockRef, RefKind) -> BlockRef,
}

impl Nio for RetargetVisitor<'_> {
    fn reading(&self) -> bool {
        false
    }
    fn u8(&mut self, _: &mut u8) -> Result<()> {
        Ok(())
    }
    fn u16(&mut self, _: &mut u16) -> Result<()> {
        Ok(())
    }
    fn u32(&mut self, _: &mut u32) -> Result<()> {
        Ok(())
    }
    fn u64(&mut self, _: &mut u64) -> Result<()> {
        Ok(())
    }
    fn i32(&mut self, _: &mut i32) -> Result<()> {
        Ok(())
    }
    fn f32(&mut self, _: &mut f32) -> Result<()> {
        Ok(())
    }
    fn sized_string(&mut self, _: &mut String) -> Result<()> {
        Ok(())
    }
    fn block_ref(&mut self, v: &mut BlockRef) -> Result<()> {
        *v = (self.f)(*v, RefKind::Child);
        Ok(())
    }
    fn block_ptr(&mut self, v: &mut BlockRef) -> Result<()> {
        *v = (self.f)(*v, RefKind::Ptr);
        Ok(())
    }
}

fn retarget<T: TypedBlock>(
    block: &Block,
    ctx: Ctx,
    f: &mut dyn FnMut(BlockRef, RefKind) -> BlockRef,
) -> Option<Block> {
    let mut value = T::decode(block, ctx).ok()?;
    value.sync(&mut RetargetVisitor { f }, ctx).ok()?;
    Some(value.encode_as(&block.type_name, ctx))
}

/// Reads the first `NiAVObject` header a block carries, ignoring
/// everything else.
struct AvReader {
    found: Option<AvObject>,
}

impl Nio for AvReader {
    fn reading(&self) -> bool {
        false
    }
    fn u8(&mut self, _: &mut u8) -> Result<()> {
        Ok(())
    }
    fn u16(&mut self, _: &mut u16) -> Result<()> {
        Ok(())
    }
    fn u32(&mut self, _: &mut u32) -> Result<()> {
        Ok(())
    }
    fn u64(&mut self, _: &mut u64) -> Result<()> {
        Ok(())
    }
    fn i32(&mut self, _: &mut i32) -> Result<()> {
        Ok(())
    }
    fn f32(&mut self, _: &mut f32) -> Result<()> {
        Ok(())
    }
    fn sized_string(&mut self, _: &mut String) -> Result<()> {
        Ok(())
    }
    fn av_object(&mut self, av: &mut AvObject, _ctx: Ctx) -> Result<()> {
        if self.found.is_none() {
            self.found = Some(av.clone());
        }
        Ok(())
    }
}

/// Replaces the first `NiAVObject` header with a given one.
struct AvWriter<'a> {
    av: &'a AvObject,
    done: bool,
}

impl Nio for AvWriter<'_> {
    fn reading(&self) -> bool {
        false
    }
    fn u8(&mut self, _: &mut u8) -> Result<()> {
        Ok(())
    }
    fn u16(&mut self, _: &mut u16) -> Result<()> {
        Ok(())
    }
    fn u32(&mut self, _: &mut u32) -> Result<()> {
        Ok(())
    }
    fn u64(&mut self, _: &mut u64) -> Result<()> {
        Ok(())
    }
    fn i32(&mut self, _: &mut i32) -> Result<()> {
        Ok(())
    }
    fn f32(&mut self, _: &mut f32) -> Result<()> {
        Ok(())
    }
    fn sized_string(&mut self, _: &mut String) -> Result<()> {
        Ok(())
    }
    fn av_object(&mut self, av: &mut AvObject, _ctx: Ctx) -> Result<()> {
        if !self.done {
            *av = self.av.clone();
            self.done = true;
        }
        Ok(())
    }
}

/// One scalar of a block's layout, in the order `sync` visits it.
///
/// The `Nio` trait bottoms out in exactly seven primitives, so a block —
/// any block, of any type — is a flat sequence of these. That is what
/// makes a field-wise merge possible without a per-type table of "which
/// fields does the editor own": walk two blocks of the same type in
/// lockstep and the i-th scalar is the same field in both.
#[derive(Debug, Clone, PartialEq)]
pub enum Scalar {
    U8(u8),
    U16(u16),
    U32(u32),
    U64(u64),
    I32(i32),
    F32(f32),
    Str(String),
}

/// Records every scalar a block's layout visits (write direction: the
/// values come OUT of the decoded struct).
#[derive(Default)]
struct Flatten {
    out: Vec<Scalar>,
}

impl Nio for Flatten {
    fn reading(&self) -> bool {
        false
    }
    fn u8(&mut self, v: &mut u8) -> Result<()> {
        self.out.push(Scalar::U8(*v));
        Ok(())
    }
    fn u16(&mut self, v: &mut u16) -> Result<()> {
        self.out.push(Scalar::U16(*v));
        Ok(())
    }
    fn u32(&mut self, v: &mut u32) -> Result<()> {
        self.out.push(Scalar::U32(*v));
        Ok(())
    }
    fn u64(&mut self, v: &mut u64) -> Result<()> {
        self.out.push(Scalar::U64(*v));
        Ok(())
    }
    fn i32(&mut self, v: &mut i32) -> Result<()> {
        self.out.push(Scalar::I32(*v));
        Ok(())
    }
    fn f32(&mut self, v: &mut f32) -> Result<()> {
        self.out.push(Scalar::F32(*v));
        Ok(())
    }
    fn sized_string(&mut self, v: &mut String) -> Result<()> {
        self.out.push(Scalar::Str(v.clone()));
        Ok(())
    }
}

/// Feeds a recorded scalar sequence back INTO a struct (read direction),
/// so counts drive the same resizes the file reader performs.
struct Unflatten<'a> {
    vals: &'a [Scalar],
    at: usize,
    mismatch: bool,
}

impl Unflatten<'_> {
    fn next(&mut self) -> Option<Scalar> {
        let v = self.vals.get(self.at).cloned();
        self.at += 1;
        if v.is_none() {
            self.mismatch = true;
        }
        v
    }
}

impl Nio for Unflatten<'_> {
    fn reading(&self) -> bool {
        true
    }
    fn u8(&mut self, v: &mut u8) -> Result<()> {
        if let Some(Scalar::U8(x)) = self.next() {
            *v = x;
        } else {
            self.mismatch = true;
        }
        Ok(())
    }
    fn u16(&mut self, v: &mut u16) -> Result<()> {
        if let Some(Scalar::U16(x)) = self.next() {
            *v = x;
        } else {
            self.mismatch = true;
        }
        Ok(())
    }
    fn u32(&mut self, v: &mut u32) -> Result<()> {
        if let Some(Scalar::U32(x)) = self.next() {
            *v = x;
        } else {
            self.mismatch = true;
        }
        Ok(())
    }
    fn u64(&mut self, v: &mut u64) -> Result<()> {
        if let Some(Scalar::U64(x)) = self.next() {
            *v = x;
        } else {
            self.mismatch = true;
        }
        Ok(())
    }
    fn i32(&mut self, v: &mut i32) -> Result<()> {
        if let Some(Scalar::I32(x)) = self.next() {
            *v = x;
        } else {
            self.mismatch = true;
        }
        Ok(())
    }
    fn f32(&mut self, v: &mut f32) -> Result<()> {
        if let Some(Scalar::F32(x)) = self.next() {
            *v = x;
        } else {
            self.mismatch = true;
        }
        Ok(())
    }
    fn sized_string(&mut self, v: &mut String) -> Result<()> {
        if let Some(Scalar::Str(x)) = self.next() {
            *v = x;
        } else {
            self.mismatch = true;
        }
        Ok(())
    }
}

/// Collects every header string index a block names.
#[derive(Default)]
struct StringUse {
    out: Vec<u32>,
}

impl Nio for StringUse {
    fn reading(&self) -> bool {
        false
    }
    fn u8(&mut self, _: &mut u8) -> Result<()> {
        Ok(())
    }
    fn u16(&mut self, _: &mut u16) -> Result<()> {
        Ok(())
    }
    fn u32(&mut self, _: &mut u32) -> Result<()> {
        Ok(())
    }
    fn u64(&mut self, _: &mut u64) -> Result<()> {
        Ok(())
    }
    fn i32(&mut self, _: &mut i32) -> Result<()> {
        Ok(())
    }
    fn f32(&mut self, _: &mut f32) -> Result<()> {
        Ok(())
    }
    fn sized_string(&mut self, _: &mut String) -> Result<()> {
        Ok(())
    }
    fn string_idx(&mut self, v: &mut StringIdx) -> Result<()> {
        if v.0 != u32::MAX {
            self.out.push(v.0);
        }
        Ok(())
    }
}

fn strings_of<T: TypedBlock>(block: &Block, ctx: Ctx) -> Option<Vec<u32>> {
    let mut value = T::decode(block, ctx).ok()?;
    let mut vis = StringUse::default();
    value.sync(&mut vis, ctx).ok()?;
    Some(vis.out)
}

fn flatten_of<T: TypedBlock>(block: &Block, ctx: Ctx) -> Option<Vec<Scalar>> {
    let mut value = T::decode(block, ctx).ok()?;
    let mut vis = Flatten::default();
    value.sync(&mut vis, ctx).ok()?;
    Some(vis.out)
}

fn rebuild_of<T: TypedBlock>(type_name: &str, ctx: Ctx, vals: &[Scalar]) -> Option<Block> {
    let mut value = T::default();
    let mut vis = Unflatten {
        vals,
        at: 0,
        mismatch: false,
    };
    value.sync(&mut vis, ctx).ok()?;
    if vis.mismatch || vis.at != vals.len() {
        return None;
    }
    Some(value.encode_as(type_name, ctx))
}

/// Reads the first `NiObjectNET` name a block carries.
struct NameReader {
    found: Option<StringIdx>,
}

impl Nio for NameReader {
    fn reading(&self) -> bool {
        false
    }
    fn u8(&mut self, _: &mut u8) -> Result<()> {
        Ok(())
    }
    fn u16(&mut self, _: &mut u16) -> Result<()> {
        Ok(())
    }
    fn u32(&mut self, _: &mut u32) -> Result<()> {
        Ok(())
    }
    fn u64(&mut self, _: &mut u64) -> Result<()> {
        Ok(())
    }
    fn i32(&mut self, _: &mut i32) -> Result<()> {
        Ok(())
    }
    fn f32(&mut self, _: &mut f32) -> Result<()> {
        Ok(())
    }
    fn sized_string(&mut self, _: &mut String) -> Result<()> {
        Ok(())
    }
    fn object_net(&mut self, net: &mut ObjectNet, _ctx: Ctx) -> Result<()> {
        if self.found.is_none() {
            self.found = Some(net.name);
        }
        Ok(())
    }
}

fn read_name<T: TypedBlock>(block: &Block, ctx: Ctx) -> Option<StringIdx> {
    let mut value = T::decode(block, ctx).ok()?;
    let mut vis = NameReader { found: None };
    value.sync(&mut vis, ctx).ok()?;
    vis.found
}

fn read_av<T: TypedBlock>(block: &Block, ctx: Ctx) -> Option<AvObject> {
    let mut value = T::decode(block, ctx).ok()?;
    let mut vis = AvReader { found: None };
    value.sync(&mut vis, ctx).ok()?;
    vis.found
}

fn write_av<T: TypedBlock>(block: &Block, ctx: Ctx, av: &AvObject) -> Option<Block> {
    let mut value = T::decode(block, ctx).ok()?;
    let mut vis = AvWriter { av, done: false };
    value.sync(&mut vis, ctx).ok()?;
    if !vis.done {
        return None;
    }
    Some(value.encode_as(&block.type_name, ctx))
}

fn scan_refs<T: TypedBlock>(block: &Block, ctx: Ctx) -> Option<Vec<(BlockRef, RefKind)>> {
    let mut value = T::decode(block, ctx).ok()?;
    let mut out = Vec::new();
    value.sync(&mut RefVisitor { out: &mut out }, ctx).ok()?;
    Some(out)
}

macro_rules! typed_registry {
    ($($m:ident :: $t:ident),* $(,)?) => {
        /// Every block reference this block holds, in field order, each
        /// tagged `Child` (owned) or `Ptr` (a back-pointer).
        ///
        /// `None` when the type has no typed view — the honest answer
        /// for `bhkRigidBody` and its family, and the reason the export
        /// splice never renumbers: a block whose refs cannot be read
        /// cannot be safely moved.
        pub fn block_refs(block: &Block, ctx: Ctx) -> Option<Vec<(BlockRef, RefKind)>> {
            let name = block.type_name.as_str();
            $(
                if <$m::$t as TypedBlock>::TYPE_NAMES.contains(&name) {
                    return scan_refs::<$m::$t>(block, ctx);
                }
            )*
            None
        }

        /// Rewrite every reference this block holds through `f`, and
        /// re-encode. `None` for a type with no typed view — those
        /// blocks can never be retargeted, which is exactly why the
        /// splice moves no index they might point at.
        pub fn retarget_refs(
            block: &Block,
            ctx: Ctx,
            f: &mut dyn FnMut(BlockRef, RefKind) -> BlockRef,
        ) -> Option<Block> {
            let name = block.type_name.as_str();
            $(
                if <$m::$t as TypedBlock>::TYPE_NAMES.contains(&name) {
                    return retarget::<$m::$t>(block, ctx, f);
                }
            )*
            None
        }

        /// The block's own name, if its layout carries one.
        pub fn block_name(block: &Block, ctx: Ctx) -> Option<StringIdx> {
            let name = block.type_name.as_str();
            $(
                if <$m::$t as TypedBlock>::TYPE_NAMES.contains(&name) {
                    return read_name::<$m::$t>(block, ctx);
                }
            )*
            None
        }

        /// The block's `NiAVObject` header, if it has one.
        ///
        /// Works for every scene node, every shape and every particle
        /// system with no per-type code — the alternative was a match
        /// over ninety types that would drift the first time one was
        /// added.
        pub fn block_av(block: &Block, ctx: Ctx) -> Option<AvObject> {
            let name = block.type_name.as_str();
            $(
                if <$m::$t as TypedBlock>::TYPE_NAMES.contains(&name) {
                    return read_av::<$m::$t>(block, ctx);
                }
            )*
            None
        }

        /// Put an `NiAVObject` header back, leaving the rest of the block
        /// alone. `None` when the block has no header to replace.
        pub fn set_block_av(block: &Block, ctx: Ctx, av: &AvObject) -> Option<Block> {
            let name = block.type_name.as_str();
            $(
                if <$m::$t as TypedBlock>::TYPE_NAMES.contains(&name) {
                    return write_av::<$m::$t>(block, ctx, av);
                }
            )*
            None
        }

        /// Apply to `source` only the fields the editor actually
        /// CHANGED, leaving every other byte of it alone.
        ///
        /// This is what NifSkope and nifly do and what a compiler that
        /// rebuilds a block from its own schema cannot: `authored` is
        /// what compiling the UNEDITED document produces, `edited` is
        /// what compiling the current one produces, so a field that
        /// differs between them is a field the user touched — and every
        /// other field keeps the source's authored value, including the
        /// ones the editor has no idea exist. A null edit therefore
        /// comes back byte-identical by construction rather than by
        /// modelling the format exhaustively.
        ///
        /// `None` when the three blocks do not share a layout (a
        /// different type, or an array whose length the edit changed).
        /// The caller must fall back to the edited block then: this is
        /// an improvement over rebuilding, never a substitute for
        /// correctness.
        pub fn merge_edit(
            source: &Block,
            authored: &Block,
            edited: &Block,
            ctx: Ctx,
        ) -> Option<Block> {
            let name = source.type_name.as_str();
            if authored.type_name != source.type_name || edited.type_name != source.type_name {
                return None;
            }
            $(
                if <$m::$t as TypedBlock>::TYPE_NAMES.contains(&name) {
                    let s = flatten_of::<$m::$t>(source, ctx)?;
                    let a = flatten_of::<$m::$t>(authored, ctx)?;
                    let e = flatten_of::<$m::$t>(edited, ctx)?;
                    if s.len() != a.len() || a.len() != e.len() {
                        return None;
                    }
                    let merged: Vec<Scalar> = (0..s.len())
                        .map(|i| if e[i] != a[i] { e[i].clone() } else { s[i].clone() })
                        .collect();
                    return rebuild_of::<$m::$t>(name, ctx, &merged);
                }
            )*
            None
        }

        /// This block as a flat scalar sequence, for diffing.
        ///
        /// The fourth derived reader off the same `sync`, beside
        /// `block_refs`, `block_av` and `used_strings`. It exists to
        /// answer "which FIELDS does the compiler still get wrong",
        /// which a byte comparison cannot: the export keeps the
        /// source bytes for every block an edit did not touch, so the
        /// remaining compiler error is invisible unless something
        /// looks at it directly.
        pub fn block_scalars(block: &Block, ctx: Ctx) -> Option<Vec<Scalar>> {
            let name = block.type_name.as_str();
            $(
                if <$m::$t as TypedBlock>::TYPE_NAMES.contains(&name) {
                    return flatten_of::<$m::$t>(block, ctx);
                }
            )*
            None
        }

        /// Every header string index this block names.
        ///
        /// `None` for a block with no typed view — which the caller must
        /// read as "cannot say", never as "names none": a string the
        /// scanner cannot see must not be trimmed away.
        pub fn used_strings(block: &Block, ctx: Ctx) -> Option<Vec<u32>> {
            let name = block.type_name.as_str();
            $(
                if <$m::$t as TypedBlock>::TYPE_NAMES.contains(&name) {
                    return strings_of::<$m::$t>(block, ctx);
                }
            )*
            None
        }

        /// Is there a typed view for this on-disk block type?
        pub fn has_typed_view(type_name: &str) -> bool {
            $(
                if <$m::$t as TypedBlock>::TYPE_NAMES.contains(&type_name) {
                    return true;
                }
            )*
            false
        }

        /// Every on-disk block type name a typed view covers, with
        /// repeats if two views claim one name. The registry's own test
        /// asserts there are none.
        pub fn typed_view_names() -> Vec<&'static str> {
            let mut names = Vec::new();
            $( names.extend_from_slice(<$m::$t as TypedBlock>::TYPE_NAMES); )*
            names
        }
    };
}

typed_registry! {
    havok::BhkCollisionObject,
    havok::BhkBlendCollisionObject,
    havok::BhkRigidBody,
    havok::BhkMoppBvTreeShape,
    havok::BhkListShape,
    havok::BhkTransformShape,
    havok::BhkBoxShape,
    havok::BhkSphereShape,
    havok::BhkCapsuleShape,
    havok::BhkCylinderShape,
    havok::BhkNiTriStripsShape,
    havok::BhkConvexVerticesShape,
    havok::BhkCompressedMeshShape,
    havok::BhkCompressedMeshShapeData,
    havok::BhkSimpleShapePhantom,
    havok::BhkRagdollConstraint,
    havok::BhkLimitedHingeConstraint,
    havok::BhkHingeConstraint,
    havok::BhkBallAndSocketConstraint,
    havok::BhkStiffSpringConstraint,
    havok::BhkBallSocketConstraintChain,
    havok::BhkBreakableConstraint,
    havok::BhkPlaneShape,
    geometry::NiCamera,
    particles::NiMeshParticleSystem,
    particles::NiMeshPSysData,
    particles::BSPSysHavokUpdateModifier,
    controllers::BSDistantObjectLargeRefExtraData,
    controllers::NiBooleanExtraData,
    controllers::NiBinaryExtraData,
    controllers::BSFurnitureMarkerNode,
    controllers::BSLagBoneController,
    controllers::BSFrustumFOVController,
    controllers::BSBoneLODExtraData,
    controllers::NiBSBoneLODController,
    controllers::NiLookAtInterpolator,
    controllers::BSDecalPlacementVectorExtraData,
    controllers::BSBound,
    controllers::NiPSysUpdateCtlr,
    controllers::NiPSysEmitterCtlr,
    controllers::NiFloatInterpolator,
    controllers::NiBoolInterpolator,
    controllers::NiPSysModifierActiveCtlr,
    controllers::NiFloatData,
    controllers::NiBoolData,
    controllers::NiBoolTimelineInterpolator,
    controllers::NiBlendFloatInterpolator,
    controllers::NiBlendBoolInterpolator,
    controllers::NiTransformInterpolator,
    controllers::NiTransformData,
    controllers::BSEffectShaderPropertyFloatController,
    controllers::BSProceduralLightningController,
    controllers::BSLightingShaderPropertyFloatController,
    controllers::BSLightingShaderPropertyColorController,
    controllers::BSEffectShaderPropertyColorController,
    controllers::BSNiAlphaPropertyTestRefController,
    controllers::NiBlendPoint3Interpolator,
    controllers::NiPoint3Interpolator,
    controllers::NiPosData,
    controllers::NiPSysModifierFloatCtlr,
    controllers::BSPSysMultiTargetEmitterCtlr,
    controllers::NiVisController,
    controllers::NiFloatExtraData,
    controllers::NiIntegerExtraData,
    controllers::NiStringExtraData,
    controllers::NiStringsExtraData,
    controllers::BSInvMarker,
    controllers::NiFloatExtraDataController,
    controllers::NiTransformController,
    controllers::NiPathInterpolator,
    controllers::NiMultiTargetTransformController,
    controllers::NiTextKeyExtraData,
    controllers::NiControllerSequence,
    controllers::NiControllerManager,
    controllers::NiDefaultAVObjectPalette,
    controllers::BSBehaviorGraphExtraData,
    geometry::NiNode,
    geometry::BsMasterParticleSystem,
    geometry::BsOrderedNode,
    geometry::BsValueNode,
    geometry::NiSwitchNode,
    geometry::BsRangeNode,
    geometry::BsMultiBoundNode,
    geometry::BsMultiBound,
    geometry::BsMultiBoundAabb,
    geometry::BsMultiBoundObb,
    geometry::BsTreeNode,
    geometry::NiBillboardNode,
    geometry::BsTriShape,
    geometry::NiTriShape,
    geometry::NiTriShapeData,
    geometry::BsLodTriShape,
    geometry::BsSegmentedTriShape,
    geometry::NiTriStripsData,
    geometry::NiSkinInstance,
    geometry::BsDismemberSkinInstance,
    geometry::NiSkinData,
    geometry::NiSkinPartition,
    geometry::BsSubIndexTriShape,
    geometry::BsDynamicTriShape,
    particles::NiParticleSystem,
    particles::NiPSysData,
    particles::BSPSysStripUpdateModifier,
    particles::BSStripPSysData,
    particles::NiPSysMeshEmitter,
    particles::NiPSysBoxEmitter,
    particles::NiPSysSphereEmitter,
    particles::NiPSysCylinderEmitter,
    particles::NiPSysAgeDeathModifier,
    particles::BSPSysSubTexModifier,
    particles::NiPSysSpawnModifier,
    particles::BSPSysInheritVelocityModifier,
    particles::BSPSysRecycleBoundModifier,
    particles::BSWindModifier,
    particles::BSPSysLODModifier,
    particles::NiPSysColliderManager,
    particles::NiPSysPlanarCollider,
    particles::NiPSysSphericalCollider,
    particles::NiPSysBoundUpdateModifier,
    particles::NiPSysPositionModifier,
    particles::NiPSysGravityModifier,
    particles::NiPSysRotationModifier,
    particles::NiPSysDragModifier,
    particles::NiPSysBombModifier,
    particles::BSPSysScaleModifier,
    particles::BSPSysSimpleColorModifier,
    shaders::NiAlphaProperty,
    shaders::BSEffectShaderProperty,
    shaders::BSSkyShaderProperty,
    shaders::BSWaterShaderProperty,
    shaders::BSShaderTextureSet,
    shaders::BSLightingShaderProperty,
}
