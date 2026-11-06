//! HDT-SMP preview: the physics XML a NIF names in its «HDT Skinned Mesh
//! Physics Object» extra data, simulated over the clip.
//!
//! The format follows FSMP's `hdtSkyrimSystem` (as ported to BodySlide's
//! `src/physics/hdt`): `<bone-default>` / `<generic-constraint-default>`
//! accumulate, `<bone name>` and `<generic-constraint bodyA bodyB>` take the
//! current default plus their own children. Bones with mass 0 are keyframed
//! (they follow the animation); the rest are simulated.
//!
//! The solver is our own, not Bullet: a position-based step at 60 Hz.
//! Generic 6DOF constraints act in B's frame (`useLinearReferenceFrameA`
//! false): each linear axis is clamped to its limits (lower > upper = free),
//! the relative rotation is clamped per XYZ Euler angle (Bullet's order),
//! springs pull softly toward the equilibrium. Stiff springs keep a distance
//! band, cone-twist constraints become angular limits.
//!
//! Collisions: every `per-triangle-shape` / `per-vertex-shape` names a NIF
//! shape (often an undrawn one: coll_body_*, VirtualGround). Shapes whose
//! bones are simulated push their vertices (spheres of margin × the bone's
//! margin-multiplier) out of the triangles of keyframed shapes, front side
//! only, up to `penetration` deep, when neither lists the other's tag in
//! `no-collide-with-tag`. The push goes to the vertex's dynamic bones by
//! weight (translation and turn). Between two moving shapes (cape against
//! skirt) the push is shared: half to the vertex, half against the
//! triangle's corners; cloth with penetration 0 is two-sided.

use crate::app::StudioApp;
use crate::character::{BoneRef, Character};
use aether_core::mesh::Transform;
use std::collections::HashMap;

/// Havok gravity in game units per s² (1 m = 69.99 units).
const GRAVITY: f32 = 9.81 * 69.99;
const DT: f32 = 1.0 / 60.0;
/// Substeps per 1/60 s step, and constraint passes per substep.
const SUBSTEPS: usize = 4;
const ITERATIONS: usize = 8;
const SUB_DT: f32 = DT / SUBSTEPS as f32;
/// A scrub further than this past the last step replays only its tail.
const MAX_STEPS_PER_FRAME: u32 = 900;

// ---- a tiny XML reader --------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct Element {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Element>,
    pub text: String,
}

impl Element {
    fn attr(&self, k: &str) -> Option<&str> {
        self.attrs.iter().find(|(n, _)| n.eq_ignore_ascii_case(k)).map(|(_, v)| v.as_str())
    }
    fn child(&self, k: &str) -> Option<&Element> {
        self.children.iter().find(|c| c.name.eq_ignore_ascii_case(k))
    }
}

/// Elements, attributes, text; skips `<?…?>`, `<!--…-->` and `<!…>`.
pub fn parse_xml(src: &str) -> Result<Element, String> {
    let mut stack = vec![Element { name: "#root".into(), ..Default::default() }];
    let b = src.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'<' {
            if src[i..].starts_with("<!--") {
                i = src[i..].find("-->").map(|e| i + e + 3).ok_or("unclosed comment")?;
                continue;
            }
            if src[i..].starts_with("<?") || src[i..].starts_with("<!") {
                i = src[i..].find('>').map(|e| i + e + 1).ok_or("unclosed declaration")?;
                continue;
            }
            let end = src[i..].find('>').map(|e| i + e).ok_or("unclosed tag")?;
            let body = &src[i + 1..end];
            i = end + 1;
            if let Some(name) = body.strip_prefix('/') {
                let el = stack.pop().ok_or("stray closing tag")?;
                if !el.name.eq_ignore_ascii_case(name.trim()) {
                    return Err(format!("</{}> closes <{}>", name.trim(), el.name));
                }
                stack.last_mut().ok_or("stray closing tag")?.children.push(el);
                continue;
            }
            let selfclose = body.ends_with('/');
            let body = body.trim_end_matches('/');
            let name_end = body.find(char::is_whitespace).unwrap_or(body.len());
            let mut el = Element { name: body[..name_end].to_string(), ..Default::default() };
            let mut rest = &body[name_end..];
            while let Some(eq) = rest.find('=') {
                let key = rest[..eq].trim().to_string();
                let after = rest[eq + 1..].trim_start();
                let q = after.chars().next().filter(|c| *c == '"' || *c == '\'').ok_or("unquoted attribute")?;
                let close = after[1..].find(q).ok_or("unclosed attribute")?;
                el.attrs.push((key, unescape(&after[1..1 + close])));
                rest = &after[close + 2..];
            }
            if selfclose {
                stack.last_mut().unwrap().children.push(el);
            } else {
                stack.push(el);
            }
        } else {
            let end = src[i..].find('<').map(|e| i + e).unwrap_or(b.len());
            let text = src[i..end].trim();
            if !text.is_empty() {
                stack.last_mut().unwrap().text.push_str(&unescape(text));
            }
            i = end;
        }
    }
    if stack.len() != 1 {
        return Err(format!("<{}> is not closed", stack.last().unwrap().name));
    }
    Ok(stack.pop().unwrap())
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

/// FSMP's number reading: decimal commas are accepted.
fn num(s: &str) -> f32 {
    s.trim().replace(',', ".").parse().unwrap_or(0.0)
}

fn vec3(e: &Element) -> [f32; 3] {
    [num(e.attr("x").unwrap_or("0")), num(e.attr("y").unwrap_or("0")), num(e.attr("z").unwrap_or("0"))]
}

// ---- small linear algebra (row-major 3×3, `v' = M v`) -------------------

type M3 = [f32; 9];
const I3: M3 = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];

fn mm(a: &M3, b: &M3) -> M3 {
    let mut o = [0.0; 9];
    for r in 0..3 {
        for c in 0..3 {
            o[r * 3 + c] = a[r * 3] * b[c] + a[r * 3 + 1] * b[3 + c] + a[r * 3 + 2] * b[6 + c];
        }
    }
    o
}
fn mt(a: &M3) -> M3 {
    [a[0], a[3], a[6], a[1], a[4], a[7], a[2], a[5], a[8]]
}
fn mv(a: &M3, v: [f32; 3]) -> [f32; 3] {
    [a[0] * v[0] + a[1] * v[1] + a[2] * v[2], a[3] * v[0] + a[4] * v[1] + a[5] * v[2], a[6] * v[0] + a[7] * v[1] + a[8] * v[2]]
}
fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn mul(a: [f32; 3], k: f32) -> [f32; 3] {
    [a[0] * k, a[1] * k, a[2] * k]
}
fn len(a: [f32; 3]) -> f32 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}

/// Rotation by `angle` about unit `axis`.
fn axis_angle(axis: [f32; 3], angle: f32) -> M3 {
    let (s, c) = angle.sin_cos();
    let t = 1.0 - c;
    let [x, y, z] = axis;
    [t * x * x + c, t * x * y - s * z, t * x * z + s * y, t * x * y + s * z, t * y * y + c, t * y * z - s * x, t * x * z - s * y, t * y * z + s * x, t * z * z + c]
}

/// The rotation vector (axis × angle) of `m`.
fn rot_vec(m: &M3) -> [f32; 3] {
    let cos = ((m[0] + m[4] + m[8] - 1.0) * 0.5).clamp(-1.0, 1.0);
    let angle = cos.acos();
    if angle < 1e-6 {
        return [0.0; 3];
    }
    let v = [m[7] - m[5], m[2] - m[6], m[3] - m[1]];
    let s = len(v);
    if s < 1e-6 {
        // ~180°: the axis from the diagonal.
        let x = ((m[0] + 1.0) * 0.5).max(0.0).sqrt();
        let y = ((m[4] + 1.0) * 0.5).max(0.0).sqrt();
        let z = ((m[8] + 1.0) * 0.5).max(0.0).sqrt();
        return mul([x, y, z], angle);
    }
    mul(v, angle / s)
}

fn from_rot_vec(v: [f32; 3]) -> M3 {
    let a = len(v);
    if a < 1e-7 { I3 } else { axis_angle(mul(v, 1.0 / a), a) }
}

/// Gram–Schmidt on the columns.
fn orthonormalize(m: &M3) -> M3 {
    let c0 = [m[0], m[3], m[6]];
    let c1 = [m[1], m[4], m[7]];
    let n = |v: [f32; 3]| mul(v, 1.0 / len(v).max(1e-9));
    let x = n(c0);
    let d = x[0] * c1[0] + x[1] * c1[1] + x[2] * c1[2];
    let y = n(sub(c1, mul(x, d)));
    let z = [x[1] * y[2] - x[2] * y[1], x[2] * y[0] - x[0] * y[2], x[0] * y[1] - x[1] * y[0]];
    [x[0], y[0], z[0], x[1], y[1], z[1], x[2], y[2], z[2]]
}

/// A rigid frame without scale.
#[derive(Debug, Clone, Copy)]
struct Frame {
    r: M3,
    p: [f32; 3],
}

impl Frame {
    const ID: Frame = Frame { r: I3, p: [0.0; 3] };
    fn of(t: &Transform) -> Frame {
        Frame { r: orthonormalize(&t.rot), p: t.t }
    }
    fn compose(&self, c: &Frame) -> Frame {
        Frame { r: mm(&self.r, &c.r), p: add(self.p, mv(&self.r, c.p)) }
    }
    fn inverse(&self) -> Frame {
        let rt = mt(&self.r);
        Frame { r: rt, p: mul(mv(&rt, self.p), -1.0) }
    }
}

// ---- the configuration --------------------------------------------------

#[derive(Debug, Clone)]
struct ShapeDef {
    name: String,
    margin: f32,
    penetration: f32,
    tags: Vec<String>,
    no_collide: Vec<String>,
}

impl ShapeDef {
    fn collides_with(&self, o: &ShapeDef) -> bool {
        let blocked = |a: &ShapeDef, b: &ShapeDef| a.no_collide.iter().any(|n| b.tags.iter().any(|t| t.eq_ignore_ascii_case(n)));
        !blocked(self, o) && !blocked(o, self)
    }
}

#[derive(Debug, Clone)]
struct BodyDef {
    name: String,
    margin_mult: f32,
    mass: f32,
    lin_damp: f32,
    ang_damp: f32,
    gravity: f32,
}

#[derive(Debug, Clone, Copy)]
enum FrameDef {
    InA(Frame),
    InB(Frame),
    Lerp(f32, f32),
}

#[derive(Debug, Clone)]
struct SixDof {
    frame: FrameDef,
    lin: [[f32; 3]; 2],
    ang: [[f32; 3]; 2],
    lin_k: [f32; 3],
    ang_k: [f32; 3],
    lin_eq: [f32; 3],
    ang_eq: [f32; 3],
}

impl Default for SixDof {
    fn default() -> Self {
        SixDof { frame: FrameDef::InA(Frame::ID), lin: [[0.0; 3]; 2], ang: [[0.0; 3]; 2], lin_k: [0.0; 3], ang_k: [0.0; 3], lin_eq: [0.0; 3], ang_eq: [0.0; 3] }
    }
}

#[derive(Debug, Clone)]
enum Kind {
    SixDof(SixDof),
    /// Distance band as factors of the rest distance, and a stiffness.
    Spring { min: f32, max: f32, stiffness: f32 },
}

#[derive(Debug, Clone)]
struct ConstraintDef {
    a: String,
    b: String,
    kind: Kind,
}

#[derive(Debug, Clone, Default)]
pub struct Config {
    bodies: Vec<BodyDef>,
    constraints: Vec<ConstraintDef>,
    shapes: Vec<ShapeDef>,
    pub notes: Vec<String>,
}

fn frame_of(e: &Element) -> Frame {
    let mut f = Frame::ID;
    if let Some(b) = e.child("basis") {
        // Quaternion x y z w.
        let (x, y, z, w) = (num(b.attr("x").unwrap_or("0")), num(b.attr("y").unwrap_or("0")), num(b.attr("z").unwrap_or("0")), num(b.attr("w").unwrap_or("1")));
        f.r = orthonormalize(&Transform::rows_from_quat([w, x, y, z]));
    }
    if let Some(o) = e.child("origin") {
        f.p = vec3(o);
    }
    f
}

fn read_body(e: &Element, d: &mut BodyDef) {
    for c in &e.children {
        match c.name.as_str() {
            "mass" => d.mass = num(&c.text),
            "linearDamping" => d.lin_damp = num(&c.text),
            "angularDamping" => d.ang_damp = num(&c.text),
            "gravity-factor" => d.gravity = num(&c.text),
            "margin-multiplier" => d.margin_mult = num(&c.text),
            _ => {}
        }
    }
}

fn read_six(e: &Element, d: &mut SixDof) {
    for c in &e.children {
        match c.name.as_str() {
            "frameInA" => d.frame = FrameDef::InA(frame_of(c)),
            "frameInB" => d.frame = FrameDef::InB(frame_of(c)),
            "frameInLerp" => {
                let t = c.child("translationLerp").map_or(0.5, |x| num(&x.text));
                let r = c.child("rotationLerp").map_or(0.5, |x| num(&x.text));
                d.frame = FrameDef::Lerp(t, r);
            }
            "linearLowerLimit" => d.lin[0] = vec3(c),
            "linearUpperLimit" => d.lin[1] = vec3(c),
            "angularLowerLimit" => d.ang[0] = vec3(c),
            "angularUpperLimit" => d.ang[1] = vec3(c),
            "linearStiffness" => d.lin_k = vec3(c),
            "angularStiffness" => d.ang_k = vec3(c),
            "linearEquilibrium" => d.lin_eq = vec3(c),
            "angularEquilibrium" => d.ang_eq = vec3(c),
            // Spans → symmetric angular limits: X twist, Y/Z swing.
            "swingSpan1" => {
                let v = num(&c.text);
                d.ang[0][2] = -v;
                d.ang[1][2] = v;
            }
            "swingSpan2" => {
                let v = num(&c.text);
                d.ang[0][1] = -v;
                d.ang[1][1] = v;
            }
            "twistSpan" => {
                let v = num(&c.text);
                d.ang[0][0] = -v;
                d.ang[1][0] = v;
            }
            _ => {}
        }
    }
}

pub fn read_config(root: &Element) -> Config {
    let mut cfg = Config::default();
    let mut body_def = BodyDef { name: String::new(), margin_mult: 1.0, mass: 0.0, lin_damp: 0.0, ang_damp: 0.0, gravity: 1.0 };
    let mut six_def = SixDof::default();
    let mut cone_def = SixDof::default();
    let mut spring_def = (1.0f32, 1.0f32, 0.0f32);
    let mut shapes = 0;
    fn walk(
        e: &Element,
        cfg: &mut Config,
        body_def: &mut BodyDef,
        six_def: &mut SixDof,
        cone_def: &mut SixDof,
        spring_def: &mut (f32, f32, f32),
        shapes: &mut usize,
    ) {
        for c in &e.children {
            match c.name.as_str() {
                "system" | "constraint-group" => walk(c, cfg, body_def, six_def, cone_def, spring_def, shapes),
                "bone-default" => read_body(c, body_def),
                "bone" => {
                    let mut d = body_def.clone();
                    d.name = c.attr("name").unwrap_or_default().to_string();
                    read_body(c, &mut d);
                    if !d.name.is_empty() {
                        cfg.bodies.retain(|b| !b.name.eq_ignore_ascii_case(&d.name));
                        cfg.bodies.push(d);
                    }
                }
                "generic-constraint-default" => read_six(c, six_def),
                "cone-twist-constraint-default" | "conetwist-constraint-default" => read_six(c, cone_def),
                "generic-constraint" | "cone-twist-constraint" | "conetwist-constraint" => {
                    let mut d = if c.name == "generic-constraint" { six_def.clone() } else { cone_def.clone() };
                    read_six(c, &mut d);
                    if let (Some(a), Some(b)) = (c.attr("bodyA"), c.attr("bodyB")) {
                        cfg.constraints.push(ConstraintDef { a: a.trim().into(), b: b.trim().into(), kind: Kind::SixDof(d) });
                    }
                }
                "stiffspring-constraint-default" => read_spring(c, spring_def),
                "stiffspring-constraint" => {
                    let mut d = *spring_def;
                    read_spring(c, &mut d);
                    if let (Some(a), Some(b)) = (c.attr("bodyA"), c.attr("bodyB")) {
                        cfg.constraints.push(ConstraintDef { a: a.trim().into(), b: b.trim().into(), kind: Kind::Spring { min: d.0, max: d.1, stiffness: d.2 } });
                    }
                }
                "per-vertex-shape" | "per-triangle-shape" => {
                    *shapes += 1;
                    let mut d = ShapeDef { name: c.attr("name").unwrap_or_default().to_string(), margin: 1.0, penetration: 1.0, tags: Vec::new(), no_collide: Vec::new() };
                    for x in &c.children {
                        match x.name.as_str() {
                            "margin" => d.margin = num(&x.text),
                            "penetration" => d.penetration = num(&x.text),
                            "tag" => d.tags.push(x.text.trim().to_string()),
                            "no-collide-with-tag" => d.no_collide.push(x.text.trim().to_string()),
                            _ => {}
                        }
                    }
                    if !d.name.is_empty() {
                        cfg.shapes.push(d);
                    }
                }
                _ => {}
            }
        }
    }
    fn read_spring(e: &Element, d: &mut (f32, f32, f32)) {
        for c in &e.children {
            match c.name.as_str() {
                "minDistanceFactor" => d.0 = num(&c.text),
                "maxDistanceFactor" => d.1 = num(&c.text),
                "stiffness" => d.2 = num(&c.text),
                _ => {}
            }
        }
    }
    walk(root, &mut cfg, &mut body_def, &mut six_def, &mut cone_def, &mut spring_def, &mut shapes);
    let _ = shapes;
    cfg
}

// ---- the running system -------------------------------------------------

#[derive(Debug, Clone)]
struct Body {
    name: String,
    /// Where the bone sits on the character (rest).
    bone: BoneRef,
    inv_mass: f32,
    lin_damp: f32,
    ang_damp: f32,
    gravity: f32,
    f: Frame,
    prev: Frame,
    v: [f32; 3],
    w: [f32; 3],
    /// The bone's own scale, kept for skinning.
    scale: f32,
    /// Links to the nearest keyframed bone: deeper bodies take more of each
    /// correction, so long chains converge from their anchor outward.
    depth: u32,
    /// Scalar inverse inertia: a rod as long as the body's longest link.
    inv_inertia: f32,
    margin_mult: f32,
}

/// A collision shape on the character: a skinned NIF shape.
#[derive(Clone)]
struct Shape {
    def: ShapeDef,
    mesh: crate::character::SkinMesh,
    /// Per bone slot: the body it is, if any.
    slot_body: Vec<Option<usize>>,
    /// Any slot is a simulated body: its vertices are pushed.
    dynamic: bool,
    /// Vertices the dynamic side tests (a subsample of big meshes).
    sample: Vec<usize>,
}

/// Fixed triangles, in a grid (big ones, like the ground, apart).
struct Tris {
    v: Vec<[f32; 3]>,
    tris: Vec<[u32; 3]>,
    /// (cell key, triangle), sorted by key.
    grid: Vec<(u64, u32)>,
    large: Vec<u32>,
    /// Bounds, padded by the largest contact radius.
    lo: [f32; 3],
    hi: [f32; 3],
}

fn cell(p: [f32; 3]) -> (i32, i32, i32) {
    ((p[0] / CELL).floor() as i32, (p[1] / CELL).floor() as i32, (p[2] / CELL).floor() as i32)
}

fn cell_key(c: (i32, i32, i32)) -> u64 {
    let k = |v: i32| (v as i64 + (1 << 20)) as u64 & 0x1F_FFFF;
    (k(c.0) << 42) | (k(c.1) << 21) | k(c.2)
}

impl Tris {
    fn near(&self, p: [f32; 3]) -> &[(u64, u32)] {
        let key = cell_key(cell(p));
        let lo = self.grid.partition_point(|e| e.0 < key);
        let hi = self.grid.partition_point(|e| e.0 <= key);
        &self.grid[lo..hi]
    }
}

/// Dynamic vertices tested per shape per substep at most.
const MAX_SAMPLES: usize = 250;
const CELL: f32 = 12.0;

#[derive(Debug, Clone)]
struct Constraint {
    a: usize,
    b: usize,
    kind: Kind,
    /// Constraint frames in A's and B's space, from the start pose.
    fa: Frame,
    fb: Frame,
    rest: f32,
}

#[derive(Clone, Default)]
pub struct System {
    bodies: Vec<Body>,
    constraints: Vec<Constraint>,
    shapes: Vec<Shape>,
    /// Contacts in the last substep, for the log.
    pub contacts: usize,
    substep_index: u32,
    pub notes: Vec<String>,
    /// The clip time the state is at.
    pub time: f32,
    /// Character + clip this was built for.
    pub key: String,
}

fn find_bone(c: &Character, name: &str) -> Option<BoneRef> {
    for m in &c.meshes {
        if let Some(i) = m.bone_names.iter().position(|n| n.eq_ignore_ascii_case(name)) {
            return m.bones.get(i).map(|b| b.0);
        }
    }
    c.resolve(name).or_else(|| c.piece_nodes.get(&name.to_ascii_lowercase()).copied())
}

/// Everything the solver needs from the animation at one time.
pub struct Pose<'a> {
    pub model: &'a [Transform],
    pub root: Transform,
}

impl System {
    pub fn build(c: &Character, cfgs: &[(String, Config)], pose: &Pose) -> System {
        let mut s = System::default();
        let mut index: HashMap<String, usize> = HashMap::new();
        for (file, cfg) in cfgs {
            s.notes.extend(cfg.notes.iter().map(|n| format!("{file}: {n}")));
            for d in &cfg.bodies {
                let key = d.name.to_ascii_lowercase();
                if index.contains_key(&key) {
                    continue;
                }
                let Some(bone) = find_bone(c, &d.name) else {
                    s.notes.push(sds_i18n::tf!("{file}: кость «{}» не найдена", "{file}: bone «{}» not found", d.name));
                    continue;
                };
                let w = Character::world(pose.model, &pose.root, &bone);
                let f = Frame::of(&w);
                index.insert(key, s.bodies.len());
                s.bodies.push(Body {
                    name: d.name.clone(),
                    bone,
                    inv_mass: if d.mass > 0.0 { 1.0 / d.mass } else { 0.0 },
                    lin_damp: d.lin_damp.clamp(0.0, 1.0),
                    ang_damp: d.ang_damp.clamp(0.0, 1.0),
                    gravity: d.gravity,
                    f,
                    prev: f,
                    v: [0.0; 3],
                    w: [0.0; 3],
                    scale: w.s,
                    depth: 0,
                    inv_inertia: 0.0,
                    margin_mult: d.margin_mult,
                });
            }
            for d in &cfg.constraints {
                let (Some(&a), Some(&b)) = (index.get(&d.a.to_ascii_lowercase()), index.get(&d.b.to_ascii_lowercase())) else {
                    s.notes.push(sds_i18n::tf!("{file}: связь {} — {}: нет тела", "{file}: constraint {} — {}: missing body", d.a, d.b));
                    continue;
                };
                let (ba, bb) = (s.bodies[a].f, s.bodies[b].f);
                let (fa, fb) = match &d.kind {
                    Kind::SixDof(six) => match six.frame {
                        FrameDef::InA(f) => (f, bb.inverse().compose(&ba.compose(&f))),
                        FrameDef::InB(f) => (ba.inverse().compose(&bb.compose(&f)), f),
                        FrameDef::Lerp(t, r) => {
                            let p = add(ba.p, mul(sub(bb.p, ba.p), t));
                            let rel = rot_vec(&mm(&bb.r, &mt(&ba.r)));
                            let w = Frame { r: mm(&from_rot_vec(mul(rel, r)), &ba.r), p };
                            (ba.inverse().compose(&w), bb.inverse().compose(&w))
                        }
                    },
                    Kind::Spring { .. } => (Frame::ID, Frame::ID),
                };
                s.constraints.push(Constraint { a, b, kind: d.kind.clone(), fa, fb, rest: len(sub(ba.p, bb.p)) });
            }
        }
        // Depth from the keyframed bones (BFS over constraints), then solve
        // the shallow constraints first.
        let mut depth: Vec<u32> = s.bodies.iter().map(|b| if b.inv_mass == 0.0 { 0 } else { u32::MAX }).collect();
        let mut changed = true;
        while changed {
            changed = false;
            for c in &s.constraints {
                for (x, y) in [(c.a, c.b), (c.b, c.a)] {
                    if depth[y] != u32::MAX && depth[y] + 1 < depth[x] {
                        depth[x] = depth[y] + 1;
                        changed = true;
                    }
                }
            }
        }
        for (b, d) in s.bodies.iter_mut().zip(&depth) {
            b.depth = if *d == u32::MAX { 1 } else { *d };
        }
        s.set_inertia();
        let key = |c: &Constraint| depth[c.a].min(depth[c.b]);
        s.constraints.sort_by_key(key);
        // Collision shapes: the character's skinned shapes by name.
        for (file, cfg) in cfgs {
            for d in &cfg.shapes {
                let mesh = c.meshes.iter().chain(&c.coll_meshes).find(|m| m.shape.eq_ignore_ascii_case(&d.name) && !m.raw.is_empty());
                let Some(mesh) = mesh else {
                    s.notes.push(sds_i18n::tf!("{file}: форма столкновений «{}» не найдена среди шейпов", "{file}: collision shape «{}» not found among the shapes", d.name));
                    continue;
                };
                let slot_body: Vec<Option<usize>> = mesh.bone_names.iter().map(|n| index.get(&n.to_ascii_lowercase()).copied()).collect();
                let dynamic = slot_body.iter().flatten().any(|&i| s.bodies[i].inv_mass > 0.0);
                let n = mesh.raw.len();
                let stride = n.div_ceil(MAX_SAMPLES).max(1);
                let sample = if dynamic { (0..n).step_by(stride).collect() } else { Vec::new() };
                s.shapes.push(Shape { def: d.clone(), mesh: mesh.clone(), slot_body, dynamic, sample });
            }
        }
        let (dynamic, kinematic) = (s.shapes.iter().filter(|x| x.dynamic).count(), s.shapes.iter().filter(|x| !x.dynamic).count());
        if !s.shapes.is_empty() {
            s.notes.push(sds_i18n::tf!("форм столкновений: {dynamic} подвижных, {kinematic} неподвижных", "collision shapes: {dynamic} moving, {kinematic} fixed"));
        }
        s
    }

    /// World positions of a shape's vertices (`only`: these indices, else all).
    fn skin_shape(&self, sh: &Shape, pose: &Pose, only: Option<&[usize]>) -> Vec<(usize, [f32; 3])> {
        let mats: Vec<Transform> = sh
            .mesh
            .bones
            .iter()
            .enumerate()
            .map(|(k, (b, s2b))| match sh.slot_body.get(k).copied().flatten() {
                Some(i) => Transform::from_av(self.bodies[i].f.p, self.bodies[i].f.r, self.bodies[i].scale).compose(s2b),
                None => Character::world(pose.model, &pose.root, b).compose(s2b),
            })
            .collect();
        let one = |i: usize| {
            let (pos, _, bi, wts) = &sh.mesh.raw[i];
            let mut p = [0.0f32; 3];
            let mut total = 0.0;
            for k in 0..4 {
                if wts[k] <= 0.0 {
                    continue;
                }
                let Some(m) = mats.get(bi[k] as usize) else { continue };
                p = add(p, mul(m.transform_point(*pos), wts[k]));
                total += wts[k];
            }
            (i, if total > 0.0 { mul(p, 1.0 / total) } else { pose.root.transform_point(*pos) })
        };
        match only {
            Some(list) => list.iter().map(|&i| one(i)).collect(),
            None => (0..sh.mesh.raw.len()).map(one).collect(),
        }
    }

    /// A shape's triangles where its bones are now, in a grid.
    fn build_tris(&self, sh: &Shape, pose: &Pose) -> Tris {
        let v: Vec<[f32; 3]> = self.skin_shape(sh, pose, None).into_iter().map(|x| x.1).collect();
        let tris: Vec<[u32; 3]> = sh.mesh.indices.chunks_exact(3).map(|t| [t[0] as u32, t[1] as u32, t[2] as u32]).collect();
        let mut grid: Vec<(u64, u32)> = Vec::with_capacity(tris.len() * 4);
        let mut large = Vec::new();
        let pad = if sh.dynamic { 1.0 } else { 4.0 } + sh.def.margin;
        for (ti, t) in tris.iter().enumerate() {
            let (a, b, c) = (v[t[0] as usize], v[t[1] as usize], v[t[2] as usize]);
            let lo = cell([a[0].min(b[0]).min(c[0]) - pad, a[1].min(b[1]).min(c[1]) - pad, a[2].min(b[2]).min(c[2]) - pad]);
            let hi = cell([a[0].max(b[0]).max(c[0]) + pad, a[1].max(b[1]).max(c[1]) + pad, a[2].max(b[2]).max(c[2]) + pad]);
            if (hi.0 - lo.0 + 1) * (hi.1 - lo.1 + 1) * (hi.2 - lo.2 + 1) > 64 {
                large.push(ti as u32);
                continue;
            }
            for x in lo.0..=hi.0 {
                for y in lo.1..=hi.1 {
                    for z in lo.2..=hi.2 {
                        grid.push((cell_key((x, y, z)), ti as u32));
                    }
                }
            }
        }
        grid.sort_unstable();
        let pad = 8.0;
        let mut lo = [f32::MAX; 3];
        let mut hi = [f32::MIN; 3];
        for p in &v {
            for k in 0..3 {
                lo[k] = lo[k].min(p[k] - pad);
                hi[k] = hi[k].max(p[k] + pad);
            }
        }
        if std::env::var_os("SDS_SMP_DEBUG").is_some() && self.time < DT * 1.5 {
            eprintln!("SMPSHAPE {} dynamic={} verts={} tris={} large={} cells={}", sh.def.name, sh.dynamic, v.len(), tris.len(), large.len(), grid.len());
        }
        Tris { v, tris, grid, large, lo, hi }
    }

    /// Which shapes each moving shape may touch (tags).
    fn partners(&self) -> Vec<Vec<usize>> {
        let n = self.shapes.len();
        (0..n)
            .map(|di| {
                let d = &self.shapes[di];
                if !d.dynamic {
                    return Vec::new();
                }
                (0..n).filter(|&ki| ki != di && d.def.collides_with(&self.shapes[ki].def)).collect()
            })
            .collect()
    }

    /// Obstacle triangles, once per step: the fixed shapes at `pose`, and
    /// the moving shapes some other moving shape can touch, where their
    /// bones are at the start of the step (one step behind at most).
    fn fixed_tris(&self, pose: &Pose) -> Vec<Option<Tris>> {
        if self.shapes.iter().all(|s| !s.dynamic) {
            return Vec::new();
        }
        let partners = self.partners();
        self.shapes
            .iter()
            .enumerate()
            .map(|(ki, sh)| (!sh.dynamic || partners.iter().any(|p| p.contains(&ki))).then(|| self.build_tris(sh, pose)))
            .collect()
    }

    /// Push the moving shapes' vertices out of other shapes' triangles: the
    /// fixed ones (the vertex takes the whole push) and other moving ones
    /// (cape against skirt: half to the vertex, half against the triangle,
    /// spread over its corners). Cloth with penetration 0 is two-sided.
    fn collide(&mut self, pose: &Pose, fixed: &[Option<Tris>]) {
        self.contacts = 0;
        if fixed.is_empty() {
            return;
        }
        let n = self.shapes.len();
        let partners = self.partners();
        let mut pushes: Vec<(usize, [f32; 3], [f32; 3])> = Vec::new();
        let dot = |x: [f32; 3], y: [f32; 3]| x[0] * y[0] + x[1] * y[1] + x[2] * y[2];
        for di in 0..n {
            if partners[di].is_empty() {
                continue;
            }
            let d = &self.shapes[di];
            let verts = self.skin_shape(d, pose, Some(&d.sample));
            for (vi, p) in verts {
                let (_, _, bi, wts) = &d.mesh.raw[vi];
                // The vertex's own radius: its heaviest bone's multiplier.
                let heavy = (0..4).max_by(|&x, &y| wts[x].total_cmp(&wts[y])).unwrap_or(0);
                let mult = d.slot_body.get(bi[heavy] as usize).copied().flatten().map_or(1.0, |i| self.bodies[i].margin_mult);
                // (push, moving obstacle: shape, triangle, barycentrics)
                let mut best: Option<([f32; 3], Option<(usize, [u32; 3], [f32; 3])>)> = None;
                for &ki in &partners[di] {
                    let k = &self.shapes[ki];
                    let Some(tr) = fixed.get(ki).and_then(|f| f.as_ref()) else { continue };
                    if (0..3).any(|a| p[a] < tr.lo[a] || p[a] > tr.hi[a]) && tr.large.is_empty() {
                        continue;
                    }
                    let r = d.def.margin * mult + k.def.margin;
                    let two_sided = k.dynamic && k.def.penetration <= 0.0;
                    let pen = if k.def.penetration > 0.0 { k.def.penetration } else { 1.0 };
                    for ti in tr.near(p).iter().map(|e| e.1).chain(tr.large.iter().copied()) {
                        let t = tr.tris[ti as usize];
                        let (a, b, c) = (tr.v[t[0] as usize], tr.v[t[1] as usize], tr.v[t[2] as usize]);
                        let (e1, e2) = (sub(b, a), sub(c, a));
                        let nrm = [e1[1] * e2[2] - e1[2] * e2[1], e1[2] * e2[0] - e1[0] * e2[2], e1[0] * e2[1] - e1[1] * e2[0]];
                        let nl = len(nrm);
                        if nl < 1e-8 {
                            continue;
                        }
                        let mut nn = mul(nrm, 1.0 / nl);
                        let mut dist = dot(sub(p, a), nn);
                        if two_sided && dist < 0.0 {
                            nn = mul(nn, -1.0);
                            dist = -dist;
                        }
                        if dist >= r || dist <= -pen {
                            continue;
                        }
                        // Inside the triangle (barycentric of the projection)?
                        let q = sub(sub(p, mul(nn, dist)), a);
                        let (d00, d01, d11, d20, d21) = (dot(e1, e1), dot(e1, e2), dot(e2, e2), dot(q, e1), dot(q, e2));
                        let den = d00 * d11 - d01 * d01;
                        if den.abs() < 1e-12 {
                            continue;
                        }
                        let v = (d11 * d20 - d01 * d21) / den;
                        let w = (d00 * d21 - d01 * d20) / den;
                        if v < 0.0 || w < 0.0 || v + w > 1.0 {
                            continue;
                        }
                        let push = mul(nn, r - dist);
                        if best.as_ref().is_none_or(|b| len(b.0) < len(push)) {
                            best = Some((push, k.dynamic.then_some((ki, t, [1.0 - v - w, v, w]))));
                        }
                    }
                }
                let Some((push, other)) = best else { continue };
                let share = if other.is_some() { 0.5 } else { 1.0 };
                for k in 0..4 {
                    if wts[k] <= 0.0 {
                        continue;
                    }
                    if let Some(i) = d.slot_body.get(bi[k] as usize).copied().flatten().filter(|&i| self.bodies[i].inv_mass > 0.0) {
                        pushes.push((i, p, mul(push, wts[k] * share)));
                    }
                }
                if let Some((ki, t, bary)) = other {
                    let (k, tr) = (&self.shapes[ki], fixed[ki].as_ref().unwrap());
                    for (corner, &ri) in t.iter().enumerate() {
                        let (_, _, cbi, cw) = &k.mesh.raw[ri as usize];
                        for s in 0..4 {
                            if cw[s] <= 0.0 {
                                continue;
                            }
                            if let Some(i) = k.slot_body.get(cbi[s] as usize).copied().flatten().filter(|&i| self.bodies[i].inv_mass > 0.0) {
                                pushes.push((i, tr.v[ri as usize], mul(push, -0.5 * bary[corner] * cw[s])));
                            }
                        }
                    }
                }
            }
        }
        self.contacts = pushes.len();
        for (i, point, delta) in pushes {
            self.push(i, point, delta);
        }
    }

    /// Move body `i`'s material point `point` by `delta` (one body, XPBD).
    fn push(&mut self, i: usize, point: [f32; 3], delta: [f32; 3]) {
        let c = len(delta);
        if c < 1e-6 {
            return;
        }
        let n = mul(delta, 1.0 / c);
        let (wm, wi) = (self.wm(i), self.wi(i));
        let b = &mut self.bodies[i];
        let r = sub(point, b.f.p);
        let rn = [r[1] * n[2] - r[2] * n[1], r[2] * n[0] - r[0] * n[2], r[0] * n[1] - r[1] * n[0]];
        let w = wm + wi * (rn[0] * rn[0] + rn[1] * rn[1] + rn[2] * rn[2]);
        if w <= 0.0 {
            return;
        }
        let l = c / w;
        b.f.p = add(b.f.p, mul(n, l * wm));
        b.f.r = orthonormalize(&mm(&from_rot_vec(mul(rn, l * wi)), &b.f.r));
    }

    fn set_inertia(&mut self) {
        let mut reach = vec![1.0f32; self.bodies.len()];
        for c in &self.constraints {
            reach[c.a] = reach[c.a].max(c.rest);
            reach[c.b] = reach[c.b].max(c.rest);
        }
        for (b, l) in self.bodies.iter_mut().zip(reach) {
            b.inv_inertia = b.inv_mass * 3.0 / (l * l);
        }
    }

    /// Inverse mass and inertia as the solver weighs them.
    fn wm(&self, i: usize) -> f32 {
        let b = &self.bodies[i];
        b.inv_mass * (1.0 + b.depth as f32)
    }
    fn wi(&self, i: usize) -> f32 {
        let b = &self.bodies[i];
        b.inv_inertia * (1.0 + b.depth as f32)
    }

    /// Move A's anchor `pa` by `delta` relative to B's anchor `pb`: both
    /// bodies translate and turn by their generalized inverse masses (XPBD).
    fn correct(&mut self, a: usize, b: usize, pa: [f32; 3], pb: [f32; 3], delta: [f32; 3]) {
        let c = len(delta);
        if c < 1e-6 {
            return;
        }
        let n = mul(delta, 1.0 / c);
        let cross = |r: [f32; 3]| [r[1] * n[2] - r[2] * n[1], r[2] * n[0] - r[0] * n[2], r[0] * n[1] - r[1] * n[0]];
        let (ca, cb) = (cross(sub(pa, self.bodies[a].f.p)), cross(sub(pb, self.bodies[b].f.p)));
        let dot = |v: [f32; 3]| v[0] * v[0] + v[1] * v[1] + v[2] * v[2];
        let (ma, mb, ia, ib) = (self.wm(a), self.wm(b), self.wi(a), self.wi(b));
        let w = ma + ia * dot(ca) + mb + ib * dot(cb);
        if w <= 0.0 {
            return;
        }
        let l = c / w;
        let ba = &mut self.bodies[a];
        ba.f.p = add(ba.f.p, mul(n, l * ma));
        ba.f.r = orthonormalize(&mm(&from_rot_vec(mul(ca, l * ia)), &ba.f.r));
        let bb = &mut self.bodies[b];
        bb.f.p = sub(bb.f.p, mul(n, l * mb));
        bb.f.r = orthonormalize(&mm(&from_rot_vec(mul(cb, -l * ib)), &bb.f.r));
    }

    pub fn dynamic_count(&self) -> usize {
        self.bodies.iter().filter(|b| b.inv_mass > 0.0).count()
    }

    /// One 1/60 s step with the keyframed bones moving from `from` to `pose`.
    pub fn step(&mut self, from: &Pose, pose: &Pose) {
        let n = SUBSTEPS as f32;
        let fixed = self.fixed_tris(pose);
        for k in 1..=SUBSTEPS {
            let f = k as f32 / n;
            for b in self.bodies.iter_mut().filter(|b| b.inv_mass == 0.0) {
                let (a, z) = (Frame::of(&Character::world(from.model, &from.root, &b.bone)), Frame::of(&Character::world(pose.model, &pose.root, &b.bone)));
                let rel = rot_vec(&mm(&z.r, &mt(&a.r)));
                b.f = Frame { r: orthonormalize(&mm(&from_rot_vec(mul(rel, f)), &a.r)), p: add(a.p, mul(sub(z.p, a.p), f)) };
            }
            self.substep(DT / n, pose, &fixed);
        }
        self.time += DT;
    }

    fn substep(&mut self, dt: f32, pose: &Pose, fixed: &[Option<Tris>]) {
        for b in &mut self.bodies {
            if b.inv_mass == 0.0 {
                continue;
            }
            b.v[2] -= GRAVITY * b.gravity * dt;
            // Bullet's damping: v *= (1 - d)^dt.
            b.v = mul(b.v, (1.0 - b.lin_damp).powf(dt));
            b.w = mul(b.w, (1.0 - b.ang_damp).powf(dt));
            b.prev = b.f;
            b.f.p = add(b.f.p, mul(b.v, dt));
            b.f.r = orthonormalize(&mm(&from_rot_vec(mul(b.w, dt)), &b.f.r));
        }
        for _ in 0..ITERATIONS {
            for k in 0..self.constraints.len() {
                self.solve(k);
            }
        }
        // Contacts last, so constraints do not pull cloth back inside.
        // Every other substep: enough for contact, half the cost.
        if self.substep_index % 2 == 1 {
            self.collide(pose, fixed);
        }
        self.substep_index += 1;
        for b in self.bodies.iter_mut().filter(|b| b.inv_mass > 0.0) {
            b.v = mul(sub(b.f.p, b.prev.p), 1.0 / dt);
            b.w = mul(rot_vec(&mm(&b.f.r, &mt(&b.prev.r))), 1.0 / dt);
        }
    }

    fn solve(&mut self, k: usize) {
        let c = self.constraints[k].clone();
        let wsum = self.wm(c.a) + self.wm(c.b);
        if wsum <= 0.0 {
            return;
        }
        match &c.kind {
            Kind::Spring { min, max, stiffness } => {
                let d = sub(self.bodies[c.a].f.p, self.bodies[c.b].f.p);
                let l = len(d).max(1e-6);
                let (lo, hi) = (c.rest * min.min(*max), c.rest * max.max(*min));
                let mut target = l.clamp(lo, hi);
                if *stiffness > 0.0 {
                    target += (c.rest - target) * (stiffness * SUB_DT * SUB_DT * wsum).min(1.0);
                }
                let (pa, pb) = (self.bodies[c.a].f.p, self.bodies[c.b].f.p);
                self.correct(c.a, c.b, pa, pb, mul(d, (target - l) / l));
            }
            Kind::SixDof(six) => {
                let soft = |kv: f32| (kv * SUB_DT * SUB_DT * wsum).clamp(0.0, 1.0);
                // Linear, in B's constraint frame.
                let fa = self.bodies[c.a].f.compose(&c.fa);
                let fb = self.bodies[c.b].f.compose(&c.fb);
                let d = mv(&mt(&fb.r), sub(fa.p, fb.p));
                let mut t = d;
                for i in 0..3 {
                    let (lo, hi) = (six.lin[0][i], six.lin[1][i]);
                    if lo <= hi {
                        t[i] = t[i].clamp(lo, hi);
                    }
                    t[i] += (six.lin_eq[i] - t[i]) * soft(six.lin_k[i]);
                }
                self.correct(c.a, c.b, fa.p, fb.p, mv(&fb.r, sub(t, d)));
                // Angular: the relative rotation in Euler XYZ, clamped.
                let fa = self.bodies[c.a].f.compose(&c.fa);
                let fb = self.bodies[c.b].f.compose(&c.fb);
                let rel = mm(&mt(&fb.r), &fa.r);
                let e = Transform::euler_from_rows(&rel);
                let mut te = e;
                for i in 0..3 {
                    let (lo, hi) = (six.ang[0][i], six.ang[1][i]);
                    if lo <= hi {
                        te[i] = te[i].clamp(lo, hi);
                    }
                    te[i] += (six.ang_eq[i] - te[i]) * soft(six.ang_k[i]);
                }
                if (0..3).all(|i| (te[i] - e[i]).abs() < 1e-6) {
                    return;
                }
                let target = mm(&fb.r, &Transform::rows_from_euler(te[0], te[1], te[2]));
                // World rotation taking A's frame to the target, split by mass.
                let fix = rot_vec(&mm(&target, &mt(&fa.r)));
                let (ia, ib) = (self.wi(c.a), self.wi(c.b));
                if ia + ib <= 0.0 {
                    return;
                }
                let ra = from_rot_vec(mul(fix, ia / (ia + ib)));
                let rb = from_rot_vec(mul(fix, -ib / (ia + ib)));
                self.bodies[c.a].f.r = orthonormalize(&mm(&ra, &self.bodies[c.a].f.r));
                self.bodies[c.b].f.r = orthonormalize(&mm(&rb, &self.bodies[c.b].f.r));
            }
        }
    }

    /// World transforms of the simulated bones, by body name (lowercase).
    pub fn output(&self) -> HashMap<String, Transform> {
        self.bodies
            .iter()
            .filter(|b| b.inv_mass > 0.0)
            .map(|b| (b.name.to_ascii_lowercase(), Transform::from_av(b.f.p, b.f.r, b.scale)))
            .collect()
    }

    /// Lines between constrained bodies, for the debug overlay.
    pub fn links(&self) -> Vec<([f32; 3], [f32; 3], bool)> {
        self.constraints.iter().map(|c| (self.bodies[c.a].f.p, self.bodies[c.b].f.p, self.bodies[c.a].inv_mass > 0.0 || self.bodies[c.b].inv_mass > 0.0)).collect()
    }
}

/// Load every physics XML of the character's pieces.
pub fn load_configs(app: &StudioApp, c: &Character) -> Vec<(String, Config)> {
    let Some(l) = &app.loaded else { return Vec::new() };
    let mut out = Vec::new();
    for path in &c.smp_files {
        let p = path.replace('\\', "/");
        let p = if p.to_ascii_lowercase().starts_with("meshes/") { p } else { format!("meshes/{p}") };
        let cfg = match l.vfs.read(&p).map_err(|e| e.to_string()).and_then(|b| parse_xml(&String::from_utf8_lossy(&b))) {
            Ok(root) => read_config(&root),
            Err(e) => Config { notes: vec![format!("{e}")], ..Default::default() },
        };
        out.push((p, cfg));
    }
    out
}

/// Bring the character's physics to clip time `t`, stepping from where it
/// was (or from the clip start after a jump back); writes the simulated
/// bones into `character.phys`.
pub fn update(app: &mut StudioApp, t: f32, root_at: &dyn Fn(f32) -> Transform) {
    let key = match (&app.character, &app.doc) {
        (Some(c), Some(d)) if !c.smp_files.is_empty() && app.scene.smp => format!("{}#{}#{}", c.id, app.character_gen, d.name),
        _ => {
            if let Some(c) = &mut app.character {
                c.phys.clear();
            }
            return;
        }
    };
    let pose_at = |app: &StudioApp, time: f32| -> Vec<Transform> {
        let c = app.character.as_ref().unwrap();
        let local = match &app.anim {
            Some(a) => sds_hkx::anim::local_pose(&c.skeleton, a, time),
            None => c.skeleton.reference.clone(),
        };
        sds_hkx::anim::model_pose(&c.skeleton, &local).iter().map(crate::character::to_transform).collect()
    };
    let mut sys = std::mem::take(&mut app.smp);
    if sys.key != key || t + 1e-4 < sys.time {
        let cfgs = load_configs(app, app.character.as_ref().unwrap());
        let model = pose_at(app, 0.0);
        sys = System::build(app.character.as_ref().unwrap(), &cfgs, &Pose { model: &model, root: root_at(0.0) });
        sys.key = key;
        for n in sys.notes.clone() {
            app.log(format!("HDT-SMP: {n}"));
        }
        app.log(sds_i18n::tf!("HDT-SMP: {} динамических костей, {} связей", "HDT-SMP: {} dynamic bones, {} constraints", sys.dynamic_count(), sys.constraints.len()));
    }
    let mut steps = 0;
    let (mut t_pose, mut t_step) = (0.0f64, 0.0f64);
    let mut prev = pose_at(app, sys.time);
    while sys.time + DT <= t + 1e-4 && steps < MAX_STEPS_PER_FRAME {
        let time = sys.time + DT;
        let clock = std::time::Instant::now();
        let model = pose_at(app, time);
        t_pose += clock.elapsed().as_secs_f64();
        let clock = std::time::Instant::now();
        sys.step(&Pose { model: &prev, root: root_at(sys.time) }, &Pose { model: &model, root: root_at(time) });
        t_step += clock.elapsed().as_secs_f64();
        prev = model;
        steps += 1;
    }
    if std::env::var_os("SDS_SMP_DEBUG").is_some() {
        let model = pose_at(app, sys.time);
        let root = root_at(sys.time);
        let mut worst = (0.0f32, String::new());
        let mut nan = 0;
        for body in sys.bodies.iter().filter(|b| b.inv_mass > 0.0) {
            if !body.f.p.iter().chain(body.f.r.iter()).all(|v| v.is_finite()) {
                nan += 1;
                continue;
            }
            let anim = Character::world(&model, &root, &body.bone).t;
            let d = len(sub(body.f.p, anim));
            if d > worst.0 {
                worst = (d, body.name.clone());
            }
        }
        eprintln!("SMPDBG t={:.3} steps={steps} nan={nan} worst={:.1} ({}) contacts={} pose={:.0}ms step={:.0}ms", sys.time, worst.0, worst.1, sys.contacts, t_pose * 1e3, t_step * 1e3);
    }
    if let Some(c) = &mut app.character {
        c.phys = sys.output();
    }
    app.smp = sys;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_hdt_xml() {
        let x = r#"<?xml version="1.0"?>
<system><!-- k -->
<bone-default><mass>0</mass></bone-default>
<bone name="NPC Spine2 [Spn2]"/>
<bone-default><mass>0,5</mass><linearDamping>0.6</linearDamping></bone-default>
<bone name="hdt A"/>
<generic-constraint-default><angularLowerLimit x="-0.1" y="0" z="0"/><angularUpperLimit x="0.1" y="0" z="0"/></generic-constraint-default>
<generic-constraint bodyA="hdt A" bodyB ="NPC Spine2 [Spn2]"></generic-constraint>
<per-triangle-shape name="coll"><tag>a</tag></per-triangle-shape>
</system>"#;
        let cfg = read_config(&parse_xml(x).unwrap());
        assert_eq!(cfg.bodies.len(), 2);
        assert_eq!(cfg.bodies[0].mass, 0.0);
        assert_eq!(cfg.bodies[1].mass, 0.5);
        assert_eq!(cfg.bodies[1].lin_damp, 0.6);
        assert_eq!(cfg.constraints.len(), 1);
        let Kind::SixDof(s) = &cfg.constraints[0].kind else { panic!() };
        assert_eq!(s.ang[1][0], 0.1);
        assert_eq!(cfg.shapes.len(), 1);
        assert_eq!(cfg.shapes[0].tags, vec!["a".to_string()]);
    }

    /// A two-link chain let go horizontally: `a` is pinned at its own origin
    /// (frameInA = identity puts the pivot there) and turns, `b` swings down
    /// under it and settles.
    #[test]
    fn chain_hangs_under_gravity() {
        let body = |name: &str, p: [f32; 3], inv_mass: f32| Body {
            name: name.into(),
            bone: BoneRef { havok: None, rel: Transform::from_av(p, I3, 1.0) },
            inv_mass,
            lin_damp: 0.95,
            ang_damp: 0.95,
            gravity: 1.0,
            f: Frame { r: I3, p },
            prev: Frame { r: I3, p },
            v: [0.0; 3],
            w: [0.0; 3],
            scale: 1.0,
            depth: 0,
            inv_inertia: 0.0,
            margin_mult: 1.0,
        };
        let mut s = System { bodies: vec![body("root", [0.0; 3], 0.0), body("a", [10.0, 0.0, 0.0], 1.0), body("b", [20.0, 0.0, 0.0], 1.0)], ..Default::default() };
        let free = SixDof { ang: [[1.0; 3], [-1.0; 3]], ..Default::default() };
        for (a, b) in [(1, 0), (2, 1)] {
            let (fa, fb) = (Frame::ID, s.bodies[b].f.inverse().compose(&s.bodies[a].f));
            s.constraints.push(Constraint { a, b, kind: Kind::SixDof(free.clone()), fa, fb, rest: 10.0 });
        }
        s.bodies[1].depth = 1;
        s.bodies[2].depth = 2;
        s.set_inertia();
        let pose = Pose { model: &[], root: Transform::IDENTITY };
        for _ in 0..240 {
            s.step(&pose, &pose);
        }
        let (a, b) = (s.bodies[1].f.p, s.bodies[2].f.p);
        assert!((len(a) - 10.0).abs() < 0.5 && (len(sub(b, a)) - 10.0).abs() < 0.5, "lengths kept: {a:?} {b:?}");
        assert!(b[2] < -9.0 && (b[0] - 10.0).abs() < 4.0, "hangs: {b:?}");
    }

    #[test]
    fn rotation_vector_round_trip() {
        let v = [0.3, -0.2, 0.5];
        let r = rot_vec(&from_rot_vec(v));
        assert!((0..3).all(|i| (r[i] - v[i]).abs() < 1e-4), "{r:?}");
    }
}
