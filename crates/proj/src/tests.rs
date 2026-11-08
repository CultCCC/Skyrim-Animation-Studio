use crate::model::*;
use crate::pattern;

const SAMPLE: &str = r#"{
  // comment
  "FormIDs": { "key_projFireball": "Skyrim.esm|0x10FBED", "key_spell": "Rebalance.esp|0x02444b" },
  "HomingData": { "key_H": { "type": "ConstSpeed", "rotationTime": 2.5, "target": "Cursor", "cursorAngle": 10 } },
  "MulticastSpawnGroups": {
    "key_SG": { "Pattern": { "Figure": { "shape": "Line", "count": 5, "size": 200, "sound": "Single" }, "xDepends": false }, "rotation": "ToSight" }
  },
  "MulticastData": { "key_MC": [ { "spawn_group": "key_SG", "spellID": "Current", "callTriggers": true } ] },
  "EmittersData": { "key_EM": { "interval": 0.5, "limited": true, "count": 3,
     "functions": [ { "type": "TriggerFunctions", "TriggerFunctions": { "functions": [ { "type": "ApplyMultiCast", "id": "key_MC" } ] } },
                    { "type": "AccelerateToMaxSpeed", "time": 1, "speedType": "Linear" } ] } },
  "FollowersData": { "key_F": { "Pattern": { "Figure": { "shape": "Circle", "count": 6, "size": 100 } }, "rounding": "Sphere", "roundingR": 15 } },
  "Triggers": [
    { "event": "ProjAppeared", "conditions": { "ProjBaseIsFormID": "key_projFireball", "CasterIsFormID": "0x14" },
      "TriggerFunctions": { "functions": [ { "type": "SetHoming", "id": "key_H" }, { "type": "SetEmitter", "id": "key_EM" },
                                           { "type": "ChangeSpeed", "data": { "type": "Mul", "value": 0.5 } } ], "disableOrigin": false } },
  ],
  "key_stray": { "x": 1 }
}"#;

fn strip(mut c: Config) -> Config {
    c.notes.clear();
    c.cond_format = None;
    c
}

#[test]
fn parse_and_round_trip_object() {
    let c = Config::parse(SAMPLE).unwrap();
    assert_eq!(c.cond_format, Some(CondFormat::Object));
    assert_eq!(c.triggers[0].conditions.len(), 2);
    assert_eq!(c.spawn_groups[0].1.pattern.figure.extra.get("sound").and_then(|v| v.as_str()), Some("Single"));
    assert!(c.notes.iter().any(|n| n.contains("sound")));
    assert!(c.notes.iter().any(|n| n.contains("key_stray")));
    let out = c.to_json(CondFormat::Object);
    let c2 = Config::parse(&out).unwrap();
    assert_eq!(strip(c.clone()), strip(c2));
    // Unknown root key survives.
    assert!(out.contains("key_stray"));
}

#[test]
fn convert_to_array_and_back() {
    let c = Config::parse(SAMPLE).unwrap();
    let arr = c.to_json(CondFormat::Array);
    assert!(arr.contains("\"type\": \"ProjBaseIsFormID\""));
    assert!(arr.contains("\"formID\": \"key_projFireball\""));
    let c2 = Config::parse(&arr).unwrap();
    assert_eq!(c2.cond_format, Some(CondFormat::Array));
    assert_eq!(strip(c), strip(c2));
}

#[test]
fn numbers_keep_their_look() {
    let c = Config::parse(SAMPLE).unwrap();
    let out = c.to_json(CondFormat::Object);
    assert!(out.contains("\"rotationTime\": 2.5"));
    assert!(out.contains("\"count\": 5"));
    assert!(out.contains("\"value\": 0.5"));
    assert!(out.contains("\"size\": 200,") || out.contains("\"size\": 200\n"));
}

#[test]
fn rename_updates_refs() {
    let mut c = Config::parse(SAMPLE).unwrap();
    c.rename_key(Section::Multicast, "key_MC", "key_Rain");
    assert_eq!(c.emitters[0].1.functions[0].tf.functions[0].id, "key_Rain");
    assert_eq!(c.ref_count(Section::Multicast, "key_Rain"), 1);
    c.rename_key(Section::FormIds, "key_projFireball", "key_FB");
    assert_eq!(c.triggers[0].conditions[0].value, "key_FB");
}

#[test]
fn line_points() {
    let c = Config::parse(SAMPLE).unwrap();
    let p = pattern::points(&c.spawn_groups[0].1.pattern, 0.0);
    assert_eq!(p.points.len(), 5);
    assert!((p.points[0][0] + 100.0).abs() < 1e-3);
    assert!((p.points[4][0] - 100.0).abs() < 1e-3);
    assert!(p.points.iter().all(|q| q[1].abs() < 1e-3 && q[2].abs() < 1e-3));
}

#[test]
fn circle_is_in_the_facing_plane() {
    let c = Config::parse(SAMPLE).unwrap();
    let p = pattern::points(&c.followers[0].1.pattern, 0.0);
    for q in &p.points {
        assert!(q[1].abs() < 1e-3, "default normal [0,1,0] → figure in the XZ plane");
        assert!(((q[0] * q[0] + q[2] * q[2]).sqrt() - 100.0).abs() < 1e-2);
    }
}

#[test]
fn validation_without_world() {
    let c = Config::parse(SAMPLE).unwrap();
    let issues = crate::validate::validate(&c, None, CondFormat::Object);
    assert!(!issues.iter().any(|i| i.level == crate::validate::Level::Error), "{issues:?}");
    let mut bad = c.clone();
    bad.triggers[0].tf.functions[0].id = "key_nope".into();
    bad.triggers[0].conditions[0].invert = true;
    let issues = crate::validate::validate(&bad, None, CondFormat::Object);
    assert!(issues.iter().filter(|i| i.level == crate::validate::Level::Error).count() >= 2, "{issues:?}");
}

#[test]
fn chains_unfold() {
    let c = Config::parse(SAMPLE).unwrap();
    let cx = crate::chain::Ctx { c: &c, w: None };
    let ch = crate::chain::build(&cx);
    assert_eq!(ch.lanes.len(), 1);
    let kinds: Vec<_> = ch.lanes[0].children.iter().map(|(_, n)| n.kind).collect();
    assert_eq!(kinds, vec![crate::chain::Kind::Homing, crate::chain::Kind::Emitter, crate::chain::Kind::Func]);
    // Emitter → multicast → group item.
    let em = &ch.lanes[0].children[1].1;
    assert_eq!(em.children[0].1.kind, crate::chain::Kind::Multicast);
    assert_eq!(em.children[0].1.children[0].1.kind, crate::chain::Kind::McItem);
}

/// Every config the SDS build ships parses and survives a round trip.
#[test]
fn sds_configs_round_trip() {
    let dir = std::path::Path::new(r"F:\SDS\SkyrimSDS\[SDS] - Mod organizer\mods\[SDS Core] - FenixProjectilesAPI\HomingProjectiles");
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut n = 0;
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().is_none_or(|x| x != "json") || p.file_name().is_some_and(|f| f.to_string_lossy().eq_ignore_ascii_case("schema.json")) {
            continue;
        }
        let text = std::fs::read_to_string(&p).unwrap();
        let c = Config::parse(&text).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
        let fmt = c.cond_format.unwrap_or_default();
        let c2 = Config::parse(&c.to_json(fmt)).unwrap();
        assert_eq!(strip(c.clone()), strip(c2), "{}", p.display());
        // Same JSON value as the original (key order aside).
        let a: serde_json::Value = serde_json::from_str(&relax(&text)).unwrap();
        let b = c.to_value(fmt);
        assert!(same(&a, &b), "{} differs after export", p.display());
        n += 1;
    }
    assert!(n > 0);
}

fn same(a: &serde_json::Value, b: &serde_json::Value) -> bool {
    use serde_json::Value::*;
    match (a, b) {
        (Object(x), Object(y)) => x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| same(v, w))),
        (Array(x), Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(v, w)| same(v, w)),
        (Number(x), Number(y)) => (x.as_f64().unwrap() - y.as_f64().unwrap()).abs() < 1e-6,
        _ => a == b,
    }
}
