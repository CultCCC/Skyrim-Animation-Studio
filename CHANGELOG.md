# Changelog

Русская версия: [CHANGELOG_RU.md](CHANGELOG_RU.md).

## 0.2.0 — 2026-11-08

### Annotations and editing
- **Root Motion Editor:** the clip's `animmotion` path as X/Y Bézier curves in a Blender-style graph (keys, Auto / Aligned / Free / Vector handles, box select, top view); every change is baked back into per-frame keys as one undo step.
- **Precision:** parameters are joined with `|` (`Collision_Add.Node(WEAPON)|Radius(25.5)`); old `.`-separated texts get a warning and a «Fix» button.
- **MCO_AttackSpeed:** `PIE.@SGVF|MCO_AttackSpeed|x` sets the playback speed from its key on, multiplied by the speed field; «MCO» checkbox.
- **SDSCombat GoThrough:** `SDS_GoThroughTarget_On` / `_Off` are drawn as a span, with an On/Off template and a hint.

### Spells and effects in the viewport
- **Projectiles:** non-self `@CAST` spells launch their projectile from the hand's magic node:
  - missiles and arrows fly straight and fall with their gravity (gravity 10000 drops at the caster's feet);
  - lobbers keep the aimed arc;
  - beams and flames are instant;
  - cones fly their range and widen by their Cone Spread.
- **Explosions and hazards:** the explosion, hit art and sounds play on impact. The blast radius comes from the spell, the magic effect or the explosion. Hazards stay for their lifetime with a radius ring and a looping sound.
- **Impacts by surface:** the impact set picks the impact for the target's skin or for the floor material.
- **Effect shaders (EFSH) on the body:** additive membrane with fill texture, fresnel edge and alpha envelopes, plus EFSH particles over the body.
- **Addon nodes:** `AddOnNode<N>` get their ADDN model (lightning, sparks, dust). World Space particles stay where they were born, so a cone wave leaves a trail.
- **Hitboxes:** projectile sphere or cone, blast sphere, hazard sphere; red when they touch the target (Scene toggle).

### Characters
- **HDT-SMP physics preview:** the NIF's physics XML is read the way FSMP does and simulated: capes, skirts, hair and other dynamic bones. Collisions work against the body and between moving shapes (cloth against cloth). Scene toggle.
- **Weapons in combat:** the scabbard stays on the belt or back. NPCs whose combat style allows dual wielding get a second one-handed weapon in the left hand; left-hand picker in the Character tab.
- **Creature clips:** opening a clip of another creature (human → draugr) loads its skeleton and default model.
- **Look:**
  - game skin, face and hair tints;
  - `_0/_1` weight morph (closes the FaceGen neck seam);
  - skin decals (scars);
  - better hair cards and eyes.
- **Paired clips:** tracks are split per actor; the partner is drawn as a copy of the character.
- **Broken bone scales:** non-finite, zero or huge scales in a clip are ignored. This fixes the «exploded head» on an SDS draugr power attack; the log names the bones.

### Export and library
- **Preview export:** pose PNG and looping GIF on a transparent background, from the viewport toolbar or in batch (`--export-batch list.txt --export-dir dir`).
- **OAR mode search:** the Library search matches OAR mod and submod folders.

### Interface
- Camera orbit is inverted by default; each axis can be toggled in the 🖱 menu.

## 0.1.0 — 2026-10-28

First public version:
- HKX playback on NPCs dressed from ESP records (MO2 profile + BSA);
- annotation editing with undo, templates and favorites;
- Precision hitboxes and trails;
- PIE art and sounds;
- SDSCombat path replay with a target dummy;
- SCAR zones, a combo graph and a combo simulator;
- OAR submods and export;
- RU / EN interface.
