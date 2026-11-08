# SDS Anim Studio — stages 0–7

[Русская версия](README_RU.md)

An animation and annotation studio for Skyrim SE in the spirit of DS Anim Studio, built for the SDS modlist.

Run `dist\sds-anim-studio.exe`. Nothing needs installing beyond system DLLs. Settings live next to the exe: `sds_anim_studio.json` and `sds_anim_studio_layout.json` (window layout).

**Interface language:** the **RU | EN** switch at the right of the menu bar; the choice is remembered. Screenshots accept `--lang en`.

## What it does

- **MO2 build.**
  - Mod order comes from the profile's `modlist.txt`, then `overwrite`.
  - BSAs are attached from the profile's `Skyrim.ini` and the active plugins in `plugins.txt`.
  - Loose files always win over BSAs.
  - Indexing the SDS build takes about 1 s: 694 mods, 149 BSAs, ~396k files, 17,896 animations.
  - A plain Data folder can be used instead of MO2.
- **Library.**
  - «Folders» mode: the `meshes\**\animations\**.hkx` tree.
  - «OAR» mode: mods → submods from `config.json`, with name, author, priority (`user.json` included) and an «off» mark.
  - Search: in «Folders» mode by animation path; in «OAR» mode by OAR folders: a mod by its name, author or folder (with all its submods), else its submods by name or folder; matches open up.
  - An `.hkx` can be opened from disk or dropped onto the window.
- **Timeline**, TAE-style:
  - one row per category: Precision, PIE/FX, SDSCombat, SCAR, MCO/BFCO, attack events, sound, footsteps, root motion, other;
  - diamonds for points, bars for start/end pairs: MCO/BFCO windows, `Collision_Add`→`Remove`/`Duration`, TargetLock, ReachTarget, CastOK;
  - a ruler in frames or seconds, a frame grid, the playhead;
  - root motion folds into one row with a speed curve, and a «root values» lane under it shows the animmotion (x, y, z) and animrotation (↻°) numbers; with dense keys only those with room are labelled, zooming in shows all. The «root values» checkbox in the timeline bar hides the lane.
  - playback speed (×) and the «MCO» checkbox: the clip plays at the speed its `PIE.@SGVF|MCO_AttackSpeed|x` annotations set — ×1 before the first, then the last one's value — multiplied by the × speed. The current multiplier is shown next to it; with no such annotations the checkbox is greyed out. Sounds are not sped up by MCO_AttackSpeed, as in game.
- **Timeline — mouse.**
  - The time (playhead) changes only on the frames/seconds ruler: click or drag.
  - In annotation rows a click selects (an empty spot clears the selection) and the playhead stays put.
  - An annotation or bar is grabbed at the press point and moves by the mouse travel, without jumping.
  - Dragging an empty spot scrolls the timeline.
  - **Stacked diamonds** on one frame:
    - each click on the stack selects the next annotation;
    - the tooltip lists them all (▶ marks the selected one);
    - RMB opens an «At this point» menu to pick one;
    - a drag takes the selected one.
- **Editing annotations.**
  - Drag in time with frame snapping (can be turned off).
  - Double-click a row: a new annotation from that row's template.
  - RMB on the timeline or in the «Annotations» list — a menu at the playhead:
    - «Add annotation» — templates by group (Precision, SDSCombat, MCO/BFCO, attack, SCAR, PIE). Compound templates add several annotations as one undo step:
      - `Collision_Add.Node(WEAPON)` / `Node(SHIELD)` + `Collision_Remove.Node(WEAPON/SHIELD)` 5 frames later;
      - `MCO_Windows` — `MCO_WinOpen` and `MCO_PowerWinOpen`, then `MCO_WinClose` and `MCO_PowerWinClose` 10 frames later;
      - `PIE.@SGVI|MCO_next(power)attack|N…` — a centred window for the light and power attack numbers (checkboxes keep just one), places `MCO_nextattack|N` and `MCO_nextpowerattack|M`;
    - «Add custom annotation…» — a centred window with a text field; «Add» (or Enter) places it at the playhead, Esc cancels;
    - «Favorite annotations…» — a centred window with the favorites: click one to place it at the playhead, ✕ removes it, the field and «Add Favorite» add a new one to the list (not to the timeline). Favorites are kept in the settings;
    - «Edit text…» and «Delete selected».
  - **Editing an annotation's text:** double-click a row in the «Annotations» tab (or RMB → «Edit text…») for a centred window; Enter applies (one undo step), Shift+Enter adds a line, Esc cancels.
  - Inspector: time in seconds and frames, text, parsed fields. `SCAR_ActionData` and `SDS_NPC_*` fields are edited as numbers.
  - Undo/redo, copy, and paste at the playhead.
- **3D render window:**
  - the race skeleton from `meshes\actors\<clip folder>\character assets\skeleton.hkx`;
  - the default `character` body (malebody/hands/feet/head from the build), skinned to the clip pose;
  - spline and interleaved decoding;
  - root motion from `animmotion`/`animrotation` (on/off);
  - the NPC path with annotation markers, and skeleton lines on top;
  - camera: LMB rotate, RMB pan, wheel zoom, «Camera follows NPC». Orbit is inverted on both axes by default; the 🖱 button in the view bar toggles horizontal and vertical inversion separately. «Top 2D» is the flat top view of the path.
- **Root Motion Editor** (View → «Root Motion Editor…» or the «Editor…» button in the 3D view):
  - The clip's AMR path (`animmotion`) as two Bézier curves, X (red) and Y (green), in a Blender-style graph: time across, offset up; on the right, the path from above with the NPC at the playhead.
  - On open, the clip's keys are fitted into a curve within «Accuracy» (2 units by default); the original path stays dashed. «Re-import» rebuilds the curves.
  - LMB — select and drag a key (time snaps to frames) or a selected key's handle; Shift — add to the selection / drag along one axis; LMB on empty — box select; Ctrl+LMB — a new key on the nearest curve; I — keys at the playhead; X/Delete — delete; A / Alt+A — select all / none; Home — frame all; wheel — zoom (Ctrl — value, Shift — time), MMB — pan; the top ruler moves the playhead.
  - Handles as in Blender: Auto (smooth, no overshoot), Aligned, Free, Vector (straight segments). Dragging an Auto handle makes it Aligned.
  - Every change is baked back into `animmotion` on every frame (or every Nth, «Bake every»), so the 3D view, the SDSCombat path and saving see it at once. One drag is one Ctrl+Z step. Z and `animrotation` are left as they are.
  - Nothing changes until you edit. «Restore original» puts `animmotion` back as it was when the clip was opened. Deleting every key removes `animmotion` from the clip.
  - A clip with no `animmotion` starts from flat zero curves: the keys you add are AMR root motion, which needs Animation Motion Revolution in game.
- **HDT-SMP physics** (Scene → «HDT-SMP physics», on by default):
  - The physics XML comes from the «HDT Skinned Mesh Physics Object» extra data of each NIF on the character (capes, hair, skirts). Read as FSMP does (ported in BodySlide's `src/physics/hdt`): cumulative `bone-default` / `generic-constraint-default`, `bone`, `generic-constraint`, `stiffspring-constraint`, `conetwist-constraint`, groups; bones with mass 0 follow the animation and root motion.
  - Our own solver (not Bullet): rigid-body XPBD, 60 Hz × 4 substeps × 8 passes. 6DOF constraints in B's frame: per-axis limits (lower > upper = free), XYZ Euler angles as in Bullet, springs toward the equilibrium; a correction at the anchor moves and turns both bodies (inertia of a rod as long as the link). Bullet-style damping, 9.81 m/s² gravity.
  - Stepped from the clip start; going back replays, so a frame always looks the same.
  - With the skeleton shown, the links are drawn (cyan = with dynamic bones). The log names the bone and constraint counts and bones not found.
  - Collisions: each `per-triangle-shape` / `per-vertex-shape` is a NIF shape (often an undrawn one: coll_body_*, VirtualGround, which the character keeps aside). Vertices of shapes on dynamic bones — spheres of margin × the bone's margin-multiplier — are pushed out of the triangles of fixed shapes (front side, up to `penetration` deep) unless one rules the other out by tag in `no-collide-with-tag`. The push goes to the vertex's dynamic bones by weight (translation and turn). Moving shapes also collide with each other (cape against skirt): the push is shared half and half between the vertex and the triangle's corners, cloth with `penetration` 0 is two-sided, and their triangles are taken where the bones are at the start of the step. Up to 250 vertices per shape, every other substep; ~7.5 ms per 1/60 s step on Malus (the cape alone is 7.6k triangles).
  - Not modelled yet: wind, non-Hookean springs. PNG/GIF exports pose without physics. `SDS_SMP_DEBUG=1` prints bone drift, contacts and timings.
- **NPC weapons in combat.** The blade is in the hand and the scabbard stays on the body: the `Scb*` shapes and nodes of the weapon NIF hang from the skeleton's sheath node (WeaponSword, WeaponDagger, WeaponAxe, WeaponMace, WeaponBack, WeaponBow). Without that node on the skeleton the scabbard is not drawn.
  - Two weapons: when the NPC's combat style (CSTY, AllowDualWielding flag, inherited with the AI Data template flag) allows it, a second one-handed weapon from the inventory (or a second copy when the count is ≥ 2) goes to the left hand (SHIELD node), its scabbard to the `…Left` node (XPMSE). The Character tab picks the left-hand weapon and shows the combat style.
- **NPCs from ESP** (the «Character» tab):
  - **Finding NPCs:**
    - search by EditorID, name (localized names from `.STRINGS`) or FormID among the build's 14,495 NPCs;
    - an NPC_/RACE/ARMO/ARMA/OTFT/LVLI/LVLN/WEAP index is built at load in ~0.3 s; the last plugin wins and deleted records are honoured;
    - templates: appearance from TPLT with the «traits» flag, inventory with the «inventory» flag.
  - **Race and size:** the race gives the skeleton (ANAM, male or female) and height. NPC scale = NAM6 × race height; root motion scales with it.
  - **Armor:**
    - taken from the default outfit (DOFT → OTFT; leveled lists take their first entry);
    - each armor picks the ARMA for its race (or armor race) and the MOD2/MOD3 model by sex; items whose slots are taken are skipped;
    - the NPC or race skin shows unless worn armor takes a slot the skin ARMO itself claims (a one-piece skin with extra slots, e.g. under a mask, stays); when no ARMA fits the race, the DefaultRace ARMA is used, as in game (SDS custom races such as Hevnoraak).
  - **Head:**
    - FaceGen `facegeom/<plugin>/<id>.nif`, or else the generic race head;
    - a full-face helmet hides the head, and hair hides when slot 31 is taken.
  - **Weapon:** from the inventory (CNTO, leveled lists expanded). The first melee weapon goes in hand, hung on the `WEAPON` node (a bow on `SHIELD`) of skeleton.nif. The weapon list can be switched.
  - **Pose details:**
    - bones Havok does not have (WEAPON, SHIELD, scabbards, armor-only bones such as cloaks and hair) follow the nearest animated bone rigidly, without physics;
    - the render window warns when the clip is made for another skeleton.
  - Armor parts can be toggled; «Reset to the default body» returns the clip folder's body.
  - A clip of another creature (say `meshes/actors/draugr/...` with a human chosen) swaps the skeleton and model at once: the NPC is dropped when its skeleton is not from that folder, and the creature's default look goes on — the bare skin (WNAM) of the race with that skeleton most NPCs use (Skyrim.esm first on a tie), no armor, weapons or FaceGen. For `character` it is the plain male body.
- **Precision hitboxes** (switch and list in the «Scene» tab):
  - **Precision 2.0.4 rules:**
    - `Collision_AttackStart` opens the attack; `Add`/`Remove` before it are ignored;
    - `Add` needs `Node()`;
    - `Remove.Node()` removes the first collision on the node (case-sensitive), `Remove.ID()` keeps its end-of-list quirk;
    - `AttackEnd` / `Cancel` / `attackStop` / `staggerStart` clear everything;
    - `Duration(0)` = fDefaultCollisionLifetime, with the power multiplier when the clip path contains «power»;
    - the recoil capsule goes away at `HitFrame`.
  - **The Precision editor** in the inspector covers every parameter from the mod page:
    - Node with the skeleton bone list;
    - ID, the flags;
    - DamageMult, Length/LengthMult, Radius/RadiusMult, Duration, Scale, Rotation/Translation, GroundShake;
    - the trail overrides.
    - The text is rebuilt in documentation order with `|` between parameters (`Collision_Add.Node(WEAPON)|Radius(25.5)`). For an old `.`-separated text the form offers «Fix».
  - **Sizes:**
    - `WEAPON` length = max(weapon model length or `Length()`, fMinWeaponLength) × fWeaponLengthMult × LengthMult × Scale;
    - the radius is fWeaponCapsuleRadius or `Radius()`;
    - other bones use their own capsule from skeleton.nif.
  - **Settings** come from `MCM\Config\Precision\settings.ini` with `MCM\Settings\Precision.ini` on top.
  - **3D:** red attack capsule, blue recoil capsule, a trail of the capsule end.
- **NPC path with SDSCombat** (the «Scene» tab, «Target and SDSCombat»):
  - **The target dummy** (player): distance and angle from the NPC at the clip start.
  - **Playback** runs frame by frame at 60 Hz, as `SDSCombat.dll` does:
    - **`TargetLockOn` / `TurnSpeed`:** turn toward the target at strength × fMaxEngineTurnSpeed;
    - **`TargetLockOff`:** the AI turn speed is set by hand;
    - **`StopRotation`:** holds the heading;
    - **`TargetLockReset`:** resets the lock state;
    - **`BumpOff` / `BumpOn`** (also spelt `SDS_NPCBumpOff` / `SDS_NPCBumpOn`): Animation Motion Fix's movement magnetism off until BumpOn or the clip end — a «SDS_NPC_BumpOff» bar on the timeline, in the SDSCombat templates; it does not change the path in the studio;
    - **`SDS_GoThroughTarget_On` / `_Off`:** the NPC and its combat target stop colliding (walls and the world still collide) until `_Off` or the clip end — a bar on the timeline, an On/Off pair in the SDSCombat templates; `[GoThrough]` in SDSCombat.ini;
    - **`ReachTarget.ref|min|max|arrive`:** forward steps × k = clamp(D − arrive, min, max) / ref, with ref/min/max scaled by the NPC scale.
  - **Inputs:** step direction follows `animmotion` relative to the current heading. Settings come from the build's `SKSE\Plugins\SDSCombat.ini`. Without the DLL, or with it turned off in «Scene», `SDS_NPC_*` annotations are ignored.
  - **3D:** the path with SDSCombat in colour and without it in grey, the target, the `arrive` rings, the heading arrow.
  - The status line shows the lock mode and the lunge multiplier.
- **OAR** (the «OAR» tab and OAR library mode):
  - **Library:**
    - mods → submods with priority and «off»/«user.json» marks;
    - «Add an OAR folder from disk…»;
    - **«⚙ properties & export»** under each submod selects it and opens the OAR tab, even if the tab was closed in the layout.
  - **Opening a clip** from an OAR submod selects that submod.
  - **Submod editor:**
    - `name`, `description`, `priority`, `disabled`, `interruptible`, `replaceOnLoop`, `replaceOnEcho`;
    - the condition tree and its JSON with validation;
    - a warning about equal priorities among submods sharing clips.
  - **«Who replaces this clip»:** every competing submod by priority, ★ for the winner, the open submod's place, and the base clip.
  - **Export to the output folder:**
    - «Override in place (user.json)»;
    - «New submod» (a copy with a higher priority);
    - «New OAR mod» (inside the «SDS Anim Studio» OAR mod);
    - clips are taken with the studio's edits, and other config keys are kept.
- **SCAR** (the «SCAR» tab, the SCAR menu, zones in 3D):
  - **`SCAR_ActionData` keys:** all keys SCAR 2.0 reads; unknown keys are kept in order.
  - **Action types:** RA/RPA, LA/LPA, BA/BPA, DA/DPA, IDLE, each with its own colour.
  - **Top-view circle:**
    - each ActionData is a ring sector; click to select;
    - drag the white dots (distances), yellow dots (angles) and the green target.
  - **Target check** at the playhead with the NPC path and heading: in the zone, in the zone outside the time window, or ✗.
  - **List and editor:** ActionData list, full editor, add ActionData / `SCAR_ComboStart` at the playhead.
  - Not modelled: Chance rolls, cooldowns, ConditionsAlias, adding WeaponLength.
- **SCAR combo graph and simulator** (the «Combo graph» tab, SCAR menu):
  - **Moveset:** one game state of an OAR mod — the conditions of one of its submods without the `SCAR_AttackVariants` check. OAR conditions stack, so every submod whose conditions are a subset of the state belongs to it. «Nakrin Fase2Main» (keyword + phase-2 effect), for example, also runs the keyword-only variants V1–V4 and the fase1main clips that fase2main lacks (priority decides overlaps). The most general state holding the open clip is picked by default.
  - **Graph:**
    - nodes are clips, edges are `SCAR_ActionData`;
    - it is built from the `scar_*` dummy (or the open clip);
    - the action type gives the clip: RA → `bfco_attack{N}`, RPA → `bfco_powerattack{M}`, where N and M come from the current clip's `PIE.@SGVI|MCO_nextattack|N` / `MCO_nextpowerattack|M` (1 when unset);
    - `VariantID` → `SCAR_AttackVariants` → OAR picks the highest-priority passing submod: a variant submod only for its own V, a base submod for any;
    - edges are coloured by type and labelled with distance, angle, chance and variant;
    - actions that do not map to a clip (IDLE, LA, BA…) and clips missing from the moveset show as dangling arrows;
    - clips without ActionData are marked «chain end».
  - **The graph updates itself** when `SCAR_ActionData`, `SCAR_ComboStart` or `MCO_next…` change in the open clip (delete a transition and its edge goes); layout, focus and path are kept.
  - **Mouse:**
    - clicking a node opens the clip;
    - clicking an edge opens the source clip and selects that ActionData;
    - drag nodes, drag empty space to pan, wheel to zoom; «Layout» re-arranges the nodes.
  - **«Show only next combo attacks»** (+ «steps: N»): shows only the selected clip (click a node; without a click, the one open in the studio) and the clips it leads to within N transitions, laid out compactly — one column per step, ordered as SCAR tries the actions. Everything else is hidden.
    - **Path:** clicking a next attack moves the focus to it while the clips walked so far stay in a row on the left with the edges between them highlighted. Above the graph, «Path: dummy → attack1 → attack2»: click any step to go back to it; «⟲ clear path». In this mode the simulator walks the path itself; when a chain ends the path stays on screen and the next chain starts a new one.
  - **«Target» filter:** distance and angle (or «from scene») → transition odds on the edges; unreachable ones fade.
  - **SCAR choice:** the ActionData annotation time is its weight/priority. Actions are tried from the latest time down: the later one is checked first; if the target is in its zone it rolls Chance; the first success wins. A low chance keeps the priority and only lets the action be skipped.
  - **Simulator:**
    - «Start» from the dummy or the open clip; «Step» makes one SCAR decision;
    - «target from the NPC path» opens the clip in the studio, takes distance and angle at each `SCAR_ComboStart` with root motion and SDSCombat, and carries the target into the next clip;
    - otherwise the target stays fixed relative to the NPC and «×20» runs 20 steps at once;
    - roll Chance or pick by hand from the list with odds;
    - with no success or the target outside every zone, the clip plays out and the chain returns to the dummy;
    - a chain log (time, target, type, chance, variant, next clip) and clip frequencies; the taken edge and the current node are highlighted.
  - GUI-free check: `--tab combo --combo-sim <steps> [--combo-next] --target-dist .. --target-angle ..`.
  - **Not modelled:** cooldowns, ConditionsAlias, TriggerStart/EndTime, situational submods with other conditions (a phase change by health, say) form a moveset of their own.
- **Projects** (File menu):
  - **Files:** `.sdsproj` (JSON), by default in `dist\Projects`.
  - **Saved in a project:**
    - the clip and all annotations, unsaved edits included;
    - playhead, speed, selection;
    - NPC, weapon, hidden parts;
    - scene settings, camera, timeline zoom;
    - external OAR folders and the selected submod.
  - **Last session:** the studio saves it on close (`last_session.sdsproj`); restore it with «File → Restore last session».
- **PIE / FX:**
  - **Aliases:** `PIE.$Alias` is resolved through every `SKSE\PayloadInterpreter\Config\*.ini` of the build. Multi-line aliases and `![sec]` delays are supported.
  - **Commands:**
    - `@CAST` / `@CASTRIGHT` / `@CASTLEFT` → spell → effects → art:
      - casting art on the magic node;
      - hit art on the caster (cast on self) or on the target;
      - enchant art on `WEAPON`;
    - `@APPLYSPELL` keeps its art until `@UNAPPLYSPELL`;
    - `@PLAYPARTICLE|nif|body part|scale|time|flags|x|y|z` puts the NIF on a body part (33+ weapon → `WEAPON`, 9/38 shield, 3 head).
  - **PIE inspector:**
    - a direct `PIE.@COMMAND|…` is a typed form with every parameter named after the PayloadInterpreter README (effectiveness, magnitude, on self, HP/stamina/magicka requirement and cost, body part, scale…);
    - an alias shows each step with its delay and named parameters, then spell → art per effect → notes.
  - **Art NIF rendering:**
    - `AttachT` (`NamedNode&WEAPON`) hangs the art on that node;
    - target-mesh emitters (blade effects) emit from the weapon in hand;
    - BeginLoopEnd sequences (mBegin, then mLoop) drive emitters and visibility;
    - effect meshes animate colour, alpha, emissive and UV, node transforms and billboards;
    - greyscale-to-palette is applied; refraction meshes are skipped;
    - particles run through the Aether simulator (checked against SkyrimSE 1.5.97); strip systems are drawn as ribbons.
  - **Projectiles and explosions.** A non-self `@CAST` of a spell with a projectile (MGEF → PROJ) launches it from the hand's magic node (posed at the cast) at the target's chest:
    - Missile/Arrow are launched straight at the target and pulled down by the PROJ gravity (×9.81 m/s²): they burst wherever they first touch the target or the ground. A projectile with gravity 10000 drops at the caster's feet and explodes there;
    - Lobber flies on an arc aimed at the target;
    - Cone flies straight without gravity, passes the target and runs its range, widening: radius = CollisionRadius + distance flown × tan(Cone Spread);
    - Beam/Flame/Barrier hit at once;
    - when Range or lifetime runs out first, it vanishes without exploding;
    - on impact the explosion plays (the MGEF's EXPL, else the PROJ's own — only with its Explosion flag). The effect's hit art and hit sound play when the target was hit directly or is inside the explosion. With the path on, the trajectory is an orange line;
    - blast radius: the spell effect's Area, else the MGEF's area (both in feet, ×21.33 → 15 ft = 320 u), else the EXPL's own Radius. It shows as a red ring for 3 s after the blast;
    - impacts strike what was hit. The MGEF's set (DATA@100): on the target, the IPCT for skin (MaterialSkin); on the floor, the one for the surface chosen in Scene → Sound (or its parent). The explosion's set always marks the floor under the blast when the blast reaches down that far. The model follows the IPCT orientation (surface normal, projectile vector or its reflection) for the IPCT duration; the sound plays on impact;
    - hitboxes (Scene → «Projectile, explosion and hazard hitboxes»): the projectile's collision sphere in flight (CollisionRadius; a cone's grows with its spread, with the cone outlined from the launch point), the blast sphere for 0.5 s after it, the hazard's sphere while it lasts. Those touching the target are red;
    - hazard: when the explosion's PlacedObject is a HAZD, its model stays at the blast (on the floor with «drop to ground») for its Lifetime (10 s without one), with a yellow ring of its radius (feet ×21.33) and its sound looping until the end;
    - sounds: the PROJ flight loop until the impact, the EXPL sound and the MGEF hit sound on it;
    - the PIE inspector notes «projectile …, type, speed → explosion»; `SDS_FX_DEBUG=1` prints FXPROJ lines.
  - **AddOnNode.** `AddOnNode<N>` nodes in any art, projectile, explosion or impact get the model of the ADDN record with index N (lightning, sparks, dust — usually particle systems) and are born where the node is (with the projectile's flight and the node's animation); nested addons go two levels deep.
  - **World-space particles.** Particle systems with the World Space flag (nearly every Skyrim effect) leave their particles where they were born: a flying projectile or a moving hand trails them instead of carrying the cloud along. The art's placement is taken at each particle's birth, so a playhead jump still lays the trail along the whole path. An unknown index is logged.
  - **Effect shaders on the body (EFSH).** The MGEF hit shader covers the NPC's body on a self `@CAST` (for the effect's duration, or by its own envelopes for instant effects) and on `@APPLYSPELL` (until `@UNAPPLYSPELL`, then it fades out):
    - fill — the fill texture with UV scroll and scale, coloured by its three keys; with the grayscale flag the texture's brightness is looked up in the membrane palette;
    - edge — a fresnel glow in the edge colour, its width from the falloff;
    - fill and edge alpha follow the EFSH envelopes (fade in, full, fade to persistent, pulse);
    - EFSH particles: sprites born at random points of the body (ICO2, U×V atlas, NAM9 palette) fly along the normal plus their initial velocity and acceleration; size, colour and alpha follow the life keys; the rate ramps up, holds, ramps down to the persistent level. The rate is taken per 5000 square units of body surface — an estimate, the engine's unit is not known;
    - «affect skin only» keeps it on skin; shaders on the target are not drawn (it has no body); weapon enchantment shaders are not drawn.
  - Not modelled: mEnd, bow arrows (BowRelease), projectile collisions with anything but the target and the ground.
  - Debug: `SDS_FX_DEBUG=1` prints the meshes, systems and clips of each art NIF.
- **Sound** (🔊 and the volume slider in the timeline bar; saved in settings):
  - **What plays:** every event the playhead passes during playback:
    - `SoundPlay.<EDID>` (SNDR, or SOUN → its SNDR);
    - PIE effect sounds: MGEF «release» on `@CAST`, plus «hit» when cast on self;
    - `weaponSwing` / `weaponLeftSwing`: the NPC weapon's swing sound (WEAP TNAM), unarmed — `WPNSwingUnarmed`;
    - `Foot*` (FootLeft, FootScuffRight…): the NPC footwear's footstep set (feet ARMA → FSTS; no NPC — FSTBarefootFootstepSet) → the Footstep with that tag → impact set → the impact for the chosen ground (or its parent material) → sound. The ground is set in «Scene» → «Sound» (MaterialStone by default).
  - **Inspector for swing/footstep events:** the record chain and a «▶ Listen» button.
  - **As in game:**
    - a random ANAM file;
    - static attenuation and dB variance;
    - pitch shift and variance;
    - the SNCT category volume chain;
    - looping sounds;
    - `.wav` falls back to `.xwm`, then `.fuz`.
  - **Output:** Windows' XAudio2 2.9 plays PCM/ADPCM `.wav`, xWMA `.xwm` and `.fuz` with no third-party decoders. Timeline speed changes the pitch too.
  - **Playback control:** pause silences; seeking and playhead jumps are silent; at the end of a non-looping clip the tails ring out.
  - **Inspector:** a `SoundPlay` annotation shows SNDR, category, attenuation, pitch and files, with a «▶ Listen» button.
  - **GUI-free check:** `sds-anim-studio.exe --sound-test <build paths>…` (nothing plays).
- **Precision trails** (switch and summary in «Scene»):
  - **The ribbon:** it follows the WEAPON/SHIELD blade with the MCM lifetime, fade and segment rate.
  - **Length:** the weapon mesh, or the collision with `TrailUseTrueLength`.
  - **Look, by priority:**
    - annotation parameters;
    - `[[TrailDefinition]]` from `SKSE\Plugins\Precision\*.toml`, matched to the NPC weapon as Precision does;
    - the MCM default.
- **Scene:** toggles for root motion, path, grid and timeline rows.
- **Saving HKX** (Ctrl+S):
  - **Write:** a copy of the clip with the new annotations goes to the output folder with the same path from Data; the source mod is never touched.
  - **Verification:** the file is re-read and compared before writing. All classes and animation data must match and the annotations must be exactly the given ones, otherwise nothing is written.
  - **Output folder over the build:** if the output folder already holds a saved copy of the clip, the studio opens that copy (source «output folder (over <mod>)»), even when the output folder is not enabled in MO2. Saved edits show when the clip is reopened.
  - **Drafts:** switching to another clip keeps unsaved edits aside; they come back when the clip is opened again (until the studio closes), as one Ctrl+Z step.
  - **Focus handling:** Ctrl+S and clip switches wait two frames, so a field still in focus (annotation text, a number such as `arrive`) applies its input first.
- **Export** of `anno.txt` in hkanno64 format: File menu.

## Keys

| Key | Action |
|---|---|
| Space | play / pause |
| ← / → | frame back / forward |
| Home / End | start / end |
| Del | delete the selected annotation |
| Ctrl+Z / Ctrl+Y | undo / redo |
| Ctrl+C / Ctrl+V | copy / paste at playhead |
| Ctrl+S | save HKX to the output folder |
| wheel over the timeline | time zoom (about the pointer) |
| Shift+wheel | scroll in time |
| Ctrl+wheel | scroll the timeline rows |

## Building from source

```
git clone <repository url>
cd <repository folder>
cargo build --release -p sds-anim-studio
cargo test
```

Rust stable (tested on x86_64-pc-windows-gnu; msvc works too). Everything needed is in the repository: the Aether crates (`vendor/aether`: nif, core, codecs; GPL-3.0) and SARDONYX's serde-hkx (`vendor/serde-hkx`, MIT/Apache-2.0), without their test data. The exe lands in `target\release\sds-anim-studio.exe`. On first start pick the MO2 folder and profile (or a Data folder) in the Library tab; `sds_anim_studio.json` and an `Output` folder appear next to the exe.

**UI check without manual runs:**

```
sds-anim-studio.exe --screenshot out.png --open "<Data path>" --time 0.75 --select 3
```

Optional flags:
- `--npc <EditorID>` dresses the character;
- `--tab npc|scene|anno|inspector|oar|scar|combo` opens a tab;
- `--project <file>` opens a project, `--save-project <file>` saves one;
- `--open` also takes a disk path;
- `--select-text <substring>` selects an annotation;
- `--target-dist` and `--target-angle` place the target;
- `--dist` and `--yaw` set the camera, `--skeleton` draws the skeleton, `--oar` switches the library to OAR mode;
- `--lang en` sets the language;
- `--save` adds a test annotation and saves the HKX.

## Structure

| Crate | Purpose |
|---|---|
| `crates/vfs` | MO2 → virtual Data (loose + BSA via Aether) |
| `crates/hkx` | HKX (packfile 2010.2): clip info and annotations, skeleton and spline/interleaved decoding, annotation writing with verification |
| `crates/records` | load-order record index, NPC appearance, spell art and weapon facts, sounds (SNDR/SOUN/SNCT, MGEF SNDD) |
| `crates/i18n` | UI language: `t!("рус", "eng")`, `tf!` for formatting, `tr!` for const tables |
| `crates/anno` | annotation parsing, start/end pairs, Precision rules, the SDSCombat path |
| `crates/app` | egui + egui_dock UI: library, wgpu render, character, Precision, SDSCombat path, OAR, SCAR, PIE/FX, sound (XAudio2), projects, timeline, inspector, scene, log |
