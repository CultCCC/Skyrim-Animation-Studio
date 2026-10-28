//! Typed parameter values and socket data types for the particle graph.

use crate::json::Json;

/// Data type carried by a socket. Drives pin colors/shapes in the UI and
/// connection validation in the graph model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SocketType {
    /// A whole particle system (NiParticleSystem slot under a scene node).
    System,
    /// Particle geometry data (NiPSysData).
    Data,
    /// A particle modifier (emitters, forces, color/scale, ...).
    Modifier,
    /// A time controller (NiPSysEmitterCtlr, NiPSysUpdateCtlr, ...).
    Controller,
    /// Effect shader (BSEffectShaderProperty).
    Shader,
    /// Alpha blending state (NiAlphaProperty).
    Alpha,
    /// Scene object reference (NiNode target, gravity/bomb object...).
    Object,
    Float,
    Vector,
    Color,
    Bool,
    Texture,
}

impl SocketType {
    pub fn name(self) -> &'static str {
        match self {
            SocketType::System => "System",
            SocketType::Data => "Data",
            SocketType::Modifier => "Modifier",
            SocketType::Controller => "Controller",
            SocketType::Shader => "Shader",
            SocketType::Alpha => "Alpha",
            SocketType::Object => "Object",
            SocketType::Float => "Float",
            SocketType::Vector => "Vector",
            SocketType::Color => "Color",
            SocketType::Bool => "Bool",
            SocketType::Texture => "Texture",
        }
    }

    /// Whether an output of type `self` may connect into an input of `input`.
    pub fn can_connect_to(self, input: SocketType) -> bool {
        self == input
    }
}

/// A parameter value on a node.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    F32(f32),
    U32(u32),
    Bool(bool),
    Str(String),
    /// A game-relative asset path (`textures\effects\foo.dds`). Distinct
    /// from `Str` so the UI can offer a browser and a preview, and so the
    /// renderer knows which fields to resolve through the asset paths.
    TexturePath(String),
    Vec3([f32; 3]),
    /// RGBA, linear 0..1.
    Color4([f32; 4]),
    /// Choice from a fixed enum set, stored by name for file stability.
    Enum(&'static [&'static str], usize),
    /// A keyed track over controller-local time. Distinct from `Str`
    /// for the same reason `TexturePath` is: the UI has to know to
    /// draw a curve rather than a line of text, and the document has
    /// to hold the key TYPE as a field instead of leaving it to be
    /// recovered by counting punctuation in a sentence.
    Curve(crate::curve::Curve),
}

impl Value {
    pub fn as_f32(&self) -> Option<f32> {
        match self {
            Value::F32(v) => Some(*v),
            Value::U32(v) => Some(*v as f32),
            _ => None,
        }
    }

    pub fn type_label(&self) -> &'static str {
        match self {
            Value::F32(_) => "float",
            Value::U32(_) => "uint",
            Value::Bool(_) => "bool",
            Value::Str(_) => "string",
            Value::TexturePath(_) => "texture",
            Value::Vec3(_) => "vec3",
            Value::Color4(_) => "color",
            Value::Enum(..) => "enum",
            Value::Curve(_) => "curve",
        }
    }

    pub fn as_curve(&self) -> Option<&crate::curve::Curve> {
        match self {
            Value::Curve(c) => Some(c),
            _ => None,
        }
    }

    pub fn to_json(&self) -> Json {
        match self {
            Value::F32(v) => Json::obj().with("f32", Json::Num(*v as f64)),
            Value::U32(v) => Json::obj().with("u32", Json::Num(*v as f64)),
            Value::Bool(v) => Json::obj().with("bool", Json::Bool(*v)),
            Value::Str(v) => Json::obj().with("str", Json::Str(v.clone())),
            Value::TexturePath(v) => Json::obj().with("texture", Json::Str(v.clone())),
            Value::Vec3(v) => Json::obj().with(
                "vec3",
                Json::Arr(v.iter().map(|c| Json::Num(*c as f64)).collect()),
            ),
            Value::Color4(v) => Json::obj().with(
                "color4",
                Json::Arr(v.iter().map(|c| Json::Num(*c as f64)).collect()),
            ),
            Value::Enum(names, idx) => Json::obj().with(
                "enum",
                Json::Str(names.get(*idx).copied().unwrap_or("?").to_string()),
            ),
            Value::Curve(c) => Json::obj().with("curve", c.to_json()),
        }
    }

    /// Restore a value from JSON, using `prototype` to recover static enum
    /// tables and to keep types stable across versions.
    pub fn from_json(json: &Json, prototype: &Value) -> Option<Value> {
        match prototype {
            Value::F32(_) => json.get("f32")?.as_f32().map(Value::F32),
            Value::U32(_) => json.get("u32")?.as_u64().map(|v| Value::U32(v as u32)),
            Value::Bool(_) => json.get("bool")?.as_bool().map(Value::Bool),
            Value::Str(_) => json.get("str")?.as_str().map(|s| Value::Str(s.to_string())),
            Value::TexturePath(_) => json
                .get("texture")?
                .as_str()
                .map(|s| Value::TexturePath(s.to_string())),
            Value::Vec3(_) => {
                let arr = json.get("vec3")?.as_arr()?;
                if arr.len() != 3 {
                    return None;
                }
                let mut v = [0.0f32; 3];
                for (i, item) in arr.iter().enumerate() {
                    v[i] = item.as_f32()?;
                }
                Some(Value::Vec3(v))
            }
            Value::Color4(_) => {
                let arr = json.get("color4")?.as_arr()?;
                if arr.len() != 4 {
                    return None;
                }
                let mut v = [0.0f32; 4];
                for (i, item) in arr.iter().enumerate() {
                    v[i] = item.as_f32()?;
                }
                Some(Value::Color4(v))
            }
            Value::Enum(names, _) => {
                let name = json.get("enum")?.as_str()?;
                let idx = names.iter().position(|n| *n == name)?;
                Some(Value::Enum(names, idx))
            }
            // The SCHEMA owns what kind of track this parameter is, the
            // same way it owns an `Enum`'s option list: a stored
            // "float" landing in a bool slot would give the widget an
            // on/off track it draws as a ramp and the compiler writes
            // as `NiFloatData`. The stored kind still decides how many
            // numbers each key carries, so it is read first and only
            // then overridden.
            Value::Curve(proto) => {
                let mut c = crate::curve::Curve::from_json(json.get("curve")?)?;
                if c.track != proto.track && c.channels() != proto.channels() {
                    return None;
                }
                c.track = proto.track;
                Some(Value::Curve(c))
            }
        }
    }
}
