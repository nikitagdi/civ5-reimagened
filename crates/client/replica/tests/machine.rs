//! The state machine of the replica on a fake engine: order by `jseq`, gaps, epochs, batches before the body,
//! batches under the snapshot's point, divergence and resync, turn changes (snapshot), the CHECK policy, the router
//! of the UI's messages.

mod common;

use common::*;
use gamecore::replica::{JournalRecord, NetMessage, decode_net};
use replica::{GameSide, GcResyncReason, Outgoing, ReplicaConfig, ReplicaStep, State};

fn turns_of(l: &Log) -> Vec<i32> {
    l.applied
        .iter()
        .flat_map(|b| b.iter().filter_map(JournalRecord::turn_after))
        .collect()
}

#[test]
fn a_join_waits_for_the_body_then_loads_in_two_stages() {
    let mut g = rig(ReplicaConfig::default());
    let s = snap(1);
    assert_eq!(g.r.state(), State::Empty);
    g.r.on_msg(base(1, 5, 0, &s));
    assert_eq!(g.r.state(), State::WaitBlob);
    assert_eq!(g.r.pump(10), ReplicaStep::Idle);
    // The body is asked for once.
    assert_eq!(g.r.wanted_blobs(), vec![s.blob]);
    assert!(g.r.wanted_blobs().is_empty());
    g.r.blob(s.blob.hash, s.body.clone());
    assert_eq!(g.r.pump(10), ReplicaStep::Working);
    assert_eq!(g.r.state(), State::Loading);
    assert_eq!(g.r.pump(10), ReplicaStep::Loaded { turn: 5 });
    assert_eq!(g.r.state(), State::Live);
    assert_eq!(g.r.generation(), 1);
    assert_eq!(g.log.borrow().loads, vec![s.raw.clone()]);
    assert_eq!(g.r.stats().loads, 1);
    assert_eq!(g.r.pump(10), ReplicaStep::Idle);
}

#[test]
fn batches_are_applied_in_jseq_order() {
    let mut g = rig(cfg_always());
    join(&mut g, 1, 5, 10, &snap(1));
    g.r.on_msg(journal(1, 11, vec![pump_batch(5), pump_batch(6)]));
    g.r.on_msg(journal(1, 13, vec![pump_batch(7)]));
    assert_eq!(g.r.pump(1000), ReplicaStep::Applied { jseq: 13 });
    assert_eq!(turns_of(&g.log.borrow()), vec![5, 6, 7]);
    assert_eq!(g.r.applied_jseq(), 13);
    assert_eq!(g.r.stats().applied, 3);
    assert_eq!(g.r.pump(1000), ReplicaStep::Idle);
}

#[test]
fn a_repeated_batch_is_applied_once() {
    let mut g = rig(cfg_always());
    join(&mut g, 1, 5, 0, &snap(1));
    g.r.on_msg(journal(1, 1, vec![pump_batch(5)]));
    pump_all(&mut g);
    // A replay of the same batches (the ring of the gateway) changes nothing.
    g.r.on_msg(journal(1, 1, vec![pump_batch(5), pump_batch(6)]));
    pump_all(&mut g);
    assert_eq!(turns_of(&g.log.borrow()), vec![5, 6]);
    assert!(g.r.stats().stale_batches >= 1);
}

#[test]
fn a_hole_in_the_numbers_asks_for_a_new_base() {
    let mut g = rig(cfg_always());
    join(&mut g, 1, 5, 0, &snap(1));
    g.r.on_msg(journal(1, 1, vec![pump_batch(5)]));
    g.r.on_msg(journal(1, 3, vec![pump_batch(7)]));
    assert_eq!(g.r.state(), State::Resyncing);
    let out = g.r.take_outgoing();
    let ours = snap(1).raw_hash;
    assert_eq!(
        out,
        vec![Outgoing::Resync {
            epoch: 1,
            have_raw_hash: Some(ours),
            at_jseq: 0,
            reason: GcResyncReason::Gap
        }]
    );
    // Asked once, not on every batch or pump.
    g.r.on_msg(journal(1, 4, vec![pump_batch(8)]));
    assert!(g.r.take_outgoing().is_empty());
    assert_eq!(g.r.pump(10), ReplicaStep::Resyncing);
    assert_eq!(g.r.stats().resyncs, 1);
    // The game is left as it was: nothing applied.
    assert!(g.log.borrow().applied.is_empty());
}

#[test]
fn a_journal_of_another_epoch_is_not_ours() {
    let mut g = rig(cfg_always());
    join(&mut g, 7, 5, 0, &snap(1));
    g.r.on_msg(journal(6, 1, vec![pump_batch(5)]));
    g.r.on_msg(journal(8, 1, vec![pump_batch(5)]));
    g.r.on_msg(turn_msg(6, 6, 4, &snap(2)));
    assert_eq!(g.r.pump(10), ReplicaStep::Idle);
    assert_eq!(g.r.stats().stale_epoch, 3);
    assert!(g.log.borrow().applied.is_empty());
    assert!(g.r.wanted_blobs().is_empty());
}

#[test]
fn batches_before_the_body_wait_and_those_under_the_snapshot_go() {
    let mut g = rig(cfg_always());
    let s = snap(1);
    // The journal comes before the base's body (and even before the base).
    g.r.on_msg(journal(
        1,
        4,
        vec![pump_batch(1), pump_batch(2), pump_batch(3), pump_batch(4)],
    ));
    g.r.on_msg(base(1, 5, 5, &s));
    assert_eq!(g.r.state(), State::WaitBlob);
    g.r.on_msg(journal(1, 8, vec![pump_batch(8)]));
    assert_eq!(g.r.pump(10), ReplicaStep::Idle);
    assert!(g.log.borrow().applied.is_empty(), "nothing before the load");
    let want = g.r.wanted_blobs();
    g.r.blob(want[0].hash, s.body.clone());
    finish_load(&mut g);
    pump_all(&mut g);
    // 4 and 5 are in the snapshot; 6 and 7 are the early journal's, 8 follows.
    assert_eq!(turns_of(&g.log.borrow()), vec![3, 4, 8]);
    assert_eq!(g.r.applied_jseq(), 8);
}

#[test]
fn a_failed_check_stops_the_journal_and_a_new_base_resumes_it() {
    let mut g = rig(cfg_always());
    join(&mut g, 1, 5, 0, &snap(1));
    g.log.borrow_mut().diverge_on_call = Some(1);
    g.r.on_msg(journal(
        1,
        1,
        vec![pump_batch(5), checked_batch(6), pump_batch(7)],
    ));
    assert_eq!(g.r.pump(1000), ReplicaStep::Resyncing);
    assert_eq!(g.r.state(), State::Resyncing);
    assert_eq!(g.r.stats().diverged, 1);
    assert_eq!(turns_of(&g.log.borrow()), vec![5]);
    let out = g.r.take_outgoing();
    assert!(matches!(
        out[..],
        [Outgoing::Resync {
            reason: GcResyncReason::Diverged,
            at_jseq: 1,
            ..
        }]
    ));
    // More batches come while the request is out: they are kept, not applied.
    g.r.on_msg(journal(1, 4, vec![pump_batch(8), pump_batch(9)]));
    assert_eq!(g.r.pump(1000), ReplicaStep::Resyncing);
    // The answer: a base at jseq 3 (it holds batch 3 and the ones before).
    let fresh = snap(2);
    g.r.on_msg(base(1, 7, 3, &fresh));
    assert_eq!(g.r.state(), State::WaitBlob);
    let want = g.r.wanted_blobs();
    g.r.blob(want[0].hash, fresh.body.clone());
    assert_eq!(finish_load(&mut g), ReplicaStep::Loaded { turn: 7 });
    assert_eq!(g.r.generation(), 2);
    assert_eq!(g.log.borrow().loads.len(), 2);
    pump_all(&mut g);
    assert_eq!(turns_of(&g.log.borrow()), vec![5, 8, 9]);
    assert_eq!(g.r.applied_jseq(), 5);
}

#[test]
fn a_refused_record_is_a_resync_too() {
    let mut g = rig(cfg_always());
    join(&mut g, 1, 5, 0, &snap(1));
    g.log.borrow_mut().fail_on_call = Some(0);
    g.r.on_msg(journal(1, 1, vec![pump_batch(5)]));
    assert_eq!(g.r.pump(10), ReplicaStep::Resyncing);
    assert!(matches!(
        g.r.take_outgoing()[..],
        [Outgoing::Resync {
            reason: GcResyncReason::Rejected,
            ..
        }]
    ));
}

#[test]
fn a_resync_is_asked_again_when_unanswered_or_not_sent() {
    let mut g = rig(cfg_always());
    join(&mut g, 1, 5, 0, &snap(1));
    g.r.on_msg(journal(1, 2, vec![pump_batch(5)]));
    let out = g.r.take_outgoing();
    assert_eq!(out.len(), 1);
    // The shell had no connection: it gives the request back and it is made again on the next pump.
    g.r.unsend(out);
    assert_eq!(g.r.stats().resyncs, 0);
    assert_eq!(g.r.pump(10), ReplicaStep::Resyncing);
    assert_eq!(g.r.take_outgoing().len(), 1);
    assert_eq!(g.r.stats().resyncs, 1);
    // Sent, but no answer for resync_retry_ms.
    g.clock.0.set(5_000);
    g.r.pump(10);
    assert!(g.r.take_outgoing().is_empty());
    g.clock.0.set(10_001);
    g.r.pump(10);
    assert_eq!(g.r.take_outgoing().len(), 1);
    assert_eq!(g.r.stats().resyncs, 2);
}

#[test]
fn a_turn_change_loads_the_snapshot_and_drops_what_it_holds() {
    let mut g = rig(cfg_always());
    join(&mut g, 1, 5, 0, &snap(1));
    g.r.on_msg(journal(1, 1, vec![pump_batch(5), pump_batch(5)]));
    pump_all(&mut g);
    let g0 = g.r.generation();
    let next = snap(2);
    // Batches above the turn's point arrive while the body is on its way.
    g.r.on_msg(turn_msg(1, 6, 2, &next));
    g.r.on_msg(journal(1, 3, vec![pump_batch(6)]));
    assert!(g.r.switching());
    assert_eq!(g.r.state(), State::Live);
    assert_eq!(g.r.pump(10), ReplicaStep::Idle, "waits for the body");
    let want = g.r.wanted_blobs();
    assert_eq!(want, vec![next.blob]);
    g.r.blob(next.blob.hash, next.body.clone());
    assert_eq!(g.r.pump(10), ReplicaStep::Working);
    assert_eq!(g.r.pump(10), ReplicaStep::TurnSwitched { turn: 6 });
    assert!(!g.r.switching());
    assert_eq!(g.r.generation(), g0 + 1);
    assert_eq!(g.log.borrow().loads.last().unwrap(), &next.raw);
    pump_all(&mut g);
    assert_eq!(g.r.applied_jseq(), 3);
    let st = g.r.stats();
    assert_eq!((st.turn_snapshot, st.turn_patch), (1, 0));
}

#[test]
fn a_turn_change_reports_its_timed_work_as_spans() {
    let mut g = rig(cfg_always());
    join(&mut g, 1, 5, 0, &snap(1));
    let _ = g.r.take_spans();
    let next = snap(2);
    g.clock.0.set(g.clock.0.get() + 100);
    g.r.on_msg(turn_msg(1, 6, 2, &next));
    assert_eq!(g.r.wanted_blobs(), vec![next.blob]);
    g.clock.0.set(g.clock.0.get() + 40);
    g.r.blob(next.blob.hash, next.body.clone());
    assert_eq!(g.r.pump(10), ReplicaStep::Working);
    assert_eq!(g.r.pump(10), ReplicaStep::TurnSwitched { turn: 6 });
    let spans = g.r.take_spans();
    let names: Vec<&str> = spans.iter().map(|s| s.name).collect();
    assert_eq!(names, ["c.turn_named", "c.fetch", "c.unpack", "c.load"]);
    assert!(spans.iter().all(|s| s.turn == 6 && s.turn_change));
    assert!(spans[0].dur_ms < 0.0, "a mark");
    assert!((spans[1].dur_ms - 40.0).abs() < 1e-9, "{:?}", spans[1]);
    let bytes = spans[1].args.iter().find(|a| a.0 == "bytes").unwrap().1;
    assert_eq!(bytes as usize, next.body.len());
    assert_eq!(g.r.stats().last_body_bytes as usize, next.body.len());
    assert!(g.r.take_spans().is_empty(), "taken once");
}

#[test]
fn a_verified_check_is_a_span() {
    let mut g = rig(cfg_always());
    join(&mut g, 1, 5, 0, &snap(1));
    let _ = g.r.take_spans();
    g.r.on_msg(journal(1, 1, vec![checked_batch(5)]));
    pump_all(&mut g);
    let spans = g.r.take_spans();
    assert!(
        spans.iter().any(|s| s.name == "c.check" && !s.turn_change),
        "{spans:?}"
    );
}

#[test]
fn a_turn_change_replaces_batches_not_yet_applied() {
    let mut g = rig(cfg_always());
    join(&mut g, 1, 5, 0, &snap(1));
    // Three batches arrive and the turn change right behind them, before any pump.
    g.r.on_msg(journal(
        1,
        1,
        vec![pump_batch(5), pump_batch(5), pump_batch(5)],
    ));
    let next = snap(2);
    g.r.on_msg(turn_msg(1, 6, 3, &next));
    g.r.wanted_blobs();
    g.r.blob(next.blob.hash, next.body.clone());
    finish_load(&mut g);
    assert!(g.log.borrow().applied.is_empty(), "the snapshot holds them");
    assert_eq!(g.r.applied_jseq(), 3);
}

#[test]
fn the_same_base_again_is_not_loaded_twice() {
    let mut g = rig(cfg_always());
    let s = snap(1);
    join(&mut g, 1, 5, 0, &s);
    g.r.on_msg(journal(1, 1, vec![pump_batch(5)]));
    pump_all(&mut g);
    // A fresh attach of the same turn.
    g.r.on_msg(base(1, 5, 0, &s));
    g.r.on_msg(journal(1, 1, vec![pump_batch(5), pump_batch(6)]));
    assert!(g.r.wanted_blobs().is_empty());
    pump_all(&mut g);
    assert_eq!(g.log.borrow().loads.len(), 1);
    assert_eq!(turns_of(&g.log.borrow()), vec![5, 6]);
    assert_eq!(g.r.stats().bases_skipped, 1);
}

#[test]
fn a_new_epoch_starts_the_chain_over() {
    let mut g = rig(cfg_always());
    join(&mut g, 1, 5, 40, &snap(1));
    g.r.on_msg(journal(1, 41, vec![pump_batch(5)]));
    // The server restarted: new epoch, numbers start again.
    let s2 = snap(3);
    g.r.on_msg(base(2, 3, 0, &s2));
    g.r.on_msg(journal(2, 1, vec![pump_batch(3)]));
    // An old journal still on the wire is not ours.
    g.r.on_msg(journal(1, 42, vec![pump_batch(5)]));
    let want = g.r.wanted_blobs();
    g.r.blob(want[0].hash, s2.body.clone());
    finish_load(&mut g);
    pump_all(&mut g);
    assert_eq!(g.r.epoch(), Some(2));
    assert_eq!(turns_of(&g.log.borrow()), vec![3]);
    assert_eq!(g.r.applied_jseq(), 1);
}

#[test]
fn a_body_that_is_not_the_named_one_is_refused() {
    let mut g = rig(cfg_always());
    let s = snap(1);
    g.r.on_msg(base(1, 5, 0, &s));
    g.r.wanted_blobs();
    // Flipped byte: the hash of the body is the URL's.
    let mut bad = s.body.clone();
    let n = bad.len() - 1;
    bad[n] ^= 1;
    g.r.blob(s.blob.hash, bad);
    assert_eq!(g.r.state(), State::Resyncing);
    assert!(matches!(
        g.r.take_outgoing()[..],
        [Outgoing::Resync {
            reason: GcResyncReason::BadBlob,
            ..
        }]
    ));
}

#[test]
fn a_body_that_cannot_be_unpacked_is_a_resync() {
    let mut g = rig(cfg_always());
    // The reference matches the bytes, but the bytes are not a snapshot body.
    let junk = vec![9u8; 100];
    let s = snap(1);
    let mut b = base(1, 5, 0, &s);
    if let replica::ReplicaMsg::Base(x) = &mut b {
        x.snapshot = blob_of(&junk);
    }
    g.r.on_msg(b);
    let want = g.r.wanted_blobs();
    g.r.blob(want[0].hash, junk);
    assert_eq!(g.r.pump(10), ReplicaStep::Resyncing);
    assert_eq!(g.r.state(), State::Resyncing);
}

#[test]
fn a_body_that_fails_to_load_is_a_resync() {
    let mut g = rig(cfg_always());
    g.log.borrow_mut().fail_load = true;
    let s = snap(1);
    g.r.on_msg(base(1, 5, 0, &s));
    g.r.wanted_blobs();
    g.r.blob(s.blob.hash, s.body.clone());
    g.r.pump(10);
    assert_eq!(g.r.pump(10), ReplicaStep::Resyncing);
    assert_eq!(g.r.generation(), 0);
}

#[test]
fn a_fetch_that_fails_is_tried_again_then_given_up() {
    let mut g = rig(cfg_always());
    let s = snap(1);
    g.r.on_msg(base(1, 5, 0, &s));
    for _ in 0..3 {
        assert_eq!(g.r.wanted_blobs(), vec![s.blob]);
        g.r.blob_failed(s.blob.hash);
    }
    assert_eq!(g.r.state(), State::Resyncing);
}

// ---------------------------------------------------------------- CHECK policy

fn checks_in(l: &Log) -> usize {
    l.applied
        .iter()
        .flatten()
        .filter(|r| matches!(r, JournalRecord::Check { .. }))
        .count()
}

#[test]
fn checks_are_cut_out_unless_they_are_worth_their_cost() {
    let mut g = rig(ReplicaConfig::default());
    join(&mut g, 1, 5, 0, &snap(1));
    g.clock.0.set(100);
    g.r.note_input();
    // Mid-turn, the player is active: skipped. Same game turn as before: not the end of the phase.
    g.r.on_msg(journal(1, 1, vec![checked_batch(5), checked_batch(5)]));
    g.r.on_msg(journal(1, 3, vec![pump_batch(5)]));
    pump_all(&mut g);
    assert_eq!(checks_in(&g.log.borrow()), 0);
    assert_eq!(g.r.stats().checks_skipped, 2);
    assert_eq!(g.log.borrow().applied.len(), 3);
    // The end of the phase: the game turn moves on. Verified.
    g.r.on_msg(journal(1, 4, vec![checked_batch(6)]));
    pump_all(&mut g);
    assert_eq!(checks_in(&g.log.borrow()), 1);
    assert_eq!(g.r.stats().checks_run, 1);
}

#[test]
fn an_idle_player_gets_a_check_but_not_more_than_every_ten_seconds() {
    let mut g = rig(ReplicaConfig::default());
    join(&mut g, 1, 5, 0, &snap(1));
    g.clock.0.set(5_000);
    g.r.on_msg(journal(1, 1, vec![checked_batch(5)]));
    pump_all(&mut g);
    assert_eq!(checks_in(&g.log.borrow()), 1, "idle for 5 s: verified");
    g.clock.0.set(8_000);
    g.r.on_msg(journal(1, 2, vec![checked_batch(5)]));
    pump_all(&mut g);
    assert_eq!(
        checks_in(&g.log.borrow()),
        1,
        "3 s after the last one: skipped"
    );
    g.clock.0.set(16_000);
    g.r.on_msg(journal(1, 3, vec![checked_batch(5)]));
    pump_all(&mut g);
    assert_eq!(checks_in(&g.log.borrow()), 2);
    // Input stops idle checks.
    g.clock.0.set(30_000);
    g.r.note_input();
    g.r.on_msg(journal(1, 4, vec![checked_batch(5)]));
    pump_all(&mut g);
    assert_eq!(checks_in(&g.log.borrow()), 2);
}

#[test]
fn always_means_always_in_tests() {
    let mut g = rig(cfg_always());
    join(&mut g, 1, 5, 0, &snap(1));
    g.r.note_input();
    g.r.on_msg(journal(1, 1, vec![checked_batch(5), checked_batch(5)]));
    pump_all(&mut g);
    assert_eq!(checks_in(&g.log.borrow()), 2);
    assert_eq!(g.r.stats().checks_skipped, 0);
}

#[test]
fn a_batch_that_does_not_decode_is_a_resync() {
    let mut g = rig(cfg_always());
    join(&mut g, 1, 5, 0, &snap(1));
    g.r.on_msg(journal(1, 1, vec![b"junk".to_vec()]));
    assert_eq!(g.r.pump(10), ReplicaStep::Resyncing);
}

// ---------------------------------------------------------------- the router

fn msg(name: &str, args: &[i32]) -> NetMessage {
    NetMessage::new(name, args).unwrap()
}

#[test]
fn the_turn_handshake_goes_as_commands_and_the_rest_as_gcnet() {
    let mut g = rig(cfg_always());
    join(&mut g, 1, 5, 0, &snap(1));
    g.r.post_net(msg("sendTurnComplete", &[0]));
    g.r.post_net(msg("sendTurnUnready", &[0]));
    g.r.post_net(msg("SendRenameCity", &[3, 0]));
    g.r.post_net(msg("sendDoTask", &[1, 2, 3]));
    let out = g.r.take_outgoing();
    assert_eq!(out.len(), 4);
    assert_eq!(out[0], Outgoing::EndTurn);
    assert_eq!(out[1], Outgoing::CancelEndTurn);
    let Outgoing::GcNet(bytes) = &out[2] else {
        panic!("{:?}", out[2])
    };
    let m = decode_net(bytes).unwrap();
    assert_eq!(m.len(), 1);
    assert_eq!(m[0].name(), "SendRenameCity");
    assert!(matches!(&out[3], Outgoing::GcNet(_)));
    assert!(g.r.take_outgoing().is_empty());
    let st = g.r.stats();
    assert_eq!(st.net_sent, 4);
}

/// The exe's `BroadcastGameplayMessage`: a gameplay message while the end of turn is out takes it back first (a unit
/// moved after END TURN reopens the turn), once; the UI hears of it through `take_turn_reopened`.
#[test]
fn an_order_after_the_end_of_turn_takes_it_back_first() {
    let mut g = rig(cfg_always());
    join(&mut g, 1, 5, 0, &snap(1));
    g.r.post_net(msg("sendTurnComplete", &[0]));
    assert!(!g.r.take_turn_reopened());
    g.log
        .borrow_mut()
        .outbox
        .push(msg("sendPushMission", &[7, 1, 3, 4]));
    g.r.pump(10);
    g.r.post_net(msg("sendDoTask", &[1, 2, 3]));
    let out = g.r.take_outgoing();
    assert_eq!(out.len(), 4, "{out:?}");
    assert_eq!(out[0], Outgoing::EndTurn);
    assert_eq!(out[1], Outgoing::CancelEndTurn);
    let Outgoing::GcNet(b) = &out[2] else {
        panic!("{:?}", out[2])
    };
    assert_eq!(decode_net(b).unwrap()[0].name(), "sendPushMission");
    assert!(matches!(&out[3], Outgoing::GcNet(_)));
    assert!(g.r.take_turn_reopened());
    assert!(!g.r.take_turn_reopened());
    assert_eq!(g.r.stats().turn_reopened, 1);

    // Taken back by the UI itself: nothing more to take back.
    g.r.post_net(msg("sendTurnComplete", &[0]));
    g.r.post_net(msg("sendTurnUnready", &[0]));
    g.r.post_net(msg("sendDoTask", &[1, 2, 3]));
    let out = g.r.take_outgoing();
    assert_eq!(out.len(), 3, "{out:?}");
    assert_eq!(out[1], Outgoing::CancelEndTurn);
    assert!(!g.r.take_turn_reopened());

    // A loaded game whose seat has ended the turn (the exe-api says so after the load).
    g.r.set_turn_sent(true);
    g.r.post_net(msg("SendRenameCity", &[3, 0]));
    let out = g.r.take_outgoing();
    assert_eq!(out[0], Outgoing::CancelEndTurn);
    assert!(g.r.take_turn_reopened());
}

/// Every human has ended the turn (the exe-api says so, `set_turn_locked`): a gameplay message goes alone, as the exe's
/// `BroadcastGameplayMessage` whose `SendTurnUnready` fails after NetTurnAllComplete, and the UI's own `sendTurnUnready`
/// is dropped; the turn stays ended (live report 2026-10-31: the END TURN button flipped while the server ran the AI).
#[test]
fn an_order_while_the_turn_is_processed_does_not_take_it_back() {
    let mut g = rig(cfg_always());
    join(&mut g, 1, 5, 0, &snap(1));
    g.r.post_net(msg("sendTurnComplete", &[0]));
    g.r.set_turn_locked(true);
    g.r.post_net(msg("sendDoTask", &[1, 2, 3]));
    g.r.post_net(msg("sendTurnUnready", &[0]));
    let out = g.r.take_outgoing();
    assert_eq!(out.len(), 2, "{out:?}");
    assert_eq!(out[0], Outgoing::EndTurn);
    assert!(matches!(&out[1], Outgoing::GcNet(_)));
    assert!(!g.r.take_turn_reopened());
    assert_eq!(g.r.stats().turn_reopened, 0);
    // Unlocked again (the exe-api's next look): the old rule.
    g.r.set_turn_locked(false);
    g.r.post_net(msg("sendDoTask", &[1, 2, 3]));
    let out = g.r.take_outgoing();
    assert_eq!(out[0], Outgoing::CancelEndTurn);
    assert!(g.r.take_turn_reopened());
}

#[test]
fn messages_that_are_not_the_uis_are_dropped_here() {
    let mut g = rig(cfg_always());
    join(&mut g, 1, 5, 0, &snap(1));
    g.r.post_net(msg("SendAICivsProcessed", &[]));
    g.r.post_net(msg("sendAutoMoves", &[]));
    assert!(g.r.take_outgoing().is_empty());
    assert_eq!(g.r.stats().net_dropped, 2);
}

#[test]
fn post_net_comes_after_what_the_core_collected() {
    let mut g = rig(cfg_always());
    join(&mut g, 1, 5, 0, &snap(1));
    // A Lua handler inside the core made a message first.
    g.log.borrow_mut().outbox.push(msg("sendDoTask", &[9]));
    g.r.post_net(msg("SendRenameCity", &[1]));
    let out = g.r.take_outgoing();
    // `post_net` routes the core's box first, then its own message: two frames, in that order.
    let names: Vec<_> = out
        .iter()
        .map(|o| {
            let Outgoing::GcNet(b) = o else {
                panic!("{o:?}")
            };
            decode_net(b).unwrap()[0].name()
        })
        .collect();
    assert_eq!(names, ["sendDoTask", "SendRenameCity"]);
}

#[test]
fn messages_of_the_core_are_taken_by_pump_and_take_outgoing() {
    let mut g = rig(cfg_always());
    join(&mut g, 1, 5, 0, &snap(1));
    g.log.borrow_mut().outbox.push(msg("sendDoTask", &[9]));
    g.r.pump(10);
    g.log
        .borrow_mut()
        .outbox
        .push(msg("sendTurnComplete", &[0]));
    let out = g.r.take_outgoing();
    assert_eq!(out.len(), 2);
    assert!(matches!(out[0], Outgoing::GcNet(_)));
    assert_eq!(out[1], Outgoing::EndTurn);
}

#[test]
fn big_batches_are_cut_into_frames_under_the_limit() {
    let mut g = rig(cfg_always());
    join(&mut g, 1, 5, 0, &snap(1));
    let long = "x".repeat(20_000);
    for _ in 0..8 {
        let mut m = msg("SendRenameCity", &[1]);
        m.text = long.clone();
        g.r.post_net(m);
    }
    let out = g.r.take_outgoing();
    assert_eq!(out.len(), 8, "each post is its own frame");
    // One drain of many at once is cut.
    {
        let mut l = g.log.borrow_mut();
        for _ in 0..8 {
            let mut m = msg("SendRenameCity", &[2]);
            m.text = long.clone();
            l.outbox.push(m);
        }
    }
    let out = g.r.take_outgoing();
    assert!(out.len() >= 3, "{} frames", out.len());
    let mut total = 0;
    for o in out {
        let Outgoing::GcNet(b) = o else { panic!() };
        assert!(b.len() <= 64 * 1024);
        total += decode_net(&b).unwrap().len();
    }
    assert_eq!(total, 8);
}

#[test]
fn input_survives_a_resync() {
    let mut g = rig(cfg_always());
    join(&mut g, 1, 5, 0, &snap(1));
    g.r.on_msg(journal(1, 5, vec![pump_batch(5)]));
    g.r.post_net(msg("SendRenameCity", &[1]));
    assert_eq!(g.r.state(), State::Resyncing);
    let out = g.r.take_outgoing();
    // The resync request and the player's message are both there, in the order they were made.
    assert!(matches!(out[0], Outgoing::Resync { .. }));
    assert!(matches!(out[1], Outgoing::GcNet(_)));
    // Could not be sent: back in front.
    g.r.unsend(out);
    g.r.post_net(msg("sendDoTask", &[1]));
    let out = g.r.take_outgoing();
    assert!(matches!(out[0], Outgoing::GcNet(_)));
    assert_eq!(out.len(), 2);
}

#[test]
fn generation_grows_on_every_load() {
    let mut g = rig(cfg_always());
    let s1 = snap(1);
    join(&mut g, 1, 5, 0, &s1);
    assert_eq!(GameSide::generation(&g.r), 1);
    let s2 = snap(2);
    g.r.on_msg(turn_msg(1, 6, 0, &s2));
    let w = g.r.wanted_blobs();
    g.r.blob(w[0].hash, s2.body.clone());
    finish_load(&mut g);
    assert_eq!(GameSide::generation(&g.r), 2);
}

#[test]
fn the_ui_calls_say_they_are_not_there_on_an_engine_without_them() {
    let mut g = rig(cfg_always());
    join(&mut g, 1, 5, 0, &snap(1));
    let mut out = Vec::new();
    let r = GameSide::ui_op(&mut g.r, replica::UiOp::ClearSelection, &[], &mut out);
    assert!(r.unwrap_err().contains("no UI interface"));
}

#[test]
fn join_and_turn_timings_are_counted() {
    let mut g = rig(cfg_always());
    g.clock.0.set(100);
    let s = snap(1);
    g.r.on_msg(base(1, 5, 0, &s));
    g.clock.0.set(400);
    g.r.wanted_blobs();
    g.r.blob(s.blob.hash, s.body.clone());
    finish_load(&mut g);
    assert_eq!(g.r.stats().join_ms, 300.0);
    let s2 = snap(2);
    g.clock.0.set(1000);
    g.r.on_msg(turn_msg(1, 6, 0, &s2));
    g.clock.0.set(1500);
    g.r.wanted_blobs();
    g.r.blob(s2.blob.hash, s2.body.clone());
    finish_load(&mut g);
    assert_eq!(g.r.stats().turn_wait_ms, 500.0);
}

#[test]
fn an_idle_replica_packs_a_raw_base() {
    // A base that came from a snapshot body is packed already; this checks the stats say so.
    let mut g = rig(ReplicaConfig::default());
    join(&mut g, 1, 5, 0, &snap(1));
    let st = g.r.stats();
    assert!(st.base_packed);
    assert!(st.base_bytes > 0);
}

/// The seat's own UI messages are "pending" from the `GcNet` frame until the journal brings their `NET` records back
/// (the auto unit cycle waits on it: a unit just ordered still looks ready in the replica), or a load, or a refusal, or
/// `OWN_PENDING_MAX_MS`.
#[test]
fn the_seats_own_messages_are_pending_until_the_journal_brings_them_back() {
    use gamecore::replica::{JournalRecord, encode_journal};
    let mut g = rig(cfg_always());
    g.log.borrow_mut().seat = Some(0);
    join(&mut g, 1, 5, 0, &snap(1));
    assert!(!g.r.own_orders_pending());
    let mission = msg("sendPushMission", &[7, 1, 3, 4]);
    g.r.post_net(mission.clone());
    g.r.post_net(msg("sendDoTask", &[1, 2, 3]));
    let _ = g.r.take_outgoing();
    assert!(g.r.own_orders_pending());
    let net = |player: u8, m: &NetMessage| JournalRecord::Net {
        player,
        message: m.clone(),
    };
    // Another player's message is not the echo.
    g.r.on_msg(journal(1, 1, vec![encode_journal(&[net(1, &mission)])]));
    pump_all(&mut g);
    assert!(g.r.own_orders_pending());
    // One of two back: still pending; both back: done.
    g.r.on_msg(journal(1, 2, vec![encode_journal(&[net(0, &mission)])]));
    pump_all(&mut g);
    assert!(g.r.own_orders_pending());
    g.r.on_msg(journal(1, 3, vec![encode_journal(&[net(0, &mission)])]));
    pump_all(&mut g);
    assert!(!g.r.own_orders_pending());
    // A refusal the shell reports takes it back too.
    g.r.post_net(mission.clone());
    assert!(g.r.own_orders_pending());
    g.r.note_net_rejected(1);
    assert!(!g.r.own_orders_pending());
    // A message nobody answers stops holding after a while.
    g.r.post_net(mission.clone());
    g.clock
        .0
        .set(g.clock.0.get() + replica::OWN_PENDING_MAX_MS as u64 + 1);
    assert!(!g.r.own_orders_pending());
}

/// An END_TURN the journal applied counts until a pump hands the turn-complete flag back to the core: the last human's
/// END_TURN has no pump after it in the journal (the AI phase is not journaled), so without this the replicas never saw
/// the last human done (live report 2026-10-31). A CANCEL_END_TURN takes it back, a load clears it.
#[test]
fn an_end_of_turn_of_the_journal_counts_until_the_next_pump() {
    use gamecore::replica::{JournalRecord, encode_journal};
    let mut g = rig(cfg_always());
    join(&mut g, 1, 5, 0, &snap(1));
    let cmd = |player: u8, c: gamecore::Command| JournalRecord::Command {
        player,
        kind: c.encode().0,
        args: vec![],
        text: None,
    };
    let pump = JournalRecord::Pump {
        slices: 1,
        status: 0,
        turn: 5,
    };
    g.r.on_msg(journal(
        1,
        1,
        vec![encode_journal(&[cmd(0, gamecore::Command::EndTurn), pump])],
    ));
    pump_all(&mut g);
    assert_eq!(
        g.r.turn_ended_by_journal(),
        0,
        "the pump marked it in the core"
    );
    g.r.on_msg(journal(
        1,
        2,
        vec![encode_journal(&[cmd(1, gamecore::Command::EndTurn)])],
    ));
    pump_all(&mut g);
    assert_eq!(g.r.turn_ended_by_journal(), 0b10);
    g.r.on_msg(journal(
        1,
        3,
        vec![encode_journal(&[cmd(1, gamecore::Command::CancelEndTurn)])],
    ));
    pump_all(&mut g);
    assert_eq!(g.r.turn_ended_by_journal(), 0);
    g.r.on_msg(journal(
        1,
        4,
        vec![encode_journal(&[cmd(2, gamecore::Command::ForceEndTurn)])],
    ));
    pump_all(&mut g);
    assert_eq!(g.r.turn_ended_by_journal(), 0b100);
}
