//! `.fsmxml` to `C5FM` v2 and `.ddtxml` to `state_clips`.
//!
//! The machine keeps the states that can be entered: the default state and every state a
//! requested action enters (1000, 1500, 1600-1640 and 1280, 1910-1960), and what they reach
//! through sockets. States are numbered in the order of the file. Transitions keep the order of the
//! sockets and of their conditions in the file; the `order` attributes are not read (the file
//! lists them in no order that those numbers explain, RW-13).

use std::collections::{BTreeMap, BTreeSet};

use asset_format::{ClipKey, FsmCondition, FsmState, IdleFsm, StateClip, Transition};
use roxmltree::Node;

/// Actions whose request enters a state of the machine besides the default state.
pub fn is_requested_action(action: u32) -> bool {
    matches!(action, 1000 | 1500 | 1280 | 1600..=1640 | 1910..=1960)
}

/// Actions of the combat readiness family: the states are kept, B2 does not play them.
pub fn is_combat_action(action: u32) -> bool {
    matches!(action, 1280 | 1600..=1640)
}

fn attr<'a>(n: Node<'a, '_>, name: &str) -> Option<&'a str> {
    n.attribute(name)
}

fn num<T: std::str::FromStr>(n: Node<'_, '_>, name: &str, what: &str) -> Result<T, String> {
    let v = attr(n, name).ok_or_else(|| format!("{what}: no {name}"))?;
    v.trim()
        .parse()
        .map_err(|_| format!("{what}: {name} {v:?} is not a number"))
}

/// An integer that the file may write as `1.000000`.
fn int(n: Node<'_, '_>, name: &str, what: &str) -> Result<i32, String> {
    let v = attr(n, name)
        .ok_or_else(|| format!("{what}: no {name}"))?
        .trim();
    if let Ok(i) = v.parse::<i32>() {
        return Ok(i);
    }
    match v.parse::<f64>() {
        Ok(f) if f.fract() == 0.0 && f.abs() <= f64::from(i32::MAX) => Ok(f as i32),
        _ => Err(format!("{what}: {name} {v:?} is not an integer")),
    }
}

fn num_opt<T: std::str::FromStr>(
    n: Node<'_, '_>,
    name: &str,
    what: &str,
) -> Result<Option<T>, String> {
    attr(n, name)
        .map(|v| {
            v.trim()
                .parse()
                .map_err(|_| format!("{what}: {name} {v:?} is not a number"))
        })
        .transpose()
}

fn flag(n: Node<'_, '_>, name: &str, what: &str) -> Result<bool, String> {
    match attr(n, name).map(str::trim) {
        Some("0") | Some("false") => Ok(false),
        Some("1") | Some("true") => Ok(true),
        other => Err(format!("{what}: {name} {other:?} is not 0 or 1")),
    }
}

fn condition(c: Node<'_, '_>, what: &str) -> Result<FsmCondition, String> {
    let ty = attr(c, "type").ok_or_else(|| format!("{what}: a condition without a type"))?;
    let what = format!("{what}: {ty}");
    Ok(match ty {
        "FFSMConditionCheckPathOption" => FsmCondition::CheckPathOption {
            method: num(c, "method", &what)?,
            path: num(c, "path", &what)?,
            value: int(c, "value", &what)?,
        },
        "FFSMConditionEventSetPathOption" => FsmCondition::EventSetPathOption {
            path: num(c, "path", &what)?,
            value: int(c, "value", &what)?,
        },
        "FFSMConditionEventRandomPathOption" => {
            let mut options = Vec::new();
            for w in c
                .descendants()
                .filter(|n| n.is_element() && n.has_tag_name("weight"))
            {
                options.push((int(w, "v", &what)?, num(w, "w", &what)?));
            }
            FsmCondition::EventRandomPathOption {
                path: num(c, "path", &what)?,
                value: int(c, "value", &what)?,
                use_weights: flag(c, "useweights", &what)?,
                options,
            }
        }
        "FFSMConditionRandomRoll" => FsmCondition::RandomRoll {
            value: num(c, "value", &what)?,
            invert: flag(c, "invert", &what)?,
        },
        "FFSMConditionCheckStateTime" => FsmCondition::CheckStateTime {
            method: num(c, "method", &what)?,
            value: num(c, "value", &what)?,
        },
        "FFSMConditionECComplete" => FsmCondition::EcComplete {
            ec: num(c, "ec", &what)?,
            value: num_opt(c, "value", &what)?,
        },
        "FFSMConditionOrOperator" => FsmCondition::Or,
        other => return Err(format!("{what}: unknown condition type {other}")),
    })
}

struct RawState {
    id: u32,
    name: String,
    asset: String,
    action: u32,
}

struct RawSocket {
    from: u32,
    to: u32,
    conditions: Vec<FsmCondition>,
}

/// Compiles the text of a `.fsmxml`. `name` is the file name without extension.
pub fn parse_fsmxml(name: &str, text: &str) -> Result<IdleFsm, String> {
    let text = text.trim_start_matches('\u{feff}');
    let doc = roxmltree::Document::parse(text).map_err(|e| format!("invalid XML: {e}"))?;
    let root = doc.root_element();
    if root.tag_name().name() != "statemachine" {
        return Err(format!(
            "unexpected root <{}>, expected <statemachine>",
            root.tag_name().name()
        ));
    }
    let default_id: u32 = num(root, "default_state", "statemachine")?;

    let mut states: Vec<RawState> = Vec::new();
    if let Some(list) = root.children().find(|n| n.has_tag_name("states")) {
        for s in list.children().filter(|n| n.has_tag_name("state")) {
            let id: u32 = num(s, "id", "a state")?;
            let name = attr(s, "name").unwrap_or("").to_owned();
            let what = format!("state {id} ({name})");
            if states.iter().any(|o| o.id == id) {
                return Err(format!("{what}: the id is used twice"));
            }
            states.push(RawState {
                id,
                asset: attr(s, "asset").unwrap_or("").to_owned(),
                action: num_opt(s, "action", &what)?.unwrap_or(0),
                name,
            });
        }
    }
    let known: BTreeSet<u32> = states.iter().map(|s| s.id).collect();
    if !states.is_empty() && !known.contains(&default_id) {
        return Err(format!("default_state {default_id} is not a state"));
    }

    let mut sockets: Vec<RawSocket> = Vec::new();
    if let Some(list) = root.children().find(|n| n.has_tag_name("sockets")) {
        for s in list.children().filter(|n| n.has_tag_name("socket")) {
            let from: u32 = num(s, "from", "a socket")?;
            let to: u32 = num(s, "too", "a socket")?;
            let what = format!("socket {from} -> {to}");
            if !known.contains(&from) || !known.contains(&to) {
                return Err(format!("{what}: a state that does not exist"));
            }
            // The only value in the install is 0; what a mirrored socket would mean is unknown.
            if attr(s, "bidirectional").is_some_and(|b| b.trim() != "0") {
                return Err(format!("{what}: a bidirectional socket"));
            }
            let mut conditions = Vec::new();
            for c in s
                .descendants()
                .filter(|n| n.is_element() && n.has_tag_name("condition"))
            {
                conditions.push(condition(c, &what)?);
            }
            sockets.push(RawSocket {
                from,
                to,
                conditions,
            });
        }
    }

    // The states that can be entered: the default one, those a requested action enters, and
    // whatever they reach.
    let mut keep: BTreeSet<u32> = BTreeSet::new();
    let mut work: Vec<u32> = Vec::new();
    if !states.is_empty() {
        work.push(default_id);
    }
    work.extend(
        states
            .iter()
            .filter(|s| is_requested_action(s.action))
            .map(|s| s.id),
    );
    while let Some(id) = work.pop() {
        if keep.insert(id) {
            work.extend(sockets.iter().filter(|k| k.from == id).map(|k| k.to));
        }
    }
    let index: BTreeMap<u32, u16> = states
        .iter()
        .filter(|s| keep.contains(&s.id))
        .enumerate()
        .map(|(i, s)| {
            u16::try_from(i)
                .map(|i| (s.id, i))
                .map_err(|_| "more than 65535 states".to_owned())
        })
        .collect::<Result<_, _>>()?;

    let mut out = IdleFsm {
        name: name.to_owned(),
        default: index.get(&default_id).copied().unwrap_or(0),
        states: Vec::new(),
    };
    for s in states.iter().filter(|s| keep.contains(&s.id)) {
        let transitions = sockets
            .iter()
            .filter(|k| k.from == s.id)
            .map(|k| Transition {
                to: index[&k.to],
                conditions: k.conditions.clone(),
            })
            .collect();
        out.states.push(FsmState {
            name: s.name.clone(),
            action: s.action,
            asset: s.asset.clone(),
            // v2 keeps the weights in the conditions (`EventRandomPathOption`); `next` is v1's.
            next: Vec::new(),
            transitions,
        });
    }
    out.validate().map_err(|e| e.to_string())?;
    Ok(out)
}

// ---------------------------------------------------------------- .ddtxml

fn child<'a, 'i>(n: Node<'a, 'i>, tag: &str) -> Option<Node<'a, 'i>> {
    n.children().find(|c| c.has_tag_name(tag))
}

/// The animation of an `Item type="sequence"`: its event code and `loop`.
fn sequence(item: Node<'_, '_>, what: &str) -> Result<Option<(u32, bool)>, String> {
    let Some(seq) = child(item, "Sequence") else {
        return Ok(None);
    };
    if attr(seq, "type") != Some("animation") {
        return Ok(None);
    }
    let a = child(seq, "AnimationSequence")
        .ok_or_else(|| format!("{what}: an animation sequence without <AnimationSequence>"))?;
    let ec: u32 = num(a, "ec", what)?;
    let looped = match child(a, "SequenceInfo").and_then(|i| attr(i, "loop")) {
        Some(v) => matches!(v.trim(), "true" | "1"),
        None => false,
    };
    Ok(Some((ec, looped)))
}

/// The animation rows of a landmark decision tree: the first layer (`ModelAnimationSet`), per
/// state. A selector item is a state, named by its comment (`<!-- RawResource State -->`); items
/// directly in the layer belong to the state `""`. `repeat` is 0 for a looping sequence (until
/// the state ends) and 1 otherwise; the file has no pause, so `pause` is 0.
pub fn parse_ddtxml(text: &str) -> Result<Vec<StateClip>, String> {
    let text = text.trim_start_matches('\u{feff}');
    let doc = roxmltree::Document::parse(text).map_err(|e| format!("invalid XML: {e}"))?;
    let root = doc.root_element();
    if root.tag_name().name() != "DecisionTree" {
        return Err(format!(
            "unexpected root <{}>, expected <DecisionTree>",
            root.tag_name().name()
        ));
    }
    let layers = child(root, "Selector")
        .and_then(|s| child(s, "Layers"))
        .ok_or("no <Selector><Layers>")?;
    let Some(layer) = child(layers, "Layer") else {
        return Ok(Vec::new());
    };
    let Some(items) = child(layer, "Items") else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    let mut push = |state: &str, ec: u32, looped: bool| {
        out.push(StateClip {
            state: state.to_owned(),
            clip: ClipKey::Ec(ec),
            repeat: if looped { 0 } else { 1 },
            pause: 0.0,
        });
    };
    for (i, item) in items
        .children()
        .filter(|n| n.has_tag_name("Item"))
        .enumerate()
    {
        match attr(item, "type") {
            Some("selector") => {
                let state = item
                    .children()
                    .find(|n| n.is_comment())
                    .and_then(|c| c.text())
                    .map(|t| t.trim().trim_end_matches("State").trim().to_owned())
                    .filter(|t| !t.is_empty())
                    .unwrap_or_else(|| format!("state{i}"));
                if let Some(inner) = child(item, "Items") {
                    for it in inner.children().filter(|n| n.has_tag_name("Item")) {
                        if let Some((ec, looped)) = sequence(it, &state)? {
                            push(&state, ec, looped);
                        }
                    }
                }
            }
            Some("sequence") => {
                if let Some((ec, looped)) = sequence(item, "the layer")? {
                    push("", ec, looped);
                }
            }
            _ => {}
        }
    }
    Ok(out)
}

/// The number of `Item`s in the first layer (`ModelAnimationSet`) of a decision tree, at any
/// depth. Zero is a static tree: `StaticLandmarkDecisionTree` and `StaticBuildingDecisionTree`
/// only drive the trigger timelines of the second layer (sparkles, smoke) and play no clip.
pub fn ddtxml_animation_items(text: &str) -> Result<usize, String> {
    let text = text.trim_start_matches('\u{feff}');
    let doc = roxmltree::Document::parse(text).map_err(|e| format!("invalid XML: {e}"))?;
    let layer = child(doc.root_element(), "Selector")
        .and_then(|s| child(s, "Layers"))
        .and_then(|l| child(l, "Layer"));
    Ok(layer.map_or(0, |l| {
        l.descendants().filter(|n| n.has_tag_name("Item")).count()
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_static_tree_has_no_animation_items() {
        let static_tree = r#"<DecisionTree><Selector><Layers>
  <Layer><!-- ModelAnimationSet Layer --><Items></Items></Layer>
  <Layer><Items><Item type="sequence"><Sequence type="trigger_timeline">
    <TriggerTimelineSequence ec="1000"/></Sequence></Item></Items></Layer>
</Layers></Selector></DecisionTree>"#;
        assert_eq!(ddtxml_animation_items(static_tree), Ok(0));
        assert!(parse_ddtxml(static_tree).unwrap().is_empty());
        let herd = r#"<DecisionTree><Selector><Layers><Layer><Items>
  <Item type="selector"><!-- Constructed State --><Items>
    <Item type="sequence"><Sequence type="animation"><AnimationSequence ec="1000">
      <SequenceInfo loop="true"/></AnimationSequence></Sequence></Item>
    <Item type="sequence"><Sequence type="animation"><AnimationSequence ec="1003">
      <SequenceInfo loop="true"/></AnimationSequence></Sequence></Item>
  </Items></Item>
</Items></Layer></Layers></Selector></DecisionTree>"#;
        assert_eq!(ddtxml_animation_items(herd), Ok(3));
        let rows = parse_ddtxml(herd).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].clip, ClipKey::Ec(1003));
    }

    const FSM: &str = r#"<statemachine fsm="FGrannyStateMachine" default_state="2">
  <states>
    <state id="1" type="FGrannyState" name="IDLE_LOOP" asset="IDLE_GRAPH.dge" action="1000" sockets="1" />
    <state id="2" type="FGrannyState" name="CHOOSER" asset="" action="0" sockets="2" />
    <state id="3" type="FGrannyState" name="FIDGETS" asset="FIDGETS.dge" action="1000" sockets="0" />
    <state id="4" type="FGrannyState" name="FORTIFY_LOOP" asset="FORTIFY_GRAPH.dge" action="1500" sockets="0" />
    <state id="5" type="FGrannyState" name="ATTACKS" asset="A.dge" action="1100" sockets="0" />
    <state id="6" type="FGrannyState" name="ORPHAN" asset="" action="0" sockets="1" />
    <state id="7" type="FGrannyState" name="AFTER_ORPHAN" asset="" sockets="0" />
    <state id="8" type="FGrannyState" name="COMBAT_READY_LOOP" asset="C.dge" action="1600" sockets="0" />
  </states>
  <sockets>
    <socket from="2" too="4" bidirectional="0" order="0" conditions="1">
      <conditions>
        <condition type="FFSMConditionCheckPathOption" order="0.2" desc="" method="1" path="4" value="1500" />
      </conditions>
    </socket>
    <socket from="2" too="1" bidirectional="0" order="0" conditions="4">
      <conditions>
        <condition type="FFSMConditionEventRandomPathOption" order="0.25" desc="" path="0" value="0" useweights="1">
          <weights>
            <weight v="1" w="0.4" />
            <weight v="0" w="0.6" />
          </weights>
        </condition>
        <condition type="FFSMConditionOrOperator" order="0.3" desc="" />
        <condition type="FFSMConditionECComplete" order="0.1" desc="" ec="1040" value="0.98" />
        <condition type="FFSMConditionCheckStateTime" order="0.4" desc="" method="5" value="0.15" />
      </conditions>
    </socket>
    <socket from="1" too="5" bidirectional="0" order="0" conditions="2">
      <conditions>
        <condition type="FFSMConditionRandomRoll" order="0.6" desc="" value="0.0007" invert="0" />
        <condition type="FFSMConditionEventSetPathOption" order="0.7" desc="" path="3" value="7" />
      </conditions>
    </socket>
    <socket from="6" too="7" bidirectional="0" />
  </sockets>
</statemachine>"#;

    #[test]
    fn compiles_the_states_that_can_be_entered() {
        let m = parse_fsmxml("m_state_01", FSM).unwrap();
        let names: Vec<&str> = m.states.iter().map(|s| s.name.as_str()).collect();
        // The orphan pair no one reaches is gone; the action 1600 state stays without a socket.
        assert_eq!(
            names,
            [
                "IDLE_LOOP",
                "CHOOSER",
                "FIDGETS",
                "FORTIFY_LOOP",
                "ATTACKS",
                "COMBAT_READY_LOOP"
            ]
        );
        assert_eq!(m.states[m.default as usize].name, "CHOOSER");
        assert_eq!(m.states[m.state("FORTIFY_LOOP").unwrap()].action, 1500);
        assert_eq!(m.states[m.state("COMBAT_READY_LOOP").unwrap()].action, 1600);
    }

    #[test]
    fn transitions_keep_the_order_of_sockets_and_conditions() {
        let m = parse_fsmxml("m", FSM).unwrap();
        let chooser = &m.states[m.state("CHOOSER").unwrap()];
        let to: Vec<&str> = chooser
            .transitions
            .iter()
            .map(|t| m.states[usize::from(t.to)].name.as_str())
            .collect();
        assert_eq!(to, ["FORTIFY_LOOP", "IDLE_LOOP"]);
        assert_eq!(
            chooser.transitions[0].conditions,
            [FsmCondition::CheckPathOption {
                method: 1,
                path: 4,
                value: 1500
            }]
        );
        assert_eq!(
            chooser.transitions[1].conditions,
            [
                FsmCondition::EventRandomPathOption {
                    path: 0,
                    value: 0,
                    use_weights: true,
                    options: vec![(1, 0.4), (0, 0.6)],
                },
                FsmCondition::Or,
                FsmCondition::EcComplete {
                    ec: 1040,
                    value: Some(0.98)
                },
                FsmCondition::CheckStateTime {
                    method: 5,
                    value: 0.15
                },
            ]
        );
        let idle = &m.states[m.state("IDLE_LOOP").unwrap()];
        assert_eq!(
            idle.transitions[0].conditions,
            [
                FsmCondition::RandomRoll {
                    value: 0.0007,
                    invert: false
                },
                FsmCondition::EventSetPathOption { path: 3, value: 7 },
            ]
        );
    }

    #[test]
    fn the_blob_roundtrips() {
        let m = parse_fsmxml("m", FSM).unwrap();
        assert_eq!(IdleFsm::decode(&m.encode()).unwrap(), m);
    }

    #[test]
    fn bad_files_are_errors() {
        for (why, text) in [
            ("not xml", "<statemachine"),
            ("root", "<x/>"),
            ("no default", "<statemachine><states/></statemachine>"),
            (
                "default not a state",
                r#"<statemachine default_state="9"><states><state id="1" name="a"/></states></statemachine>"#,
            ),
            (
                "twice",
                r#"<statemachine default_state="1"><states><state id="1" name="a"/><state id="1" name="b"/></states></statemachine>"#,
            ),
            (
                "socket to nowhere",
                r#"<statemachine default_state="1"><states><state id="1" name="a"/></states><sockets><socket from="1" too="2"/></sockets></statemachine>"#,
            ),
            (
                "bidirectional",
                r#"<statemachine default_state="1"><states><state id="1" name="a"/></states><sockets><socket from="1" too="1" bidirectional="1"/></sockets></statemachine>"#,
            ),
            (
                "unknown condition",
                r#"<statemachine default_state="1"><states><state id="1" name="a"/></states><sockets><socket from="1" too="1"><conditions><condition type="FFSMConditionNew"/></conditions></socket></sockets></statemachine>"#,
            ),
            (
                "bad number",
                r#"<statemachine default_state="1"><states><state id="1" name="a"/></states><sockets><socket from="1" too="1"><conditions><condition type="FFSMConditionCheckPathOption" method="x" path="1" value="1"/></conditions></socket></sockets></statemachine>"#,
            ),
        ] {
            assert!(parse_fsmxml("m", text).is_err(), "{why}");
        }
    }

    #[test]
    fn a_file_without_sockets_or_states_is_an_empty_machine() {
        let m = parse_fsmxml(
            "m",
            r#"<statemachine default_state="0"><states></states></statemachine>"#,
        )
        .unwrap();
        assert!(m.states.is_empty());
    }

    #[test]
    fn the_action_families() {
        for a in [1000, 1500, 1280, 1600, 1640, 1910, 1960] {
            assert!(is_requested_action(a), "{a}");
        }
        for a in [0, 1100, 1200, 1400, 1650, 1900, 1990, 10000] {
            assert!(!is_requested_action(a), "{a}");
        }
        assert!(is_combat_action(1280) && is_combat_action(1620) && !is_combat_action(1500));
    }

    const DDT: &str = r#"<DecisionTree>
  <Selector>
    <Layers>
      <Layer>
        <!-- ModelAnimationSet Layer -->
        <Items>
          <Item type="selector">
            <!-- RawResource State -->
            <Items>
              <Item type="sequence">
                <Sequence type="animation">
                  <AnimationSequence ec="1000">
                    <SequenceInfo duration="6.000000" speed="1.000000" loop="true" />
                  </AnimationSequence>
                </Sequence>
              </Item>
              <Item type="sequence">
                <Sequence type="animation">
                  <AnimationSequence ec="1001">
                    <SequenceInfo duration="6.000000" speed="1.000000" loop="false" />
                  </AnimationSequence>
                </Sequence>
              </Item>
            </Items>
          </Item>
          <Item type="selector">
            <!-- Pillaged State -->
            <Items>
              <Item type="sequence">
                <Sequence type="animation">
                  <AnimationSequence ec="1002">
                    <SequenceInfo duration="6.000000" speed="1.000000" loop="true" />
                  </AnimationSequence>
                </Sequence>
              </Item>
            </Items>
          </Item>
        </Items>
      </Layer>
      <Layer>
        <Items>
          <Item type="sequence">
            <Sequence type="trigger_timeline">
              <TriggerTimelineSequence ec="1000">
                <SequenceInfo duration="6.000000" speed="1.000000" loop="true" />
              </TriggerTimelineSequence>
            </Sequence>
          </Item>
        </Items>
      </Layer>
    </Layers>
  </Selector>
</DecisionTree>"#;

    #[test]
    fn a_decision_tree_gives_clips_per_state() {
        let rows = parse_ddtxml(DDT).unwrap();
        let got: Vec<(&str, &ClipKey, u16)> = rows
            .iter()
            .map(|r| (r.state.as_str(), &r.clip, r.repeat))
            .collect();
        assert_eq!(
            got,
            [
                ("RawResource", &ClipKey::Ec(1000), 0),
                ("RawResource", &ClipKey::Ec(1001), 1),
                ("Pillaged", &ClipKey::Ec(1002), 0),
            ]
        );
    }

    #[test]
    fn a_flat_decision_tree_is_the_empty_state() {
        let flat = r#"<DecisionTree><Selector><Layers><Layer><Items>
          <Item type="sequence"><Sequence type="animation"><AnimationSequence ec="0"><SequenceInfo loop="true"/></AnimationSequence></Sequence></Item>
        </Items></Layer></Layers></Selector></DecisionTree>"#;
        let rows = parse_ddtxml(flat).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].state, "");
        assert!(parse_ddtxml("<x/>").is_err());
        assert!(parse_ddtxml("<DecisionTree/>").is_err());
        assert_eq!(
            parse_ddtxml("<DecisionTree><Selector><Layers/></Selector></DecisionTree>").unwrap(),
            []
        );
    }
}
