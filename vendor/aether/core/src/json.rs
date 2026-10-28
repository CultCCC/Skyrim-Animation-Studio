//! Minimal dependency-free JSON reader/writer used for project files and
//! templates. Not a general-purpose JSON library: it supports exactly what
//! Aether writes (objects, arrays, strings, finite numbers, bools, null),
//! preserves object key order, and produces stable pretty output so project
//! files diff cleanly under version control.

use std::collections::BTreeMap;
use std::fmt::Write as _;

#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    /// Ordered object: insertion order is preserved.
    Obj(Vec<(String, Json)>),
}

impl Json {
    pub fn obj() -> Json {
        Json::Obj(Vec::new())
    }

    pub fn set(&mut self, key: &str, value: Json) {
        if let Json::Obj(entries) = self {
            if let Some(e) = entries.iter_mut().find(|(k, _)| k == key) {
                e.1 = value;
            } else {
                entries.push((key.to_string(), value));
            }
        }
    }

    pub fn with(mut self, key: &str, value: Json) -> Json {
        self.set(key, value);
        self
    }

    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Json::Num(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_f32(&self) -> Option<f32> {
        self.as_f64().map(|n| n as f32)
    }

    pub fn as_u64(&self) -> Option<u64> {
        self.as_f64().and_then(|n| {
            if n >= 0.0 && n.fract() == 0.0 && n <= u64::MAX as f64 {
                Some(n as u64)
            } else {
                None
            }
        })
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Json::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_arr(&self) -> Option<&[Json]> {
        match self {
            Json::Arr(a) => Some(a),
            _ => None,
        }
    }

    /// Object entries, if this is an object.
    pub fn entries(&self) -> Option<&[(String, Json)]> {
        match self {
            Json::Obj(e) => Some(e),
            _ => None,
        }
    }

    pub fn from_map(map: &BTreeMap<String, String>) -> Json {
        Json::Obj(
            map.iter()
                .map(|(k, v)| (k.clone(), Json::Str(v.clone())))
                .collect(),
        )
    }

    // ---- writing ----

    pub fn pretty(&self) -> String {
        let mut out = String::new();
        self.write_pretty(&mut out, 0);
        out.push('\n');
        out
    }

    fn write_pretty(&self, out: &mut String, indent: usize) {
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Json::Num(n) => write_number(out, *n),
            Json::Str(s) => write_string(out, s),
            Json::Arr(items) => {
                if items.is_empty() {
                    out.push_str("[]");
                    return;
                }
                let simple = items.len() <= 8
                    && items
                        .iter()
                        .all(|i| matches!(i, Json::Num(_) | Json::Bool(_) | Json::Null));
                if simple {
                    out.push('[');
                    for (i, item) in items.iter().enumerate() {
                        if i > 0 {
                            out.push_str(", ");
                        }
                        item.write_pretty(out, indent);
                    }
                    out.push(']');
                    return;
                }
                out.push_str("[\n");
                for (i, item) in items.iter().enumerate() {
                    push_indent(out, indent + 1);
                    item.write_pretty(out, indent + 1);
                    if i + 1 < items.len() {
                        out.push(',');
                    }
                    out.push('\n');
                }
                push_indent(out, indent);
                out.push(']');
            }
            Json::Obj(entries) => {
                if entries.is_empty() {
                    out.push_str("{}");
                    return;
                }
                out.push_str("{\n");
                for (i, (k, v)) in entries.iter().enumerate() {
                    push_indent(out, indent + 1);
                    write_string(out, k);
                    out.push_str(": ");
                    v.write_pretty(out, indent + 1);
                    if i + 1 < entries.len() {
                        out.push(',');
                    }
                    out.push('\n');
                }
                push_indent(out, indent);
                out.push('}');
            }
        }
    }

    // ---- parsing ----

    pub fn parse(text: &str) -> Result<Json, JsonError> {
        let mut p = Parser {
            bytes: text.as_bytes(),
            pos: 0,
        };
        p.skip_ws();
        let v = p.value()?;
        p.skip_ws();
        if p.pos != p.bytes.len() {
            return Err(p.err("trailing data"));
        }
        Ok(v)
    }
}

fn push_indent(out: &mut String, n: usize) {
    for _ in 0..n {
        out.push_str("  ");
    }
}

fn write_number(out: &mut String, n: f64) {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        let _ = write!(out, "{}", n as i64);
    } else {
        let _ = write!(out, "{n}");
    }
}

fn write_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

#[derive(Debug)]
pub struct JsonError {
    pub offset: usize,
    pub message: String,
}

impl std::fmt::Display for JsonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "JSON error at byte {}: {}", self.offset, self.message)
    }
}

impl std::error::Error for JsonError {}

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn err(&self, msg: impl Into<String>) -> JsonError {
        JsonError {
            offset: self.pos,
            message: msg.into(),
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.pos += 1;
        }
    }

    fn expect(&mut self, b: u8) -> Result<(), JsonError> {
        if self.peek() == Some(b) {
            self.pos += 1;
            Ok(())
        } else {
            Err(self.err(format!("expected {:?}", b as char)))
        }
    }

    fn value(&mut self) -> Result<Json, JsonError> {
        match self.peek() {
            Some(b'{') => self.object(),
            Some(b'[') => self.array(),
            Some(b'"') => Ok(Json::Str(self.string()?)),
            Some(b't') => self.literal("true", Json::Bool(true)),
            Some(b'f') => self.literal("false", Json::Bool(false)),
            Some(b'n') => self.literal("null", Json::Null),
            Some(c) if c == b'-' || c.is_ascii_digit() => self.number(),
            _ => Err(self.err("expected a JSON value")),
        }
    }

    fn literal(&mut self, lit: &str, v: Json) -> Result<Json, JsonError> {
        if self.bytes[self.pos..].starts_with(lit.as_bytes()) {
            self.pos += lit.len();
            Ok(v)
        } else {
            Err(self.err(format!("expected {lit}")))
        }
    }

    fn number(&mut self) -> Result<Json, JsonError> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        while matches!(self.peek(), Some(c) if c.is_ascii_digit() || matches!(c, b'.' | b'e' | b'E' | b'+' | b'-'))
        {
            self.pos += 1;
        }
        let text = std::str::from_utf8(&self.bytes[start..self.pos]).unwrap();
        text.parse::<f64>()
            .map(Json::Num)
            .map_err(|_| self.err(format!("bad number {text:?}")))
    }

    fn string(&mut self) -> Result<String, JsonError> {
        self.expect(b'"')?;
        let mut out = String::new();
        loop {
            match self.peek() {
                None => return Err(self.err("unterminated string")),
                Some(b'"') => {
                    self.pos += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    self.pos += 1;
                    match self.peek() {
                        Some(b'"') => out.push('"'),
                        Some(b'\\') => out.push('\\'),
                        Some(b'/') => out.push('/'),
                        Some(b'n') => out.push('\n'),
                        Some(b'r') => out.push('\r'),
                        Some(b't') => out.push('\t'),
                        Some(b'b') => out.push('\u{8}'),
                        Some(b'f') => out.push('\u{c}'),
                        Some(b'u') => {
                            if self.pos + 5 > self.bytes.len() {
                                return Err(self.err("bad \\u escape"));
                            }
                            let hex = std::str::from_utf8(&self.bytes[self.pos + 1..self.pos + 5])
                                .map_err(|_| self.err("bad \\u escape"))?;
                            let code = u32::from_str_radix(hex, 16)
                                .map_err(|_| self.err("bad \\u escape"))?;
                            out.push(char::from_u32(code).unwrap_or('\u{fffd}'));
                            self.pos += 4;
                        }
                        _ => return Err(self.err("bad escape")),
                    }
                    self.pos += 1;
                }
                Some(_) => {
                    // Consume one UTF-8 scalar.
                    let s = std::str::from_utf8(&self.bytes[self.pos..])
                        .map_err(|_| self.err("invalid utf-8"))?;
                    let c = s.chars().next().unwrap();
                    out.push(c);
                    self.pos += c.len_utf8();
                }
            }
        }
    }

    fn array(&mut self) -> Result<Json, JsonError> {
        self.expect(b'[')?;
        let mut items = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Ok(Json::Arr(items));
        }
        loop {
            self.skip_ws();
            items.push(self.value()?);
            self.skip_ws();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b']') => {
                    self.pos += 1;
                    return Ok(Json::Arr(items));
                }
                _ => return Err(self.err("expected ',' or ']'")),
            }
        }
    }

    fn object(&mut self) -> Result<Json, JsonError> {
        self.expect(b'{')?;
        let mut entries = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(Json::Obj(entries));
        }
        loop {
            self.skip_ws();
            let key = self.string()?;
            self.skip_ws();
            self.expect(b':')?;
            self.skip_ws();
            let value = self.value()?;
            entries.push((key, value));
            self.skip_ws();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(Json::Obj(entries));
                }
                _ => return Err(self.err("expected ',' or '}'")),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let v = Json::obj()
            .with("name", Json::Str("Fire — жар \"test\"".into()))
            .with("count", Json::Num(42.0))
            .with("rate", Json::Num(1.5))
            .with("on", Json::Bool(true))
            .with("items", Json::Arr(vec![Json::Num(1.0), Json::Null]));
        let text = v.pretty();
        let parsed = Json::parse(&text).unwrap();
        assert_eq!(v, parsed);
    }

    /// A Windows path survives, backslashes and all.
    ///
    /// The round trip above covers a quote and non-ASCII and stops
    /// there, which was fine while nothing stored a filesystem path.
    /// The project manifest now records the NIF each graph came from,
    /// and an unportable one is kept VERBATIM — `C:\Games\...` — so the
    /// backslash escape and its inverse are load-bearing. A UNC path
    /// leads with two of them, which is where an off-by-one in the
    /// unescaper shows up first.
    ///
    /// Planted defect: drop the `'\\' => "\\\\"` arm from the writer.
    /// `C:\temp\x.nif` then emits `"C:\temp\x.nif"`, whose `\t` the
    /// parser reads as a TAB — a different path, silently, and one that
    /// resolves to nothing.
    #[test]
    fn a_windows_path_round_trips_through_the_string_codec() {
        let bs = char::from(92); // built, never typed: see the probe trap
        let paths = [
            format!("C:{bs}Games{bs}Skyrim{bs}Data{bs}meshes{bs}fx{bs}a.nif"),
            format!("{bs}{bs}server{bs}share{bs}mods{bs}b.nif"),
            format!("C:{bs}temp{bs}x.nif"),   // \t
            format!("C:{bs}new{bs}y.nif"),    // \n
            format!("C:{bs}return{bs}z.nif"), // \r
            format!("C:{bs}u0041{bs}w.nif"),  // \u
        ];
        for p in &paths {
            let v = Json::obj().with("source_nif", Json::Str(p.clone()));
            let parsed = Json::parse(&v.pretty()).expect("parses");
            assert_eq!(
                parsed.get("source_nif").and_then(Json::as_str),
                Some(p.as_str()),
                "{p} did not survive"
            );
        }
        // Control characters go out as \u escapes and come back.
        let odd = "a\u{1}b\u{8}c\u{c}d";
        let v = Json::obj().with("k", Json::Str(odd.into()));
        assert_eq!(
            Json::parse(&v.pretty())
                .unwrap()
                .get("k")
                .and_then(Json::as_str),
            Some(odd)
        );
    }
}
