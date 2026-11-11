//! The build queue, buying and research: the order each intent turns into and every refusal.

mod common;

use common::{Rig, city_at, key, kstr, s};
use protocol::Command;
use ui2_actions::Sent;
use ui2_model::generated::{ItemKind, Opt, OptGroup, Production, QueueRow, Research, Tech};
use ui2_model::{Key, KeyedList, Value};

/// `key<item>` of an order type and an id: `(order << 16) | item`.
fn item(order: u32, id: u32) -> Key {
    Key::from_u32((order << 16) | id)
}

fn ks(k: Key) -> Value {
    s(&k.to_js())
}

const WARRIOR: (u32, u32) = (0, 5);
const LIBRARY: (u32, u32) = (1, 7);
const BANK: (u32, u32) = (1, 9);
const UNIVERSITY: (u32, u32) = (1, 11);
const GRANARY: (u32, u32) = (1, 3);

fn opt(of: (u32, u32), enabled: bool) -> Opt {
    Opt {
        item: item(of.0, of.1),
        order: of.1 as i32,
        enabled,
        locked_reason: (!enabled).then(|| "Needs Banking".into()),
        ..Opt::default()
    }
}

fn queue_row(slot: i32, of: (u32, u32)) -> QueueRow {
    QueueRow {
        slot,
        item: item(of.0, of.1),
        kind: ItemKind::Building,
        ..QueueRow::default()
    }
}

fn window(city: u32) -> Production {
    let mut university = opt(UNIVERSITY, true);
    university.buy_gold = Some(560);
    university.buy_gold_ok = false;
    let mut granary = opt(GRANARY, true);
    granary.buy_gold = Some(100);
    granary.buy_gold_ok = true;
    granary.buy_faith = Some(80);
    granary.buy_faith_ok = true;
    Production {
        city: key(city),
        gold: 312,
        current: Some(Default::default()),
        queue_known: true,
        queue: KeyedList::from_vec(vec![
            queue_row(0, LIBRARY),
            queue_row(1, GRANARY),
            queue_row(2, WARRIOR),
        ])
        .unwrap(),
        groups: KeyedList::from_vec(vec![
            OptGroup {
                kind: ItemKind::Unit,
                count: 1,
                options: KeyedList::from_vec(vec![opt(WARRIOR, true)]).unwrap(),
            },
            OptGroup {
                kind: ItemKind::Building,
                count: 4,
                options: KeyedList::from_vec(vec![
                    opt(LIBRARY, true),
                    opt(BANK, false),
                    university,
                    granary,
                ])
                .unwrap(),
            },
        ])
        .unwrap(),
        ..Production::default()
    }
}

fn city_rig() -> Rig {
    let mut rig = Rig::new();
    rig.cities(vec![
        city_at(20, 1, 1, true),
        city_at(22, 2, 2, true),
        city_at(21, 3, 3, false),
    ]);
    rig.put(Some(window(20)));
    rig
}

fn go(rig: &mut Rig, name: &str, args: &[(&str, Value)]) {
    let mut all = vec![("city", kstr(20))];
    all.extend_from_slice(args);
    rig.one(name, &all);
}

#[test]
fn choose_replaces_the_queue_with_the_item() {
    let mut rig = city_rig();
    go(&mut rig, "production.choose", &[("item", ks(item(1, 7)))]);
    assert_eq!(
        rig.commands(),
        vec![Command::CitySetProduction {
            city: 20,
            order: 1,
            item: 7
        }]
    );
    assert!(matches!(rig.sent()[0], Sent::Command { turn: 42, .. }));
    let row = rig.ui().pending.iter().next().unwrap().clone();
    assert_eq!(&*row.intent, "production.choose");
    assert_eq!(
        &*row.target, "city/0:20/0:65543",
        "the source looks up `option_target`"
    );
    assert!(rig.toasts().is_empty());
}

#[test]
fn an_item_of_each_order_type_carries_its_order_and_id() {
    let mut rig = city_rig();
    go(&mut rig, "production.choose", &[("item", ks(item(0, 5)))]);
    assert_eq!(
        rig.commands(),
        vec![Command::CitySetProduction {
            city: 20,
            order: 0,
            item: 5
        }]
    );
}

#[test]
fn a_closed_option_is_refused_with_nothing_sent() {
    for name in ["production.choose", "production.replace", "production.push"] {
        let mut rig = city_rig();
        let mut args = vec![("item", ks(item(BANK.0, BANK.1)))];
        if name == "production.push" {
            args.push(("at", s("back")));
        }
        go(&mut rig, name, &args);
        rig.assert_refused("option_unavailable");
    }
}

#[test]
fn an_item_the_window_does_not_have_is_refused() {
    let mut rig = city_rig();
    go(&mut rig, "production.choose", &[("item", ks(item(1, 99)))]);
    rig.assert_refused("option_unavailable");
}

#[test]
fn a_foreign_city_is_refused() {
    let mut rig = city_rig();
    rig.one(
        "production.choose",
        &[("city", kstr(21)), ("item", ks(item(1, 7)))],
    );
    rig.assert_refused("not_your_city");
}

#[test]
fn a_city_whose_window_the_model_does_not_show_cannot_be_checked() {
    let mut rig = city_rig();
    rig.one(
        "production.choose",
        &[("city", kstr(22)), ("item", ks(item(1, 7)))],
    );
    rig.assert_refused("option_unavailable");

    let mut rig = city_rig();
    rig.put(None::<Production>);
    go(&mut rig, "production.choose", &[("item", ks(item(1, 7)))]);
    rig.assert_refused("option_unavailable");
}

#[test]
fn replace_changes_only_the_head() {
    let mut rig = city_rig();
    go(&mut rig, "production.replace", &[("item", ks(item(1, 7)))]);
    assert_eq!(
        rig.commands(),
        vec![Command::CityReplaceHead {
            city: 20,
            order: 1,
            item: 7
        }]
    );
}

#[test]
fn replace_needs_something_being_built() {
    let mut rig = city_rig();
    let mut w = window(20);
    w.current = None;
    w.queue = KeyedList::new();
    rig.put(Some(w));
    go(&mut rig, "production.replace", &[("item", ks(item(1, 7)))]);
    rig.assert_refused("bad_slot");
}

#[test]
fn push_goes_to_the_back_or_the_front() {
    let mut rig = city_rig();
    go(
        &mut rig,
        "production.push",
        &[("item", ks(item(1, 7))), ("at", s("back"))],
    );
    go(
        &mut rig,
        "production.push",
        &[("item", ks(item(0, 5))), ("at", s("front"))],
    );
    assert_eq!(
        rig.commands(),
        vec![
            Command::CityQueuePush {
                city: 20,
                order: 1,
                item: 7,
                front: false
            },
            Command::CityQueuePush {
                city: 20,
                order: 0,
                item: 5,
                front: true
            },
        ]
    );
}

#[test]
fn queue_orders_need_the_queue_to_be_known() {
    let mut rig = city_rig();
    let mut w = window(20);
    w.queue_known = false;
    rig.put(Some(w));
    go(
        &mut rig,
        "production.push",
        &[("item", ks(item(1, 7))), ("at", s("back"))],
    );
    rig.assert_refused("unsupported");

    let mut rig = city_rig();
    let mut w = window(20);
    w.queue_known = false;
    rig.put(Some(w));
    go(&mut rig, "production.pop", &[("slot", Value::I32(0))]);
    rig.assert_refused("unsupported");
}

#[test]
fn pop_removes_a_slot_of_the_queue() {
    let mut rig = city_rig();
    go(&mut rig, "production.pop", &[("slot", Value::I32(2))]);
    assert_eq!(
        rig.commands(),
        vec![Command::CityQueuePop { city: 20, slot: 2 }]
    );
    let row = rig.ui().pending.iter().next().unwrap().clone();
    assert_eq!(&*row.target, "city/0:20");
}

#[test]
fn pop_outside_the_queue_is_refused() {
    for slot in [3, -1, 300] {
        let mut rig = city_rig();
        go(&mut rig, "production.pop", &[("slot", Value::I32(slot))]);
        rig.assert_refused("bad_slot");
    }
}

#[test]
fn move_swaps_two_slots_of_the_queue() {
    let mut rig = city_rig();
    go(
        &mut rig,
        "production.move",
        &[("from", Value::I32(2)), ("to", Value::I32(0))],
    );
    assert_eq!(
        rig.commands(),
        vec![Command::CityQueueMove {
            city: 20,
            from: 2,
            to: 0
        }]
    );
}

#[test]
fn move_with_a_bad_slot_or_to_itself_is_refused() {
    for (from, to) in [(0, 3), (5, 1), (1, 1), (-1, 0)] {
        let mut rig = city_rig();
        go(
            &mut rig,
            "production.move",
            &[("from", Value::I32(from)), ("to", Value::I32(to))],
        );
        rig.assert_refused("bad_slot");
    }
}

#[test]
fn buy_with_gold_and_with_faith() {
    let mut rig = city_rig();
    go(
        &mut rig,
        "production.buy",
        &[("item", ks(item(1, 3))), ("with", s("gold"))],
    );
    go(
        &mut rig,
        "production.buy",
        &[("item", ks(item(1, 3))), ("with", s("faith"))],
    );
    assert_eq!(
        rig.commands(),
        vec![
            Command::CityPurchase {
                city: 20,
                order: 1,
                item: 3,
                yield_: 2
            },
            Command::CityPurchase {
                city: 20,
                order: 1,
                item: 3,
                yield_: 5
            },
        ]
    );
    // Both rows name the option of the city (item 1:3 is the key 65539), as the source looks it up.
    let rows: Vec<_> = rig
        .ui()
        .pending
        .iter()
        .map(|p| p.target.to_string())
        .collect();
    assert_eq!(rows, vec!["city/0:20/0:65539"; 2]);
}

#[test]
fn buying_what_the_player_cannot_afford_is_refused() {
    // University costs 560, the player has 312.
    let mut rig = city_rig();
    go(
        &mut rig,
        "production.buy",
        &[
            ("item", ks(item(UNIVERSITY.0, UNIVERSITY.1))),
            ("with", s("gold")),
        ],
    );
    rig.assert_refused("cannot_buy");

    // No faith price at all.
    let mut rig = city_rig();
    go(
        &mut rig,
        "production.buy",
        &[
            ("item", ks(item(UNIVERSITY.0, UNIVERSITY.1))),
            ("with", s("faith")),
        ],
    );
    rig.assert_refused("cannot_buy");

    // An item that is not in the window.
    let mut rig = city_rig();
    go(
        &mut rig,
        "production.buy",
        &[("item", ks(item(1, 99))), ("with", s("gold"))],
    );
    rig.assert_refused("option_unavailable");
}

#[test]
fn the_sink_refusing_is_a_toast() {
    let mut rig = city_rig();
    rig.sink.refuse = Some(ui2_actions::Refused::TurnChanged);
    go(&mut rig, "production.choose", &[("item", ks(item(1, 7)))]);
    assert_eq!(rig.why(), vec!["turn_changed"]);
    assert!(rig.ui().pending.is_empty());
}

fn tech(id: u32) -> Tech {
    Tech {
        id: Key::from_u32(id),
        ..Tech::default()
    }
}

fn research_rig() -> Rig {
    let mut rig = Rig::new();
    rig.put(Research {
        known: true,
        options: KeyedList::from_vec(vec![tech(4), tech(6)]).unwrap(),
        queue: KeyedList::from_vec(vec![tech(9)]).unwrap(),
        ..Research::default()
    });
    rig
}

fn tk(id: u32) -> Value {
    s(&Key::from_u32(id).to_js())
}

#[test]
fn research_now_and_queued() {
    let mut rig = research_rig();
    rig.one(
        "research.choose",
        &[("tech", tk(4)), ("queue", Value::Bool(false))],
    );
    rig.one(
        "research.choose",
        &[("tech", tk(6)), ("queue", Value::Bool(true))],
    );
    assert_eq!(
        rig.commands(),
        vec![
            Command::ResearchSet {
                tech: 4,
                queue: false
            },
            Command::ResearchSet {
                tech: 6,
                queue: true
            },
        ]
    );
    assert!(matches!(rig.sent()[0], Sent::Command { turn: 42, .. }));
    let rows: Vec<_> = rig
        .ui()
        .pending
        .iter()
        .map(|p| p.target.to_string())
        .collect();
    assert_eq!(rows, vec![tk_text(4), tk_text(6)]);
}

fn tk_text(id: u32) -> String {
    format!("tech/0:{id}")
}

#[test]
fn a_tech_that_is_not_an_option_is_refused() {
    let mut rig = research_rig();
    rig.one(
        "research.choose",
        &[("tech", tk(77)), ("queue", Value::Bool(false))],
    )
    .unwrap();
    rig.assert_refused("tech_unavailable");

    // One that is only in the queue cannot be chosen now, but may be queued again.
    let mut rig = research_rig();
    rig.one(
        "research.choose",
        &[("tech", tk(9)), ("queue", Value::Bool(false))],
    )
    .unwrap();
    rig.assert_refused("tech_unavailable");
    let mut rig = research_rig();
    rig.one(
        "research.choose",
        &[("tech", tk(9)), ("queue", Value::Bool(true))],
    );
    assert_eq!(
        rig.commands(),
        vec![Command::ResearchSet {
            tech: 9,
            queue: true
        }]
    );
}

#[test]
fn without_an_answer_of_the_core_research_is_unsupported() {
    let mut rig = Rig::new();
    rig.one(
        "research.choose",
        &[("tech", tk(4)), ("queue", Value::Bool(false))],
    )
    .unwrap();
    rig.assert_refused("unsupported");

    let mut rig = Rig::new();
    rig.put(Research {
        known: false,
        ..Research::default()
    });
    rig.one(
        "research.choose",
        &[("tech", tk(4)), ("queue", Value::Bool(false))],
    )
    .unwrap();
    rig.assert_refused("unsupported");
}
