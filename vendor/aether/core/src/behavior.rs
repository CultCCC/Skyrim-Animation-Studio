//! The autoplay behavior model and its evaluator, std-only.
//!
//! A BeginLoopEnd-style behavior graph is a flat state machine: each state
//! wraps one `BGSGamebryoSequenceGenerator` that plays a named
//! NiControllerSequence, and transitions fire on events. The `end` event is
//! raised by the sequence's own `end` text key; external events
//! (`WeaponDraw`, `attackStart`, ...) come from the game — or from preview
//! buttons. Semantics verified against SkyrimSE 1.5.97 in
//! `docs/knowledge/behavior-hkx.md`: sequence lookup is case-insensitive,
//! transitions hard-cut (Zodiac's `ZeroDuration`), text keys that match no
//! event name are silently ignored, and `Event.Payload` keys match on the
//! prefix before the first dot.
//!
//! The model is produced by `crates/behavior` (the one crate allowed to
//! carry the Havok dependency) and consumed here without any of it.

/// One state: plays `sequence` until a transition fires.
#[derive(Debug, Clone, PartialEq)]
pub struct AutoplayState {
    /// Explicit state id — real graphs use sparse ids (0/2/4/6/8).
    pub id: i32,
    pub name: String,
    /// NiControllerSequence name, matched case-insensitively.
    pub sequence: String,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AutoplayTransition {
    /// Source state id, or -1 for a wildcard (any state).
    pub from: i32,
    /// Index into `AutoplayGraph::events`.
    pub event: usize,
    pub to: i32,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct AutoplayGraph {
    pub name: String,
    /// Event names; index = event id.
    pub events: Vec<String>,
    pub states: Vec<AutoplayState>,
    pub transitions: Vec<AutoplayTransition>,
    pub start_state: i32,
}

impl AutoplayGraph {
    pub fn state(&self, id: i32) -> Option<&AutoplayState> {
        self.states.iter().find(|s| s.id == id)
    }

    /// Case-INSENSITIVE, because the game's is. The trigger lookup
    /// (1.5.97 `0x1404EA500`) compares interned `BSFixedString` pointers,
    /// and the pool interns through `_stricmp` (`0x140C2A603`), so `end`
    /// and `End` are one string. Vanilla depends on it: the stock
    /// `genericbehaviors\autoplay.hkx` names its event `End` while the
    /// 176 NIFs that bind it write the key `end`.
    pub fn event_id(&self, name: &str) -> Option<usize> {
        self.events.iter().position(|e| e.eq_ignore_ascii_case(name))
    }

    /// Match a sequence text key against the event table, the way the game
    /// registers triggers: the whole key first, then the prefix before the
    /// first `.` (the `Event.Payload` convention). Anything else — `start`,
    /// exporter garbage — matches nothing and is ignored.
    pub fn event_for_text_key(&self, key: &str) -> Option<usize> {
        if let Some(id) = self.event_id(key) {
            return Some(id);
        }
        let prefix = key.split('.').next().unwrap_or(key);
        if prefix != key {
            return self.event_id(prefix);
        }
        None
    }

    /// The transition taken from `from` on `event`, state-specific rules
    /// first, wildcards second.
    pub fn transition(&self, from: i32, event: usize) -> Option<i32> {
        self.transitions
            .iter()
            .find(|t| t.from == from && t.event == event)
            .or_else(|| {
                self.transitions
                    .iter()
                    .find(|t| t.from == -1 && t.event == event)
            })
            .map(|t| t.to)
    }

    /// External events worth showing as preview buttons: everything that
    /// appears in a transition and is not the sequence-driven `end`.
    pub fn external_events(&self) -> Vec<usize> {
        let mut ids: Vec<usize> = self
            .transitions
            .iter()
            .map(|t| t.event)
            .filter(|&e| !self.events.get(e).is_some_and(|n| n.eq_ignore_ascii_case("end")))
            .collect();
        ids.sort_unstable();
        ids.dedup();
        ids
    }
}

/// One clip a behavior graph plays, and what it needs from the NIF.
#[derive(Debug, Clone, PartialEq)]
pub struct ClipRequirement {
    /// The NiControllerSequence name, matched case-insensitively.
    pub sequence: String,
    /// The state leaves only on an `end` key the clip itself carries.
    pub needs_end_key: bool,
}

/// What choosing a graph asks of a file.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BehaviorContract {
    pub clips: Vec<ClipRequirement>,
    /// Events the GAME sends (StopEffect, ...), not raised by a clip.
    pub game_events: Vec<String>,
}

impl AutoplayGraph {
    /// What this graph asks of the NIF it is bound to.
    ///
    /// Clips come in the order the machine REACHES them from its start
    /// state -- the first one listed is what plays on load -- then any
    /// state no transition leads to. A clip "needs an end key" when its
    /// state leaves on `end`: that key is an ordinary text key the clip
    /// itself must carry, and without it the graph stays in that state
    /// forever. Every other event on a transition is one the game sends.
    pub fn contract(&self) -> BehaviorContract {
        let is_end = |e: usize| self.events.get(e).is_some_and(|n| n.eq_ignore_ascii_case("end"));
        let mut order: Vec<i32> = Vec::new();
        let mut queue = std::collections::VecDeque::from([self.start_state]);
        while let Some(id) = queue.pop_front() {
            if order.contains(&id) || self.state(id).is_none() {
                continue;
            }
            order.push(id);
            for t in self.transitions.iter().filter(|t| t.from == id || t.from == -1) {
                queue.push_back(t.to);
            }
        }
        for s in &self.states {
            if !order.contains(&s.id) {
                order.push(s.id);
            }
        }
        let mut clips: Vec<ClipRequirement> = Vec::new();
        for id in order {
            let state = self.state(id).expect("ordered ids are states");
            if state.sequence.is_empty()
                || clips.iter().any(|c| c.sequence.eq_ignore_ascii_case(&state.sequence))
            {
                continue;
            }
            clips.push(ClipRequirement {
                sequence: state.sequence.clone(),
                needs_end_key: self.transitions.iter().any(|t| t.from == id && is_end(t.event)),
            });
        }
        let mut game_events: Vec<String> = Vec::new();
        for t in &self.transitions {
            let name = &self.events[t.event];
            if !is_end(t.event) && !game_events.contains(name) {
                game_events.push(name.clone());
            }
        }
        BehaviorContract { clips, game_events }
    }
}

/// Build a graph from names, the way the stock table is written:
/// transitions name their event rather than index it, so an entry reads
/// like the census that decoded it.
fn stock(
    name: &str,
    start_state: i32,
    events: &[&str],
    states: &[(i32, &str, &str)],
    transitions: &[(i32, &str, i32)],
) -> AutoplayGraph {
    let events: Vec<String> = events.iter().map(|e| e.to_string()).collect();
    AutoplayGraph {
        name: name.into(),
        states: states
            .iter()
            .map(|&(id, n, seq)| AutoplayState {
                id,
                name: n.into(),
                sequence: seq.into(),
            })
            .collect(),
        transitions: transitions
            .iter()
            .map(|&(from, e, to)| AutoplayTransition {
                from,
                event: events.iter().position(|x| x == e).expect("stock event"),
                to,
            })
            .collect(),
        events,
        start_state,
    }
}

/// A behavior graph that ships with the game, as decoded from its files.
pub struct StockGraph {
    /// The BGED project path, relative to `meshes\`.
    pub path: &'static str,
    /// One line for the picker.
    pub summary: &'static str,
    pub build: fn() -> AutoplayGraph,
}

/// The stock graphs the corpus's particle files bind, largest first
/// (NIFs with particles that use each, over the whole configured corpus:
/// 137, 116, 83, 44, 22, 28, 10). Written out by hand because core
/// cannot parse Havok; `crates/behavior/tests/stock_graphs.rs` decodes
/// every entry from the game's own files and fails on any difference.
///
/// `magic\castingbasic.hkx` (41) is deliberately absent: its hand-cast
/// machine chooses clips with a manual selector and expression
/// conditions, which this flat model cannot state honestly.
pub const STOCK_GRAPHS: &[StockGraph] = &[
    StockGraph {
        path: r"Magic\BeginLoopEnd.hkx",
        summary: "Spell / enchantment effect: mBegin once, mLoop while it lasts, mEnd when it ends",
        build: || {
            stock(
                "BeginLoopEndBehavior.hkb",
                4,
                &["StopEffect", "end", "EndAnim", "SoundPlay", "RestartEffect"],
                &[(0, "mLoopState", "mLoop"), (2, "mEndState", "mEnd"), (4, "mBeginState", "mBegin")],
                &[(0, "StopEffect", 2), (2, "RestartEffect", 4), (4, "end", 0)],
            )
        },
    },
    StockGraph {
        path: r"GenericBehaviors\Autoplay.hkx",
        summary: "Ambient effect: AutoPlay once, AutoLoop forever, AutoFadeOut when stopped",
        build: || {
            stock(
                "AutoplayBehavior.hkb",
                0,
                &["End", "StopEffect", "AutoOneOff", "Reset", "to AutoplayState", "AutoReset"],
                &[
                    (0, "AutoplayState", "AutoPlay"),
                    (1, "AutoLoopState", "AutoLoop"),
                    (2, "AutoFadeOutState", "AutoFadeOut"),
                    (3, "AutoOneOffState", "AutoOneOff"),
                    (4, "AutoOneOffLoopState", "AutoOneOffLoop"),
                ],
                &[
                    (0, "End", 1),
                    (1, "StopEffect", 2),
                    (3, "Reset", 0),
                    (3, "End", 4),
                    (4, "Reset", 0),
                    (-1, "StopEffect", 2),
                    (-1, "AutoOneOff", 3),
                    (-1, "AutoReset", 0),
                ],
            )
        },
    },
    StockGraph {
        path: r"Magic\IdleOnLoad.hkx",
        summary: "One clip, mIdle, from the moment the object loads",
        build: || {
            stock(
                "IdleOnLoadBehavior.hkb",
                0,
                &["EndAnim", "PlaceActor", "StopEffect", "SoundPlay", "end", "Imod", "Rimod"],
                &[(0, "mIdleState", "mIdle")],
                &[(0, "StopEffect", 0)],
            )
        },
    },
    StockGraph {
        path: r"GenericBehaviors\WaitPlayIdleAway\WaitPlayIdleAway.hkx",
        summary: "Scripted object: Wait until Play, then Idle until Away",
        build: || {
            stock(
                "waitPlayIdleAwayBehavior.hkb",
                1,
                &[
                    "WaitState_to_PlayState",
                    "End",
                    "PlayState_to_PlayState00",
                    "Play",
                    "IdleState_to_AwayState",
                    "Away",
                    "AwayState_to_WaitState",
                ],
                &[
                    (4, "AwayState", "Away"),
                    (1, "WaitState", "Wait"),
                    (2, "PlayState", "Play"),
                    (3, "IdleState", "Idle"),
                ],
                &[(4, "End", 1), (1, "Play", 2), (2, "End", 3), (3, "Away", 4)],
            )
        },
    },
    StockGraph {
        path: r"GenericBehaviors\IdlePlayIdle.hkx",
        summary: "Scripted object: AnimIdle, AnimPlay on PlayAnim01, back to idle",
        build: || {
            stock(
                "IdlePlayIdleBehavior.hkb",
                0,
                &["End", "StopEffect", "EndAnim", "PlayAnim01", "SoundPlay", "Imod", "RImod"],
                &[(0, "IdleState", "AnimIdle"), (1, "PlayAnimState", "AnimPlay")],
                &[(0, "PlayAnim01", 1), (1, "End", 0)],
            )
        },
    },
    StockGraph {
        path: r"GenericBehaviors\IdleTransIdleBack.hkx",
        summary: "Scripted object: two idles with a transition clip each way",
        build: || {
            stock(
                "IdleTransIdleBackBehavior.hkb",
                0,
                &[
                    "End",
                    "StopEffect",
                    "EndAnim",
                    "PlayAnim01",
                    "SoundPlay",
                    "Imod",
                    "RImod",
                    "PlayAnim02",
                    "TransitionAnim",
                    "SoundStop",
                    "SoundPlayAt",
                ],
                &[
                    (0, "Idle01State", "AnimIdle01"),
                    (1, "Transition01State", "AnimTrans01"),
                    (2, "Idle02State", "AnimIdle02"),
                    (3, "Transition02State", "AnimTrans02"),
                ],
                &[
                    (0, "PlayAnim02", 1),
                    (0, "TransitionAnim", 1),
                    (1, "End", 2),
                    (1, "PlayAnim01", 3),
                    (1, "TransitionAnim", 3),
                    (2, "PlayAnim01", 3),
                    (2, "TransitionAnim", 3),
                    (3, "End", 0),
                    (3, "PlayAnim02", 1),
                    (3, "TransitionAnim", 1),
                ],
            )
        },
    },
    StockGraph {
        path: r"GenericBehaviors\StagesNoLoops\StagesNoLoops.hkx",
        summary: "Scripted object: stage1, stage2, stage3, each on its own event",
        build: || {
            stock(
                "Behavior.hkb",
                0,
                &["stage1State_to_stage2State", "stage2", "stage2State_to_stage3State", "stage3"],
                &[(0, "stage1State", "stage1"), (1, "stage2State", "stage2"), (2, "stage3State", "stage3")],
                &[(0, "stage2", 1), (1, "stage3", 2)],
            )
        },
    },
];

/// Look a BGED path up in the stock table, case-insensitively and with
/// either separator -- the string pool folds case, and paths are
/// written both ways.
pub fn stock_graph(path: &str) -> Option<&'static StockGraph> {
    let want = path.trim().replace('/', "\\");
    STOCK_GRAPHS.iter().find(|s| s.path.eq_ignore_ascii_case(&want))
}

/// The running evaluator: which state is active now.
#[derive(Debug, Clone, PartialEq)]
pub struct AutoplayPlayer {
    pub current: i32,
}

impl AutoplayPlayer {
    pub fn new(graph: &AutoplayGraph) -> AutoplayPlayer {
        AutoplayPlayer {
            current: graph.start_state,
        }
    }

    /// The sequence the current state wants playing.
    pub fn sequence<'g>(&self, graph: &'g AutoplayGraph) -> Option<&'g str> {
        graph.state(self.current).map(|s| s.sequence.as_str())
    }

    /// Feed an event by id; returns true when the state changed (the caller
    /// hard-cuts to the new state's sequence).
    pub fn handle_event(&mut self, graph: &AutoplayGraph, event: usize) -> bool {
        match graph.transition(self.current, event) {
            Some(to) if graph.state(to).is_some() => {
                self.current = to;
                true
            }
            _ => false,
        }
    }

    /// Feed a sequence text key (the `[prev, cur)` trigger window is the
    /// caller's business — this consumes keys the caller decided fired).
    pub fn handle_text_key(&mut self, graph: &AutoplayGraph, key: &str) -> bool {
        match graph.event_for_text_key(key) {
            Some(event) => self.handle_event(graph, event),
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Zodiac graph exactly as recovered from autoplaybehavior.hkx
    /// (docs/knowledge/behavior-hkx.md §1).
    fn zodiac() -> AutoplayGraph {
        let events: Vec<String> = [
            "WeaponDraw",
            "WeaponSheathe",
            "end",
            "attackPowerStart_SprintLeftHand",
            "attackPowerStartForwardLeftHand",
            "attackPowerStartRightLeftHand",
            "attackPowerStartLeftLeftHand",
            "attackPowerStartBackLeftHand",
            "attackPowerStartInPlaceLeftHand",
            "attackPowerStart_Sprint",
            "attackPowerStartForward",
            "attackPowerStartRight",
            "attackPowerStartLeft",
            "attackPowerStartBack",
            "attackPowerStartInPlace",
            "attackPowerStartDualWield",
            "EndAnim",
            "attackStart",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let state = |id, name: &str, seq: &str| AutoplayState {
            id,
            name: name.into(),
            sequence: seq.into(),
        };
        let mut transitions = vec![
            AutoplayTransition { from: 0, event: 0, to: 2 },
            AutoplayTransition { from: 2, event: 2, to: 4 },
            AutoplayTransition { from: 2, event: 1, to: 8 },
            AutoplayTransition { from: 4, event: 17, to: 6 },
            AutoplayTransition { from: 4, event: 1, to: 8 },
            AutoplayTransition { from: 6, event: 2, to: 4 },
            AutoplayTransition { from: 6, event: 1, to: 8 },
            AutoplayTransition { from: 8, event: 2, to: 0 },
            AutoplayTransition { from: 8, event: 0, to: 2 },
        ];
        for power in 3..=15 {
            transitions.push(AutoplayTransition { from: 4, event: power, to: 6 });
        }
        AutoplayGraph {
            name: "BeginLoopEndBehavior.hkb".into(),
            events,
            states: vec![
                state(0, "IdleState", "Idle"),
                state(2, "drawState", "draw"),
                state(4, "KeepState", "Keep"),
                state(6, "swingState", "swing"),
                state(8, "SheatheState", "Sheathe"),
            ],
            transitions,
            start_state: 0,
        }
    }

    #[test]
    fn the_zodiac_cycle_walks_begin_loop_end() {
        let g = zodiac();
        let mut p = AutoplayPlayer::new(&g);
        assert_eq!(p.sequence(&g), Some("Idle"));

        // Draw the weapon, sequence plays through, "end" advances to Keep.
        assert!(p.handle_text_key(&g, "WeaponDraw"));
        assert_eq!(p.sequence(&g), Some("draw"));
        assert!(p.handle_text_key(&g, "end"));
        assert_eq!(p.sequence(&g), Some("Keep"));

        // Attack swings and returns to the loop.
        assert!(p.handle_text_key(&g, "attackStart"));
        assert_eq!(p.sequence(&g), Some("swing"));
        assert!(p.handle_text_key(&g, "end"));
        assert_eq!(p.sequence(&g), Some("Keep"));

        // Sheathe winds down to idle.
        assert!(p.handle_text_key(&g, "WeaponSheathe"));
        assert_eq!(p.sequence(&g), Some("Sheathe"));
        assert!(p.handle_text_key(&g, "end"));
        assert_eq!(p.sequence(&g), Some("Idle"));
    }

    #[test]
    fn unknown_text_keys_are_ignored_like_the_game_ignores_them() {
        let g = zodiac();
        let mut p = AutoplayPlayer::new(&g);
        // The keys real sequences carry that match no event.
        assert!(!p.handle_text_key(&g, "start"));
        assert!(!p.handle_text_key(
            &g,
            " -GlobalRatio 100 -GlobalCompressFloats true -GlobalDontCompress false"
        ));
        assert_eq!(p.current, 0, "state must not move");
        // But the Event.Payload convention matches on the prefix.
        assert_eq!(g.event_for_text_key("WeaponDraw.Something"), Some(0));
    }

    #[test]
    fn events_without_a_transition_in_this_state_do_nothing() {
        let g = zodiac();
        let mut p = AutoplayPlayer::new(&g);
        // attackStart only means something in the Keep state.
        assert!(!p.handle_text_key(&g, "attackStart"));
        assert_eq!(p.sequence(&g), Some("Idle"));
    }

    /// The stock `genericbehaviors\autoplay.hkx` graph, as decoded by
    /// `crates/behavior/tests/graph_census.rs`: its event is `End`, the
    /// 176 NIFs that bind it carry the text key `end`, and in game they
    /// advance -- the string pool interns case-insensitively. An exact
    /// match left the preview in `AutoPlay` forever.
    #[test]
    fn a_text_key_matches_its_event_whatever_the_case() {
        let events: Vec<String> = ["End", "StopEffect", "AutoOneOff", "Reset"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let state = |id, seq: &str| AutoplayState {
            id,
            name: format!("{seq}State"),
            sequence: seq.into(),
        };
        let g = AutoplayGraph {
            name: "AutoplayBehavior.hkb".into(),
            events,
            states: vec![state(0, "AutoPlay"), state(1, "AutoLoop"), state(2, "AutoFadeOut")],
            transitions: vec![
                AutoplayTransition { from: 0, event: 0, to: 1 },
                AutoplayTransition { from: 1, event: 1, to: 2 },
                AutoplayTransition { from: -1, event: 1, to: 2 },
            ],
            start_state: 0,
        };
        let mut p = AutoplayPlayer::new(&g);
        assert!(p.handle_text_key(&g, "end"), "the NIF's `end` must fire the graph's `End`");
        assert_eq!(p.sequence(&g), Some("AutoLoop"));
        assert!(p.handle_text_key(&g, "stopeffect"));
        assert_eq!(p.sequence(&g), Some("AutoFadeOut"));
        // `End` is raised by the clip, not by the game: not a button.
        assert_eq!(g.external_events(), vec![1]);
    }

    #[test]
    fn external_events_exclude_the_sequence_driven_end() {
        let g = zodiac();
        let ext = g.external_events();
        assert!(!ext.contains(&2), "end is not a button");
        assert!(ext.contains(&0) && ext.contains(&1) && ext.contains(&17));
    }
}
