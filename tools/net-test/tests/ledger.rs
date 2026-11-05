//! The bot's books of orders (NT-D15): one final outcome per `id`, `Queued` is not final, resends after a
//! reconnect keep the `id`, a lost answer is a `Lost`, not silence.

use std::time::{Duration, Instant};

use net_test::bot::ledger::{Answer, Ledger, Outcome};
use protocol::{Cmd, Command, CommandOutcome, RejectCode};

fn cmd(id: u32) -> Cmd {
    Cmd {
        id,
        turn: 1,
        cmd: Command::UnitSleep { unit: 7 },
    }
}

fn rejected(code: RejectCode) -> CommandOutcome {
    CommandOutcome::Rejected {
        code,
        reason: "no".into(),
    }
}

#[test]
fn one_final_outcome_per_id() {
    let mut l = Ledger::default();
    let t = Instant::now();
    l.sent(&cmd(1), t, false);
    l.sent(&cmd(2), t, false);
    assert_eq!(l.open(), 2);
    let a = l.answer(1, &CommandOutcome::Applied, t + Duration::from_millis(40));
    assert_eq!(
        a,
        Answer::Final(Duration::from_millis(40), Outcome::Applied)
    );
    assert_eq!(
        l.answer(
            2,
            &rejected(RejectCode::NotYourTurn),
            t + Duration::from_millis(9)
        ),
        Answer::Final(
            Duration::from_millis(9),
            Outcome::Rejected(RejectCode::NotYourTurn)
        )
    );
    assert_eq!(l.open(), 0);
    let c = l.counts();
    assert_eq!((c.unique, c.applied, c.lost, c.open), (2, 1, 0, 0));
    assert_eq!(c.rejected.get("NotYourTurn"), Some(&1));
    assert_eq!(c.double_outcome, 0);
}

#[test]
fn queued_is_not_an_outcome() {
    let mut l = Ledger::default();
    let t = Instant::now();
    l.sent(&cmd(1), t, false);
    assert_eq!(l.answer(1, &CommandOutcome::Queued, t), Answer::Queued);
    assert_eq!(l.open(), 1, "a queued order is still open");
    assert_eq!(l.outcome(1), None);
    assert!(matches!(
        l.answer(1, &CommandOutcome::Applied, t + Duration::from_millis(500)),
        Answer::Final(_, Outcome::Applied)
    ));
    let c = l.counts();
    assert_eq!((c.queued, c.applied, c.open), (1, 1, 0));
}

#[test]
fn a_second_different_outcome_is_a_double() {
    let mut l = Ledger::default();
    let t = Instant::now();
    l.sent(&cmd(1), t, false);
    l.answer(1, &CommandOutcome::Applied, t);
    // Without a resend, even the same answer twice is a double.
    assert_eq!(l.answer(1, &CommandOutcome::Applied, t), Answer::Double);
    assert_eq!(l.answer(1, &CommandOutcome::Lost, t), Answer::Double);
    assert_eq!(l.counts().double_outcome, 2);
    // The first outcome stands.
    assert_eq!(l.outcome(1), Some(&Outcome::Applied));
}

#[test]
fn a_resent_order_may_hear_its_answer_again() {
    let mut l = Ledger::default();
    let t = Instant::now();
    l.sent(&cmd(1), t, false);
    // The link drops, the order goes again with the same id (a duplicate for the gateway).
    assert_eq!(l.unanswered(), vec![cmd(1)]);
    l.sent(&cmd(1), t + Duration::from_secs(1), false);
    assert_eq!(
        l.answer(1, &CommandOutcome::Applied, t + Duration::from_secs(2)),
        Answer::Final(Duration::from_secs(1), Outcome::Applied)
    );
    // The gateway answers the repeat as well: not a double.
    assert_eq!(
        l.answer(1, &CommandOutcome::Applied, t + Duration::from_secs(2)),
        Answer::Repeat
    );
    let c = l.counts();
    assert_eq!((c.double_outcome, c.repeated_answers), (0, 1));
    assert_eq!((c.unique, c.sent), (1, 2));
    // But a different answer to the repeat is a double.
    assert_eq!(
        l.answer(1, &rejected(RejectCode::Expired), t),
        Answer::Double
    );
}

#[test]
fn lost_is_a_final_outcome_and_is_not_sent_again() {
    let mut l = Ledger::default();
    let t = Instant::now();
    l.sent(&cmd(1), t, false);
    l.sent(&cmd(2), t, false);
    l.answer(1, &CommandOutcome::Lost, t);
    assert_eq!(l.unanswered(), vec![cmd(2)]);
    let c = l.counts();
    assert_eq!((c.lost, c.open), (1, 1));
}

#[test]
fn duplicates_are_counted_and_an_unknown_answer_is_noticed() {
    let mut l = Ledger::default();
    let t = Instant::now();
    l.sent(&cmd(5), t, false);
    l.sent(&cmd(5), t, true);
    l.sent(&cmd(5), t, true);
    let c = l.counts();
    assert_eq!((c.sent, c.unique, c.dup_sent), (3, 1, 2));
    assert_eq!(l.answer(99, &CommandOutcome::Applied, t), Answer::Unknown);
    assert_eq!(l.counts().unknown_answers, 1);
}
