//! The seat stream (9 `stream`): the materialized view and its checks, the batches, the
//! keyframe rule, `MapBasis`, `Attached`; then the same through a game actor and the mock worker:
//! orders, repeats, `Lost`, restarts, queries, replay. The first half drives [`SeatStream`]
//! directly with synthetic views, the second half runs a whole game on the mock worker.

mod common;

use std::sync::Arc;

use protocol::{
    AttachMode, Attached, BasisClaim, CatalogRef, ContextMsg, GameBatch, GameMsg, KeyframeReason,
    MapBasis, PlotBasisView, Resume, ServerMsg, UnitContext, ViewDiff, ViewHash, ViewUpdate,
    WorldView, basis_hash_of, diff,
};
use server::game::stats::SeatStats;
use server::game::stream::{
    AttachArgs, Parts, RING_BATCHES, SeatStream, WANT_FULL_EVERY_MS, keyframe_wins,
};
use wire::Limits;

// ---------------------------------------------------------------- synthetic views

const W: u16 = 8;
const H: u16 = 6;

fn basis(w: u16, h: u16) -> MapBasis {
    let mut m = MapBasis {
        game: 1,
        basis_hash: 0,
        width: w,
        height: h,
        wrap_x: false,
        wrap_y: false,
        terrain_seed: 5,
        plots: vec![PlotBasisView::default(); usize::from(w) * usize::from(h)],
    };
    m.basis_hash = basis_hash_of(&m);
    m
}

fn view(w: u16, h: u16, turn: i32) -> WorldView {
    let mut v = WorldView {
        turn,
        year: 0,
        human: 0,
        human_team: 0,
        status: protocol::TurnStatus::HumanTurn,
        blocker: None,
        map: Default::default(),
        units: vec![],
        cities: vec![],
        players: vec![],
        top_bar: Default::default(),
        notifications: vec![],
        peace_offers: vec![],
        trade_routes: vec![],
    };
    v.map.width = w;
    v.map.height = h;
    v.map.terrain_seed = 5;
    v.map.plots = vec![Default::default(); usize::from(w) * usize::from(h)];
    v
}

fn full(rev: u32, v: &WorldView) -> ViewUpdate {
    ViewUpdate::Full {
        rev,
        view: v.clone(),
        hash: ViewHash::of(v).value(),
    }
}

fn delta(base_rev: u32, rev: u32, old: &WorldView, new: &WorldView) -> ViewUpdate {
    ViewUpdate::Delta {
        base_rev,
        rev,
        diff: diff(old, new, None),
        hash: ViewHash::of(new).value(),
        basis_hash: None,
    }
}

fn parts_view(update: ViewUpdate) -> Parts {
    Parts {
        view: Some(update),
        ..Parts::default()
    }
}

fn decode(frame: &[u8]) -> ServerMsg {
    wire::decode(frame, &Limits::CLIENT).expect("a frame the client can read")
}

fn batch(frame: &[u8]) -> GameBatch {
    match decode(frame) {
        ServerMsg::Game(b) => b,
        other => panic!("expected a batch, got {other:?}"),
    }
}

fn kinds(b: &GameBatch) -> Vec<&'static str> {
    b.msgs
        .iter()
        .map(|m| match m {
            GameMsg::Keyframe(_) => "Keyframe",
            GameMsg::Diff(_) => "Diff",
            GameMsg::Results(_) => "Results",
            GameMsg::Journal(_) => "Journal",
            GameMsg::TurnState(_) => "TurnState",
            GameMsg::Seats(_) => "Seats",
            GameMsg::Context(_) => "Context",
            GameMsg::Chat(_) => "Chat",
            GameMsg::MapBasis(_) => "MapBasis",
            GameMsg::GcBase(_) => "GcBase",
            GameMsg::GcJournal(_) => "GcJournal",
            GameMsg::GcTurn(_) => "GcTurn",
            GameMsg::AiTurn(_) => "AiTurn",
        })
        .collect()
}

const CATALOG: CatalogRef = CatalogRef { hash: 9, len: 1 };

fn args() -> AttachArgs {
    AttachArgs {
        resume: None,
        claim: None,
        catalog: CATALOG,
        last_cmd_id: 0,
    }
}

fn new_stream() -> (SeatStream, Arc<SeatStats>) {
    let stats = Arc::new(SeatStats::default());
    (SeatStream::new(1, 0, Arc::clone(&stats)), stats)
}

/// A stream with a basis and a `Full` (rev 1) of the `W` x `H` map, not attached yet.
fn started() -> (SeatStream, Arc<SeatStats>, WorldView) {
    let (mut s, stats) = new_stream();
    let v = view(W, H, 1);
    let out = s.step(
        Parts {
            basis: Some(basis(W, H)),
            view: Some(full(1, &v)),
            ..Parts::default()
        },
        0,
    );
    assert_eq!(out.want_full, None);
    (s, stats, v)
}

fn get(c: &std::sync::atomic::AtomicU64) -> u64 {
    c.load(std::sync::atomic::Ordering::Relaxed)
}

/// A world with one more unit: a small change.
fn with_unit(v: &WorldView, id: i32) -> WorldView {
    let mut n = v.clone();
    n.units.push(protocol::UnitView {
        id,
        owner: 0,
        ..Default::default()
    });
    n
}

// ---------------------------------------------------------------- stream numbers, first batch

#[test]
fn nothing_goes_out_before_the_first_full_and_deltas_wait() {
    let (mut s, _stats) = new_stream();
    assert!(s.attach(args()).is_none(), "no view, no attach");
    let v = view(W, H, 1);
    // A delta before any `Full`: dropped, nothing out, no ask for a full view either.
    let out = s.step(parts_view(delta(1, 2, &v, &with_unit(&v, 1))), 0);
    assert!(out.frame.is_none());
    assert_eq!(out.want_full, None);
    assert_eq!(s.seq(), 0);
    assert!(s.waits_for_full());
}

#[test]
fn the_first_batch_of_a_fresh_stream_is_ordered_and_numbered_from_one() {
    let (mut s, _stats, v) = started();
    // The step that carried the `Full` made batch 1 in the ring (no socket was there yet).
    assert_eq!(s.seq(), 1);
    let turn = protocol::TurnState {
        turn: 1,
        phase: protocol::TurnPhase::Humans,
        mode: protocol::TurnMode::Hybrid,
        seats: vec![],
        timers: vec![],
        paused: false,
    };
    s.step(
        Parts {
            turn: Some(turn),
            seats: Some(vec![]),
            journal: vec![protocol::JournalEntry {
                turn: 1,
                kind: protocol::JournalKind::Info,
                text: "hello".into(),
                at: None,
                simplification: None,
            }],
            ..Parts::default()
        },
        0,
    );
    let a = s.attach(args()).expect("a view is there");
    assert_eq!(a.mode, AttachMode::Fresh);
    let ServerMsg::Attached(Attached {
        stream,
        mode,
        catalog,
        last_cmd_id,
        ..
    }) = a.attached
    else {
        panic!("Attached expected");
    };
    assert_eq!(
        (mode, catalog, last_cmd_id),
        (AttachMode::Fresh, CATALOG, 0)
    );
    assert_eq!(stream, s.stream_id());
    assert_eq!(a.frames.len(), 1);
    let b = batch(&a.frames[0]);
    // `seq` starts at 1 again, and the order is the one of NP 4.4.
    assert_eq!(b.seq, 1);
    assert_eq!(
        kinds(&b),
        ["MapBasis", "Keyframe", "Journal", "TurnState", "Seats"]
    );
    let GameMsg::Keyframe(kf) = &b.msgs[1] else {
        unreachable!()
    };
    assert_eq!(kf.view, v);
    assert_eq!(kf.hash, ViewHash::of(&v).value());
    assert_eq!(kf.basis_hash, basis(W, H).basis_hash);
    assert_eq!(s.view_seq(), 1);
}

#[test]
fn every_attach_gets_a_new_stream_number_and_seq_one() {
    let (mut s, _stats, _v) = started();
    let first = s.stream_id();
    let a = s.attach(args()).unwrap();
    let second = s.stream_id();
    assert_ne!(first, second);
    assert_eq!(batch(&a.frames[0]).seq, 1);
    let b = s.attach(args()).unwrap();
    assert_ne!(s.stream_id(), second);
    assert_eq!(batch(&b.frames[0]).seq, 1);
    assert_eq!(s.ring().len(), 1, "the ring starts over with a Fresh");
}

#[test]
fn seq_grows_by_one_and_a_small_diff_is_a_diff() {
    let (mut s, stats, v) = started();
    s.attach(args()).unwrap();
    let v2 = with_unit(&v, 7);
    let out = s.step(parts_view(delta(1, 2, &v, &v2)), 0);
    let b = batch(&out.frame.unwrap());
    assert_eq!(b.seq, 2);
    let GameMsg::Diff(d) = &b.msgs[0] else {
        panic!("a diff expected, got {:?}", kinds(&b));
    };
    // The diff is to the batch that carried the last view message.
    assert_eq!(d.base, 1);
    assert_eq!(d.hash, ViewHash::of(&v2).value());
    assert_eq!(s.view_seq(), 2);
    assert_eq!(s.rev(), 2);
    assert_eq!(get(&stats.diffs), 1);
    // A step with no view change does not move `view_seq`.
    let out = s.step(
        Parts {
            seats: Some(vec![]),
            ..Parts::default()
        },
        0,
    );
    assert_eq!(batch(&out.frame.unwrap()).seq, 3);
    assert_eq!(s.view_seq(), 2);
    // An empty step makes no batch.
    assert!(s.step(Parts::default(), 0).frame.is_none());
    assert_eq!(s.seq(), 3);
}

// ---------------------------------------------------------------- keyframe or diff (NP-D7)

#[test]
fn a_diff_over_half_of_the_plots_is_a_keyframe() {
    let (mut s, _stats, v) = started();
    s.attach(args()).unwrap();
    let mut v2 = v.clone();
    let n = v2.map.plots.len();
    for p in v2.map.plots.iter_mut().take(n / 2 + 1) {
        p.terrain = 3;
    }
    let out = s.step(parts_view(delta(1, 2, &v, &v2)), 0);
    let b = batch(&out.frame.unwrap());
    assert_eq!(
        kinds(&b),
        ["Keyframe"],
        "the basis did not change: no MapBasis"
    );
}

#[test]
fn the_packed_comparison_decides_between_the_thresholds() {
    // Under 30 % of the raw keyframe: a diff, the packed sizes are not even looked at.
    assert!(!keyframe_wins(29, 100, || panic!("not asked")));
    // At 30 % or more the packed sizes decide: over 70 % of the packed keyframe is a keyframe.
    assert!(keyframe_wins(30, 100, || (71, 100)));
    assert!(!keyframe_wins(30, 100, || (70, 100)));
    assert!(!keyframe_wins(90, 100, || (10, 100)));
}

#[test]
fn a_diff_that_changes_over_half_of_the_basis_is_a_map_basis_and_a_keyframe() {
    let (mut s, stats, v) = started();
    s.attach(args()).unwrap();
    let mut b2 = basis(W, H);
    let n = b2.plots.len();
    for p in b2.plots.iter_mut().take(n / 2 + 1) {
        p.terrain = 2;
    }
    b2.basis_hash = basis_hash_of(&b2);
    let changes: Vec<(u32, PlotBasisView)> = b2
        .plots
        .iter()
        .enumerate()
        .take(n / 2 + 1)
        .map(|(i, p)| (i as u32, p.clone()))
        .collect();
    let d = ViewDiff {
        basis: changes,
        ..ViewDiff::default()
    };
    let upd = ViewUpdate::Delta {
        base_rev: 1,
        rev: 2,
        diff: d,
        hash: ViewHash::of(&v).value(),
        basis_hash: Some(b2.basis_hash),
    };
    let out = s.step(parts_view(upd), 0);
    let b = batch(&out.frame.unwrap());
    assert_eq!(kinds(&b), ["MapBasis", "Keyframe"]);
    let GameMsg::MapBasis(m) = &b.msgs[0] else {
        unreachable!()
    };
    assert_eq!(m.basis_hash, b2.basis_hash);
    assert_eq!(s.sent_basis_hash(), Some(b2.basis_hash));
    assert_eq!(
        get(&stats.basis_sent),
        3,
        "the first step, the Fresh and this one"
    );
}

#[test]
fn a_small_basis_change_rides_in_the_diff() {
    let (mut s, _stats, v) = started();
    s.attach(args()).unwrap();
    let mut b2 = basis(W, H);
    b2.plots[3].terrain = 2;
    b2.basis_hash = basis_hash_of(&b2);
    let d = ViewDiff {
        basis: vec![(3, b2.plots[3].clone())],
        ..ViewDiff::default()
    };
    let upd = ViewUpdate::Delta {
        base_rev: 1,
        rev: 2,
        diff: d,
        hash: ViewHash::of(&v).value(),
        basis_hash: Some(b2.basis_hash),
    };
    let out = s.step(parts_view(upd), 0);
    let b = batch(&out.frame.unwrap());
    assert_eq!(kinds(&b), ["Diff"]);
    let GameMsg::Diff(d) = &b.msgs[0] else {
        unreachable!()
    };
    assert_eq!(d.basis_hash, Some(b2.basis_hash));
    assert_eq!(s.sent_basis_hash(), Some(b2.basis_hash));
}

// ---------------------------------------------------------------- bad worker updates

#[test]
fn a_delta_on_the_wrong_base_asks_for_a_full_view_and_deltas_are_dropped_until_it_comes() {
    let (mut s, stats, v) = started();
    s.attach(args()).unwrap();
    let hash_before = s.view_hash();
    let v2 = with_unit(&v, 1);
    // Base 5, the stream is at rev 1.
    let out = s.step(parts_view(delta(5, 6, &v, &v2)), 10_000);
    assert!(out.frame.is_none(), "the client gets nothing new");
    assert_eq!(out.want_full, Some(false));
    assert_eq!(s.view_hash(), hash_before);
    assert_eq!(get(&stats.hash_fail_worker), 1);
    assert_eq!(get(&stats.wantfull), 1);
    // A good delta now is dropped: we wait for the `Full`.
    let out = s.step(parts_view(delta(1, 2, &v, &v2)), 11_000);
    assert!(out.frame.is_none());
    assert_eq!(get(&stats.deltas_dropped), 1);
    assert_eq!(out.want_full, None, "not more often than every 5 s");
    // 5 s after the ask, a dropped delta asks again.
    let out = s.step(
        parts_view(delta(1, 2, &v, &v2)),
        10_000 + WANT_FULL_EVERY_MS,
    );
    assert_eq!(out.want_full, Some(false));
    // The `Full` ends the wait: a keyframe in the same stream, no new Attached.
    let out = s.step(parts_view(full(3, &v2)), 20_000);
    let b = batch(&out.frame.unwrap());
    assert_eq!(kinds(&b), ["Keyframe"]);
    assert!(!s.waits_for_full());
    assert_eq!(s.rev(), 3);
}

#[test]
fn a_delta_with_a_wrong_hash_is_undone() {
    let (mut s, stats, v) = started();
    s.attach(args()).unwrap();
    let v2 = with_unit(&v, 1);
    let before = (s.view_hash(), s.view().cloned());
    let ViewUpdate::Delta {
        base_rev,
        rev,
        diff,
        basis_hash,
        ..
    } = delta(1, 2, &v, &v2)
    else {
        unreachable!()
    };
    let bad = ViewUpdate::Delta {
        base_rev,
        rev,
        diff,
        hash: 0xDEAD,
        basis_hash,
    };
    let out = s.step(parts_view(bad), 0);
    assert_eq!(out.want_full, Some(false));
    assert!(out.frame.is_none());
    // The view is as it was (`UndoLog::restore`), hash included.
    assert_eq!((s.view_hash(), s.view().cloned()), before);
    assert_eq!(get(&stats.hash_fail_worker), 1);
}

#[test]
fn a_delta_with_a_wrong_basis_hash_asks_for_the_basis_too() {
    let (mut s, _stats, v) = started();
    s.attach(args()).unwrap();
    let mut b2 = basis(W, H);
    b2.plots[0].terrain = 4;
    b2.basis_hash = basis_hash_of(&b2);
    let upd = ViewUpdate::Delta {
        base_rev: 1,
        rev: 2,
        diff: ViewDiff {
            basis: vec![(0, b2.plots[0].clone())],
            ..ViewDiff::default()
        },
        hash: ViewHash::of(&v).value(),
        basis_hash: Some(b2.basis_hash ^ 1),
    };
    let basis_before = s.basis_hash();
    let out = s.step(parts_view(upd), 0);
    assert_eq!(out.want_full, Some(true));
    assert_eq!(s.basis_hash(), basis_before, "the basis was put back");
}

#[test]
fn a_full_that_does_not_check_out_is_refused() {
    let (mut s, _stats) = new_stream();
    let v = view(W, H, 1);
    // No basis at all: the pair cannot be checked.
    let out = s.step(parts_view(full(1, &v)), 0);
    assert_eq!(out.want_full, Some(true));
    assert!(!s.has_view());
    // A basis of another map size does not fit the view.
    let out = s.step(
        Parts {
            basis: Some(basis(W + 1, H)),
            view: Some(full(1, &v)),
            ..Parts::default()
        },
        10_000,
    );
    assert_eq!(out.want_full, Some(true));
    assert!(!s.has_view());
    // A wrong hash.
    let ViewUpdate::Full { rev, view, .. } = full(1, &v) else {
        unreachable!()
    };
    let out = s.step(
        Parts {
            basis: Some(basis(W, H)),
            view: Some(ViewUpdate::Full { rev, view, hash: 1 }),
            ..Parts::default()
        },
        20_000,
    );
    assert!(out.frame.is_none());
    assert!(!s.has_view());
}

// ---------------------------------------------------------------- MapBasis

#[test]
fn a_claim_with_the_same_basis_skips_the_map_basis() {
    let (mut s, stats, _v) = started();
    let h = basis(W, H).basis_hash;
    let claim = |game, hash| AttachArgs {
        claim: Some(BasisClaim { game, hash }),
        ..args()
    };
    // The same game and hash: skipped.
    let a = s.attach(claim(1, h)).unwrap();
    assert_eq!(kinds(&batch(&a.frames[0]))[..1], ["Keyframe"]);
    assert_eq!(get(&stats.basis_skipped), 1);
    // Another hash, or another game: shown.
    let a = s.attach(claim(1, h ^ 1)).unwrap();
    assert_eq!(kinds(&batch(&a.frames[0]))[0], "MapBasis");
    let a = s.attach(claim(2, h)).unwrap();
    assert_eq!(kinds(&batch(&a.frames[0]))[0], "MapBasis");
    // `resume.basis` counts the same way (a resume of an old stream is a Fresh).
    let resume = Resume {
        stream: 0,
        last_seq: 0,
        basis: Some(BasisClaim { game: 1, hash: h }),
    };
    let a = s
        .attach(AttachArgs {
            resume: Some(resume),
            ..args()
        })
        .unwrap();
    assert_eq!(a.mode, AttachMode::Fresh);
    assert_eq!(kinds(&batch(&a.frames[0]))[0], "Keyframe");
    assert_eq!(get(&stats.basis_skipped), 2);
}

#[test]
fn a_restart_with_the_same_basis_sends_no_map_basis_and_a_new_one_does() {
    let (mut s, stats, v) = started();
    s.attach(args()).unwrap();
    let sent = get(&stats.basis_sent);
    // The worker dies; the new one sends the same basis and a `Full`: a keyframe, no MapBasis.
    s.worker_lost();
    assert!(s.waits_for_full());
    let out = s.step(
        Parts {
            basis: Some(basis(W, H)),
            view: Some(full(1, &v)),
            ..Parts::default()
        },
        0,
    );
    assert_eq!(kinds(&batch(&out.frame.unwrap())), ["Keyframe"]);
    assert_eq!(get(&stats.basis_sent), sent);
    // Another worker with another basis: the MapBasis goes before the keyframe.
    s.worker_lost();
    let mut other = basis(W, H);
    other.plots[1].terrain = 6;
    other.basis_hash = basis_hash_of(&other);
    let out = s.step(
        Parts {
            basis: Some(other.clone()),
            view: Some(full(1, &v)),
            ..Parts::default()
        },
        0,
    );
    assert_eq!(kinds(&batch(&out.frame.unwrap())), ["MapBasis", "Keyframe"]);
    assert_eq!(s.sent_basis_hash(), Some(other.basis_hash));
}

// ---------------------------------------------------------------- contexts

fn unit_ctx(unit: i32) -> ContextMsg {
    ContextMsg::Unit(UnitContext {
        unit,
        view_seq: 0,
        ..UnitContext::default()
    })
}

#[test]
fn a_context_of_an_old_revision_is_dropped_and_the_rest_get_the_view_seq() {
    let (mut s, stats, v) = started();
    s.attach(args()).unwrap();
    let v2 = with_unit(&v, 1);
    let out = s.step(
        Parts {
            view: Some(delta(1, 2, &v, &v2)),
            contexts: vec![(1, unit_ctx(10)), (2, unit_ctx(11))],
            ..Parts::default()
        },
        0,
    );
    let b = batch(&out.frame.unwrap());
    assert_eq!(kinds(&b), ["Diff", "Context"]);
    let GameMsg::Context(ContextMsg::Unit(u)) = &b.msgs[1] else {
        unreachable!()
    };
    assert_eq!(u.unit, 11);
    assert_eq!(
        u.view_seq, b.seq,
        "the batch with the view of that revision"
    );
    assert_eq!(get(&stats.contexts_dropped), 1);
    // A context that comes later for the same revision gets the same `view_seq`.
    let out = s.step(
        Parts {
            contexts: vec![(2, unit_ctx(12))],
            ..Parts::default()
        },
        0,
    );
    let b2 = batch(&out.frame.unwrap());
    let GameMsg::Context(ContextMsg::Unit(u)) = &b2.msgs[0] else {
        unreachable!()
    };
    assert_eq!(u.view_seq, b.seq);
    // A Fresh carries the cached contexts of the current revision (a reloaded tab needs no query).
    let a = s.attach(args()).unwrap();
    let fb = batch(&a.frames[0]);
    assert!(kinds(&fb).contains(&"Context"));
    let ctxs = fb
        .msgs
        .iter()
        .filter(|m| matches!(m, GameMsg::Context(_)))
        .count();
    assert_eq!(ctxs, 2);
}

// ---------------------------------------------------------------- ring, replay, NeedKeyframe

fn feed_small_diffs(s: &mut SeatStream, from: &WorldView, n: u32) -> WorldView {
    let mut cur = from.clone();
    for i in 0..n {
        let next = with_unit(&cur, i as i32 + 100);
        let rev = s.rev();
        s.step(parts_view(delta(rev, rev + 1, &cur, &next)), 0);
        cur = next;
    }
    cur
}

#[test]
fn replay_sends_the_tail_of_the_ring_as_it_was() {
    let (mut s, stats, v) = started();
    let a = s.attach(args()).unwrap();
    let ServerMsg::Attached(att) = &a.attached else {
        unreachable!()
    };
    let (stream, first_seq) = (att.stream, 1);
    let v3 = feed_small_diffs(&mut s, &v, 3);
    assert_eq!(s.seq(), 4);
    // The client saw batches up to 2.
    let r = s
        .attach(AttachArgs {
            resume: Some(Resume {
                stream,
                last_seq: first_seq + 1,
                basis: None,
            }),
            ..args()
        })
        .unwrap();
    assert_eq!(r.mode, AttachMode::Replay);
    let seqs: Vec<u64> = r.frames.iter().map(|f| batch(f).seq).collect();
    assert_eq!(seqs, [3, 4]);
    let ServerMsg::Attached(att2) = &r.attached else {
        unreachable!()
    };
    assert_eq!(att2.stream, stream, "the stream goes on");
    assert_eq!(get(&stats.resume_replay), 1);
    // A client that is up to date gets `Replay` with nothing after it.
    let r = s
        .attach(AttachArgs {
            resume: Some(Resume {
                stream,
                last_seq: 4,
                basis: None,
            }),
            ..args()
        })
        .unwrap();
    assert_eq!((r.mode, r.frames.len()), (AttachMode::Replay, 0));
    let _ = v3;
}

#[test]
fn a_resume_of_another_stream_or_of_the_future_is_a_fresh() {
    let (mut s, _stats, _v) = started();
    let stream = s.stream_id();
    for resume in [
        Resume {
            stream: stream ^ 1,
            last_seq: 1,
            basis: None,
        },
        Resume {
            stream,
            last_seq: 99,
            basis: None,
        },
    ] {
        let r = s
            .attach(AttachArgs {
                resume: Some(resume),
                ..args()
            })
            .unwrap();
        assert_eq!(r.mode, AttachMode::Fresh);
    }
}

#[test]
fn a_ring_overflow_turns_the_resume_into_a_fresh() {
    let (mut s, stats, v) = started();
    let a = s.attach(args()).unwrap();
    let ServerMsg::Attached(att) = &a.attached else {
        unreachable!()
    };
    let stream = att.stream;
    // More batches than the ring holds while nobody is connected.
    let n = RING_BATCHES as u32 + 10;
    for i in 0..n {
        s.step(
            Parts {
                seats: Some(vec![protocol::SeatView {
                    player: 0,
                    control: protocol::SeatControl::Ai { kicked: None },
                    conn: protocol::Conn::Lost {
                        since_ms: u64::from(i),
                    },
                    host: false,
                }]),
                ..Parts::default()
            },
            0,
        );
    }
    assert_eq!(s.ring().len(), RING_BATCHES);
    assert!(get(&stats.ring_overflow_total) > 0);
    // Batch 2 is gone: the client's `last_seq = 1` cannot be served.
    let r = s
        .attach(AttachArgs {
            resume: Some(Resume {
                stream,
                last_seq: 1,
                basis: None,
            }),
            ..args()
        })
        .unwrap();
    assert_eq!(r.mode, AttachMode::Fresh);
    let _ = v;
}

#[test]
fn need_keyframe_sends_a_keyframe_and_for_a_basis_mismatch_the_basis_first() {
    let (mut s, stats, _v) = started();
    s.attach(args()).unwrap();
    let f = s.need_keyframe(KeyframeReason::HashMismatch).unwrap();
    assert_eq!(kinds(&batch(&f)), ["Keyframe"]);
    let f = s.need_keyframe(KeyframeReason::BasisMismatch).unwrap();
    assert_eq!(kinds(&batch(&f)), ["MapBasis", "Keyframe"]);
    assert_eq!(get(&stats.need_keyframe[0]), 1);
    assert_eq!(get(&stats.need_keyframe[4]), 1);
    // The keyframe is of the current view: the next diff is to this batch.
    assert_eq!(s.view_seq(), s.seq());
}

#[test]
fn the_stream_goes_on_without_a_socket_and_counts_the_bytes() {
    let (mut s, stats, v) = started();
    let before = get(&stats.tx_batches);
    feed_small_diffs(&mut s, &v, 5);
    assert_eq!(get(&stats.tx_batches), before + 5);
    assert!(get(&stats.tx_body_bytes) > 0 && get(&stats.tx_raw_bytes) > 0);
    assert!(get(&stats.keyframe_bytes_max) > 0);
}

// ================================================================ a game on the mock worker

use std::time::Duration;

use common::{mock_server_config, temp_dir};
use protocol::{
    AutoChoices, CivPick, Cmd, Command, CommandOutcome, GameSettings, GameSetup, RejectCode,
    SlotControl, SlotSetup, TimerMode, TurnMode,
};
use server::game::{
    ActorMsg, AdminCtl, AttachRequest, ClientGameMsg, GameHandle, GameInit, SessionLink, link_of,
};
use server::session::outbox::{self, OutboxRx};
use server::{ServerConfig, Shared};

const T: Duration = Duration::from_secs(10);

fn settings() -> GameSettings {
    GameSettings {
        name: "mock".into(),
        world_size: "WORLDSIZE_DUEL".into(),
        map_script: "m".into(),
        map_options: vec![],
        speed: "s".into(),
        start_era: "e".into(),
        handicap: "h".into(),
        minor_civs: 0,
        turn_mode: TurnMode::Hybrid,
        timer: TimerMode::Off,
        game_options: vec![],
        seed: Some(1),
        map_seed: None,
        auto_choices: AutoChoices::default(),
        diplo_auto_decline: true,
    }
}

struct World {
    shared: Arc<Shared>,
    game: GameHandle,
    dir: std::path::PathBuf,
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

const QUICK: [Duration; 3] = [
    Duration::ZERO,
    Duration::from_millis(50),
    Duration::from_millis(100),
];

async fn world(name: &str, mode: &str, humans: &[&str], delays: [Duration; 3]) -> World {
    let dir = temp_dir(name);
    let config: ServerConfig = mock_server_config(&dir, mode, Duration::from_secs(20), delays);
    let shared = Shared::new(config).unwrap();
    shared.ensure_actors();
    let slots = humans
        .iter()
        .enumerate()
        .map(|(i, n)| SlotSetup {
            player: i as i8,
            civ: CivPick::Random,
            control: SlotControl::Human { nick: (*n).into() },
        })
        .collect();
    let game = shared
        .start_game(
            GameInit {
                id: 1,
                setup: GameSetup {
                    settings: settings(),
                    slots,
                },
                host: humans[0].into(),
                room: None,
                seed: 1,
                load: None,
                kicked: Default::default(),
                created_ms: None,
            },
            shared.choices(),
        )
        .unwrap();
    World { shared, game, dir }
}

/// One fake connection: the queue a session would write to the socket, and its link.
struct Conn {
    rx: OutboxRx,
    link: SessionLink,
    session: u64,
    player: i8,
    game: GameHandle,
}

impl Conn {
    fn attach(
        game: &GameHandle,
        player: i8,
        nick: &str,
        session: u64,
        resume: Option<Resume>,
        queue: (usize, usize),
    ) -> Conn {
        let (out, rx) = outbox::channel(queue.0, queue.1);
        let link = SessionLink::default();
        assert!(game.send(ActorMsg::Attach(AttachRequest {
            player,
            nick: nick.into(),
            session,
            out,
            link: link.clone(),
            resume,
            claim: None,
        })));
        Conn {
            rx,
            link,
            session,
            player,
            game: game.clone(),
        }
    }

    fn new(game: &GameHandle, player: i8, nick: &str, session: u64) -> Conn {
        Self::attach(game, player, nick, session, None, (256, 4 << 20))
    }

    async fn next(&mut self) -> ServerMsg {
        let frame = tokio::time::timeout(T, self.rx.recv())
            .await
            .expect("a message within 10 s")
            .expect("the queue is open");
        decode(&frame)
    }

    /// The next batch (other messages are skipped).
    async fn batch(&mut self) -> GameBatch {
        loop {
            if let ServerMsg::Game(b) = self.next().await {
                return b;
            }
        }
    }

    /// Batches until one satisfies `pred`.
    async fn until(&mut self, what: &str, pred: impl Fn(&GameBatch) -> bool) -> GameBatch {
        for _ in 0..400 {
            let b = self.batch().await;
            if pred(&b) {
                return b;
            }
        }
        panic!("no batch with {what}");
    }

    fn send(&self, msg: ClientGameMsg) {
        self.game.send(ActorMsg::Client {
            bytes: 0,
            player: self.player,
            session: self.session,
            msg,
        });
    }

    fn cmd(&self, id: u32) {
        self.send(ClientGameMsg::Cmd(Cmd {
            id,
            turn: 1,
            cmd: Command::CancelEndTurn,
        }));
    }

    fn linked(&self) -> bool {
        link_of(&self.link).is_some()
    }
}

fn results(b: &GameBatch) -> Vec<(u32, CommandOutcome)> {
    b.msgs
        .iter()
        .filter_map(|m| match m {
            GameMsg::Results(r) => Some(r),
            _ => None,
        })
        .flatten()
        .map(|r| (r.id, r.outcome.clone()))
        .collect()
}

fn has_result(id: u32) -> impl Fn(&GameBatch) -> bool {
    move |b| results(b).iter().any(|(i, _)| *i == id)
}

/// Attaches seat 0 as alice and reads `Attached` and the first batch.
async fn alice(w: &World) -> (Conn, Attached, GameBatch) {
    let mut c = Conn::new(&w.game, 0, "alice", 1);
    let ServerMsg::Attached(a) = c.next().await else {
        panic!("Attached first");
    };
    let b = c.batch().await;
    (c, a, b)
}

#[tokio::test]
async fn a_seat_gets_attached_fresh_and_the_first_batch_in_order() {
    let w = world("fresh", "echo", &["alice", "bob"], QUICK).await;
    let (c, a, b) = alice(&w).await;
    assert!(c.linked(), "the session can reach its game now");
    assert_eq!(
        (a.game, a.player, a.mode, a.last_cmd_id),
        (1, 0, AttachMode::Fresh, 0)
    );
    assert_eq!(a.catalog.len, 6 + 12, "C5CT, version, the mock catalog");
    assert_eq!(b.seq, 1);
    let k = kinds(&b);
    assert_eq!(k[0], "MapBasis");
    assert_eq!(k[1], "Keyframe");
    assert!(k.contains(&"TurnState") && k.contains(&"Seats"), "{k:?}");
    // The order of NP 4.4: TurnState before Seats.
    let pos = |n| k.iter().position(|x| *x == n).unwrap();
    assert!(pos("TurnState") < pos("Seats"));
    let seats = b
        .msgs
        .iter()
        .find_map(|m| match m {
            GameMsg::Seats(s) => Some(s.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(seats.len(), 2);
    assert!(matches!(seats[0].conn, protocol::Conn::Online));
    assert!(matches!(seats[1].conn, protocol::Conn::None));
    assert!(seats[0].host && !seats[1].host);
}

#[tokio::test]
async fn an_order_is_answered_after_its_view_and_a_repeat_gets_the_same_answer() {
    let w = world("orders", "echo", &["alice"], QUICK).await;
    let (mut c, _a, _first) = alice(&w).await;
    c.cmd(1);
    let b = c.until("the answer to 1", has_result(1)).await;
    let k = kinds(&b);
    // The view with the consequence comes before the answer (NP-D11).
    let pos = |n| k.iter().position(|x| *x == n);
    assert!(pos("Diff") < pos("Results"), "{k:?}");
    assert_eq!(results(&b), [(1, CommandOutcome::Applied)]);
    // The same id again: the answer is repeated, the worker is not asked.
    let applied_before = get(&w.game.info.stats.applied);
    c.cmd(1);
    let again = c.until("the repeat", has_result(1)).await;
    assert_eq!(results(&again), [(1, CommandOutcome::Applied)]);
    assert!(!kinds(&again).contains(&"Diff"));
    assert_eq!(get(&w.game.info.stats.applied), applied_before);
    assert_eq!(get(&w.game.info.stats.dup_answered), 1);
    // A gap in the numbers is no error.
    c.cmd(5);
    c.until("the answer to 5", has_result(5)).await;
    assert_eq!(get(&w.game.info.stats.id_gap), 1);
}

#[tokio::test]
async fn a_repeat_of_an_order_the_worker_still_has_is_not_answered_twice() {
    // The slow worker takes 400 ms per frame.
    let w = world("pending", "slow ms=400", &["alice"], QUICK).await;
    let (mut c, _a, _b) = alice(&w).await;
    c.cmd(1);
    c.cmd(1);
    c.cmd(1);
    let b = c.until("the answer to 1", has_result(1)).await;
    assert_eq!(results(&b).len(), 1);
    // And nothing more arrives for it.
    let mut extra = 0;
    while let Ok(Some(f)) = tokio::time::timeout(Duration::from_millis(900), c.rx.recv()).await {
        if let ServerMsg::Game(b) = decode(&f) {
            extra += results(&b).len();
        }
    }
    assert_eq!(extra, 0);
    assert_eq!(get(&w.game.info.stats.dup_pending_dropped), 2);
    assert_eq!(get(&w.game.info.stats.forwarded), 1);
}

#[tokio::test]
async fn an_id_older_than_the_answer_ring_expires() {
    let w = world("expired", "echo", &["alice"], QUICK).await;
    let (mut c, _a, _b) = alice(&w).await;
    for id in 1..=70 {
        c.cmd(id);
        c.until("an answer", has_result(id)).await;
    }
    c.cmd(3);
    let b = c.until("the answer to 3", has_result(3)).await;
    assert!(matches!(
        results(&b)[0].1,
        CommandOutcome::Rejected {
            code: RejectCode::Expired,
            ..
        }
    ));
    c.cmd(70);
    let b = c.until("the repeat of 70", has_result(70)).await;
    assert_eq!(results(&b), [(70, CommandOutcome::Applied)]);
}

#[tokio::test]
async fn too_many_orders_get_too_many_and_every_id_one_answer() {
    let w = world("limit", "echo", &["alice"], QUICK).await;
    let (mut c, _a, _b) = alice(&w).await;
    for id in 1..=300 {
        c.cmd(id);
    }
    let mut seen = std::collections::BTreeMap::new();
    while seen.len() < 300 {
        let b = c.batch().await;
        for (id, o) in results(&b) {
            assert!(seen.insert(id, o).is_none(), "id {id} answered twice");
        }
    }
    let too_many = seen
        .values()
        .filter(|o| {
            matches!(
                o,
                CommandOutcome::Rejected {
                    code: RejectCode::TooMany,
                    ..
                }
            )
        })
        .count();
    assert!(
        too_many > 0,
        "the burst of 200 or the window of 64 must show"
    );
}

#[tokio::test]
async fn a_query_is_answered_with_its_own_id_and_focus_is_accepted() {
    let w = world("query", "echo", &["alice"], QUICK).await;
    let (mut c, _a, _b) = alice(&w).await;
    c.send(ClientGameMsg::Focus(protocol::Focus {
        units: vec![1],
        city: None,
    }));
    c.send(ClientGameMsg::Query {
        id: 77,
        q: protocol::Query::Units,
    });
    loop {
        if let ServerMsg::QueryResult { id, .. } = c.next().await {
            assert_eq!(id, 77);
            break;
        }
    }
    assert_eq!(get(&w.game.info.stats.queries), 1);
}

#[tokio::test]
async fn need_keyframe_from_the_client_gets_a_keyframe_and_the_basis_on_a_mismatch() {
    let w = world("needkf", "echo", &["alice", "bob"], QUICK).await;
    let (mut c, _a, _b) = alice(&w).await;
    c.send(ClientGameMsg::NeedKeyframe(KeyframeReason::BasisMismatch));
    let b = c
        .until("a keyframe", |b| kinds(b).contains(&"Keyframe"))
        .await;
    assert_eq!(&kinds(&b)[..2], ["MapBasis", "Keyframe"]);
    c.send(ClientGameMsg::NeedKeyframe(KeyframeReason::HashMismatch));
    let b = c
        .until("a keyframe", |b| kinds(b).contains(&"Keyframe"))
        .await;
    assert_eq!(kinds(&b)[0], "Keyframe");
}

#[tokio::test]
async fn a_dropped_connection_resumes_from_the_ring_and_an_unknown_stream_is_fresh() {
    let w = world("replay", "echo", &["alice"], QUICK).await;
    let (mut c, a, _b) = alice(&w).await;
    c.cmd(1);
    c.until("1", has_result(1)).await;
    // The connection is gone (the session ended).
    let tx = link_of(&c.link).unwrap().tx;
    tx.send(ActorMsg::SessionGone {
        player: 0,
        session: 1,
    })
    .unwrap();
    // A second connection of the seat starts a Fresh stream with a new number.
    let mut d = Conn::new(&w.game, 0, "alice", 2);
    let ServerMsg::Attached(second) = d.next().await else {
        panic!()
    };
    assert_eq!(second.mode, AttachMode::Fresh);
    assert_ne!(second.stream, a.stream);
    assert_eq!(second.last_cmd_id, 1);
    d.cmd(2);
    let b2 = d.until("2", has_result(2)).await;
    // `d` drops too; a third connection resumes `d`'s stream after the batch before the answer.
    tx.send(ActorMsg::SessionGone {
        player: 0,
        session: 2,
    })
    .unwrap();
    let mut e = Conn::attach(
        &w.game,
        0,
        "alice",
        3,
        Some(Resume {
            stream: second.stream,
            last_seq: b2.seq - 1,
            basis: None,
        }),
        (256, 4 << 20),
    );
    let ServerMsg::Attached(third) = e.next().await else {
        panic!()
    };
    assert_eq!(
        (third.mode, third.stream),
        (AttachMode::Replay, second.stream)
    );
    assert_eq!(e.batch().await.seq, b2.seq, "the tail, as it was");
    // A stream number nobody knows is a Fresh.
    let mut f = Conn::attach(
        &w.game,
        0,
        "alice",
        4,
        Some(Resume {
            stream: second.stream ^ 1,
            last_seq: 1,
            basis: None,
        }),
        (256, 4 << 20),
    );
    let ServerMsg::Attached(fourth) = f.next().await else {
        panic!()
    };
    assert_eq!(fourth.mode, AttachMode::Fresh);
}

#[tokio::test]
async fn a_session_whose_queue_overflows_is_lost_and_the_stream_goes_on() {
    let w = world("overflow", "echo", &["alice", "bob"], QUICK).await;
    // A queue of 3 messages that nobody reads.
    let c = Conn::attach(&w.game, 0, "alice", 1, None, (3, 4 << 20));
    // Orders reach the game once the seat is taken, which waits for the worker to be ready.
    while !c.linked() {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    for id in 1..=20 {
        c.cmd(id);
    }
    let deadline = std::time::Instant::now() + T;
    loop {
        let seats = w.game.info.snapshot().seats;
        if seats.first().is_some_and(|s| s.conn == "lost") {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the seat never got Lost: {seats:?}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    // The seat still builds batches into its ring, and a new connection gets the state.
    let mut d = Conn::new(&w.game, 0, "alice", 2);
    assert!(matches!(d.next().await, ServerMsg::Attached(_)));
}

#[tokio::test]
async fn a_worker_crash_gives_lost_then_restarting_then_a_keyframe_in_the_same_stream() {
    let dir = temp_dir("crash-marker");
    let marker = dir.join("m").to_string_lossy().into_owned();
    // The second frame after Init (the first is the save order) kills the worker, once.
    let mode = format!("crash-after n=2 once={marker}");
    let w = world(
        "crash",
        &mode,
        &["alice"],
        [Duration::from_millis(400), Duration::ZERO, Duration::ZERO],
    )
    .await;
    let (mut c, _a, _b) = alice(&w).await;
    // A save first: a restart needs one.
    w.game.send(ActorMsg::Admin(AdminCtl::Save));
    let deadline = std::time::Instant::now() + T;
    while get(&w.game.info.stats.saves_ok) == 0 {
        assert!(std::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    c.cmd(1);
    // The worker dies on it: `Lost`.
    let b = c.until("Lost for 1", has_result(1)).await;
    assert_eq!(results(&b), [(1, CommandOutcome::Lost)]);
    // While the next worker is on its way, orders are refused with `Restarting`.
    c.cmd(2);
    let b = c.until("an answer to 2", has_result(2)).await;
    assert!(matches!(
        results(&b)[0].1,
        CommandOutcome::Rejected {
            code: RejectCode::Restarting,
            ..
        }
    ));
    // The restart line in the journal (Russian, AC-N1.5) and a keyframe in the same stream; the
    // basis is the same, so no MapBasis.
    let mut journal = None;
    let kf = loop {
        let b = c.batch().await;
        for m in &b.msgs {
            if let GameMsg::Journal(j) = m {
                journal = Some(j[0].text.clone());
            }
        }
        if kinds(&b).contains(&"Keyframe") {
            break b;
        }
    };
    assert_eq!(kinds(&kf)[0], "Keyframe", "no MapBasis for the same basis");
    // The order that was in flight died with the worker (`Lost`); none was applied after the save, so
    // the restart line, which reports rolled-back orders only, is not sent.
    assert_eq!(journal, None);
    c.cmd(3);
    let b = c.until("an answer to 3", has_result(3)).await;
    assert_eq!(results(&b), [(3, CommandOutcome::Applied)]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_game_that_cannot_restart_detaches_its_seats_and_loses_the_pending_orders() {
    // No save: the first crash is final.
    let w = world("faulted", "crash-after n=1", &["alice"], QUICK).await;
    let (mut c, _a, _b) = alice(&w).await;
    c.cmd(1);
    let mut lost = false;
    let mut detached = false;
    for _ in 0..50 {
        match c.next().await {
            ServerMsg::Game(b) => {
                lost |= results(&b).contains(&(1, CommandOutcome::Lost));
            }
            ServerMsg::Detached {
                reason: protocol::DetachReason::GameStopped { .. },
            } => {
                detached = true;
                break;
            }
            _ => {}
        }
    }
    assert!(lost, "the order gets Lost before the seat is let go");
    assert!(detached);
    assert!(!c.linked());
    // The game stays in the registry as `Faulted` (an administrator may restart it); it holds no slot of
    // `max_games` and is not in the lobby list.
    let deadline = std::time::Instant::now() + T;
    loop {
        let faulted = w
            .shared
            .games
            .get(1)
            .is_some_and(|g| g.info.snapshot().state == server::supervisor::GameState::Faulted);
        if faulted {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(w.shared.games.running(), 0);
    assert!(w.shared.games.get(1).unwrap().info.status().is_none());
}
