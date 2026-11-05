//! `C5FM`: the idle state machine of a unit model, compiled from a `.fsmxml`.
//!
//! Version 2 adds the transitions with their conditions, in the order of the file. Version 1 had
//! states with `next` weights only (it was never written by a converter in this repository; its
//! layout here is v2 without `transitions`, as AB-D1 says); it is read and converted to v2 with
//! empty `transitions`.
//!
//! What a state plays is not in this blob: the program of the graph named by `asset` is
//! `world-anim`'s (its WA-D5); what the conditions mean exactly is RW-13.

use serde::{Deserialize, Serialize};

use crate::{FormatError, blob};

pub const FSM_MAGIC: [u8; 4] = *b"C5FM";

/// One condition of a transition, as the file gives it. The `order` attribute of the file is not
/// kept: the conditions are in the order they have in the file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum FsmCondition {
    /// `FFSMConditionCheckPathOption`: compares the path option `path` with `value` by `method`.
    CheckPathOption { method: u8, path: u16, value: i32 },
    /// `FFSMConditionEventSetPathOption`: sets the path option `path` to `value` on the event.
    EventSetPathOption { path: u16, value: i32 },
    /// `FFSMConditionEventRandomPathOption`: sets the path option `path` to a random value; the
    /// `options` are `(value, weight)` in file order, used when `use_weights`; `value` is the
    /// attribute of the same name.
    EventRandomPathOption {
        path: u16,
        value: i32,
        use_weights: bool,
        options: Vec<(i32, f32)>,
    },
    /// `FFSMConditionRandomRoll`.
    RandomRoll { value: f32, invert: bool },
    /// `FFSMConditionCheckStateTime`: the time in the state against `value` seconds by `method`.
    CheckStateTime { method: u8, value: f32 },
    /// `FFSMConditionECComplete`: the clip of event code `ec` is done (`value`: the fraction of
    /// it, when the file gives one).
    EcComplete { ec: u32, value: Option<f32> },
    /// `FFSMConditionOrOperator`: a marker at its place in the list; it has no operands in the
    /// file (the groups it separates are for the reader, RW-13).
    Or,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Transition {
    /// Index in [`IdleFsm::states`].
    pub to: u16,
    pub conditions: Vec<FsmCondition>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FsmState {
    pub name: String,
    /// Code of the action whose request enters the state (path option 4); 0 for none. It is not
    /// an event code of a clip: `IDLE_LOOP` and `FIDGETS_GRAPH` are both 1000.
    pub action: u32,
    /// Name of the graph (`.dge`) of the state; empty for a dummy state.
    pub asset: String,
    /// Weighted targets as in v1: `(state, weight)`.
    pub next: Vec<(u16, f32)>,
    /// Transitions with conditions, in the order of the file.
    pub transitions: Vec<Transition>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct IdleFsm {
    /// Name of the `.fsmxml` without extension.
    pub name: String,
    /// Index of the state the machine starts in.
    pub default: u16,
    pub states: Vec<FsmState>,
}

// ---------------------------------------------------------------- version 1

#[derive(Deserialize)]
struct FsmStateV1 {
    name: String,
    action: u32,
    asset: String,
    next: Vec<(u16, f32)>,
}

#[derive(Deserialize)]
struct IdleFsmV1 {
    name: String,
    default: u16,
    states: Vec<FsmStateV1>,
}

impl From<IdleFsmV1> for IdleFsm {
    fn from(v: IdleFsmV1) -> Self {
        IdleFsm {
            name: v.name,
            default: v.default,
            states: v
                .states
                .into_iter()
                .map(|s| FsmState {
                    name: s.name,
                    action: s.action,
                    asset: s.asset,
                    next: s.next,
                    transitions: Vec::new(),
                })
                .collect(),
        }
    }
}

impl IdleFsm {
    pub const VERSION: u16 = 2;
    pub const MIN_VERSION: u16 = 1;

    pub fn encode(&self) -> Vec<u8> {
        blob::encode_v(FSM_MAGIC, Self::VERSION, self)
    }

    /// Reads v1 and v2; a v1 machine comes back with empty `transitions`.
    pub fn decode(bytes: &[u8]) -> Result<Self, FormatError> {
        let (version, body) =
            blob::split_range(FSM_MAGIC, Self::MIN_VERSION, Self::VERSION, bytes)?;
        let fsm: IdleFsm = if version == 1 {
            blob::payload::<IdleFsmV1>(body)?.into()
        } else {
            blob::payload(body)?
        };
        fsm.validate()?;
        Ok(fsm)
    }

    /// Indices, weights and numbers a reader indexes by or divides with.
    pub fn validate(&self) -> Result<(), FormatError> {
        let n = self.states.len();
        if n > 0 && usize::from(self.default) >= n {
            return Err(FormatError::invalid(format!(
                "default state {} of {n}",
                self.default
            )));
        }
        for (i, s) in self.states.iter().enumerate() {
            for (to, w) in &s.next {
                if usize::from(*to) >= n {
                    return Err(FormatError::invalid(format!(
                        "state {i} ({}): next {to} of {n}",
                        s.name
                    )));
                }
                if !w.is_finite() || *w < 0.0 {
                    return Err(FormatError::invalid(format!(
                        "state {i} ({}): a weight is not finite or is negative",
                        s.name
                    )));
                }
            }
            for t in &s.transitions {
                if usize::from(t.to) >= n {
                    return Err(FormatError::invalid(format!(
                        "state {i} ({}): transition to {} of {n}",
                        s.name, t.to
                    )));
                }
                for c in &t.conditions {
                    check_condition(c).map_err(|why| {
                        FormatError::invalid(format!("state {i} ({}): {why}", s.name))
                    })?;
                }
            }
        }
        Ok(())
    }

    /// Index of the first state named `name`.
    pub fn state(&self, name: &str) -> Option<usize> {
        self.states.iter().position(|s| s.name == name)
    }
}

fn check_condition(c: &FsmCondition) -> Result<(), &'static str> {
    match c {
        FsmCondition::EventRandomPathOption { options, .. }
            if options.iter().any(|(_, w)| !w.is_finite() || *w < 0.0) =>
        {
            Err("a random weight is not finite or is negative")
        }
        FsmCondition::RandomRoll { value, .. } | FsmCondition::CheckStateTime { value, .. }
            if !value.is_finite() =>
        {
            Err("a condition value is not finite")
        }
        FsmCondition::EcComplete { value: Some(v), .. } if !v.is_finite() => {
            Err("a condition value is not finite")
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn machine() -> IdleFsm {
        IdleFsm {
            name: "m".into(),
            default: 1,
            states: vec![
                FsmState {
                    name: "IDLE_LOOP".into(),
                    action: 1000,
                    asset: "IDLE_GRAPH.dge".into(),
                    next: vec![],
                    transitions: vec![],
                },
                FsmState {
                    name: "CHOOSER".into(),
                    action: 0,
                    asset: String::new(),
                    next: vec![(0, 0.4), (2, 0.6)],
                    transitions: vec![
                        Transition {
                            to: 0,
                            conditions: vec![
                                FsmCondition::EventRandomPathOption {
                                    path: 0,
                                    value: 0,
                                    use_weights: true,
                                    options: vec![(1, 0.4), (0, 0.6)],
                                },
                                FsmCondition::CheckPathOption {
                                    method: 6,
                                    path: 4,
                                    value: 1500,
                                },
                                FsmCondition::Or,
                                FsmCondition::EcComplete {
                                    ec: 1040,
                                    value: Some(0.98),
                                },
                            ],
                        },
                        Transition {
                            to: 2,
                            conditions: vec![
                                FsmCondition::RandomRoll {
                                    value: 0.0007,
                                    invert: false,
                                },
                                FsmCondition::CheckStateTime {
                                    method: 5,
                                    value: 0.15,
                                },
                                FsmCondition::EventSetPathOption { path: 2, value: 0 },
                            ],
                        },
                    ],
                },
                FsmState {
                    name: "FORTIFY_LOOP".into(),
                    action: 1500,
                    asset: "FORTIFY_GRAPH.dge".into(),
                    next: vec![],
                    transitions: vec![],
                },
            ],
        }
    }

    #[test]
    fn roundtrip_is_deterministic() {
        let m = machine();
        let b = m.encode();
        assert_eq!(b, m.encode());
        assert_eq!(IdleFsm::decode(&b).unwrap(), m);
        assert_eq!(m.state("FORTIFY_LOOP"), Some(2));
        assert_eq!(m.state("nope"), None);
    }

    /// The v1 layout as the spec describes it: v2 without `transitions`.
    fn v1_bytes(m: &IdleFsm) -> Vec<u8> {
        #[derive(Serialize)]
        struct S1<'a> {
            name: &'a str,
            action: u32,
            asset: &'a str,
            next: &'a [(u16, f32)],
        }
        #[derive(Serialize)]
        struct M1<'a> {
            name: &'a str,
            default: u16,
            states: Vec<S1<'a>>,
        }
        let v = M1 {
            name: &m.name,
            default: m.default,
            states: m
                .states
                .iter()
                .map(|s| S1 {
                    name: &s.name,
                    action: s.action,
                    asset: &s.asset,
                    next: &s.next,
                })
                .collect(),
        };
        blob::encode_v(FSM_MAGIC, 1, &v)
    }

    #[test]
    fn v1_is_read_and_becomes_v2_without_transitions() {
        let m = machine();
        let got = IdleFsm::decode(&v1_bytes(&m)).unwrap();
        assert_eq!(got.default, 1);
        assert_eq!(got.states.len(), 3);
        assert_eq!(got.states[1].next, vec![(0, 0.4), (2, 0.6)]);
        assert_eq!(got.states[2].action, 1500);
        assert!(got.states.iter().all(|s| s.transitions.is_empty()));
        // Written again, it is v2.
        let again = IdleFsm::decode(&got.encode()).unwrap();
        assert_eq!(again, got);
    }

    fn bad(f: impl FnOnce(&mut IdleFsm)) -> bool {
        let mut m = machine();
        f(&mut m);
        IdleFsm::decode(&m.encode()).is_err()
    }

    #[test]
    fn hostile_machines_are_errors() {
        assert!(bad(|m| m.default = 3));
        assert!(bad(|m| m.states[1].next[0].0 = 9));
        assert!(bad(|m| m.states[1].next[0].1 = f32::NAN));
        assert!(bad(|m| m.states[1].next[0].1 = -1.0));
        assert!(bad(|m| m.states[1].transitions[0].to = 3));
        assert!(bad(|m| {
            m.states[1].transitions[1].conditions[0] = FsmCondition::RandomRoll {
                value: f32::INFINITY,
                invert: false,
            }
        }));
        assert!(bad(|m| {
            m.states[1].transitions[0].conditions[0] = FsmCondition::EventRandomPathOption {
                path: 0,
                value: 0,
                use_weights: true,
                options: vec![(0, f32::NAN)],
            }
        }));
        let good = machine().encode();
        for cut in 0..good.len() {
            assert!(IdleFsm::decode(&good[..cut]).is_err(), "cut {cut}");
        }
        let mut wrong = good.clone();
        wrong[4] = 9;
        assert!(matches!(
            IdleFsm::decode(&wrong),
            Err(FormatError::UnsupportedVersion { .. })
        ));
        let mut v1 = v1_bytes(&machine());
        v1.push(0);
        assert!(IdleFsm::decode(&v1).is_err());
        // An empty machine is a valid one (a model whose file has no states).
        assert!(IdleFsm::decode(&IdleFsm::default().encode()).is_ok());
    }
}
