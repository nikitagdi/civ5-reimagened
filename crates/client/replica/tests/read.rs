//! The read-only facade: `Replica::read` exists only while the replica is live, the fake counts its calls, and the
//! core's "not implemented" becomes `ReadError::Unsupported`.

mod common;

use std::cell::RefCell;
use std::rc::Rc;

use common::{Fake, Log, cfg_always, journal, pump_all, pump_batch, snap};
use gamecore::{Applied, GameError, GcError};
use replica::testing::{FakeRead, top_bar};
use replica::{
    CatalogTable, Engine, GameRead, NetMessage, ReadError, Replica, ReplicaStep, State, testing,
};

/// The fake engine of the state-machine tests with a reader attached.
struct Reading {
    inner: Fake,
    read: FakeRead,
}

impl Engine for Reading {
    fn load(&mut self, snapshot: &[u8]) -> Result<(), String> {
        self.inner.load(snapshot)
    }

    fn apply(&mut self, batch: &[u8]) -> Result<Applied, String> {
        self.inner.apply(batch)
    }

    fn take_ui_messages(&mut self) -> Result<Vec<NetMessage>, String> {
        self.inner.take_ui_messages()
    }

    fn read(&mut self) -> Option<&mut dyn GameRead> {
        Some(&mut self.read)
    }
}

fn replica_with_reader(gold: i32) -> (Replica, Rc<RefCell<Log>>) {
    let log = Rc::new(RefCell::new(Log::default()));
    let mut read = FakeRead::new(2, 1);
    read.top.gold = gold;
    let engine = Reading {
        inner: Fake(log.clone()),
        read,
    };
    (Replica::with_engine(Box::new(engine), cfg_always()), log)
}

#[test]
fn read_is_there_only_while_live() {
    let (mut r, log) = replica_with_reader(312);
    assert_eq!(r.state(), State::Empty);
    assert!(r.read().is_none());

    let s = snap(1);
    r.on_msg(common::base(1, 5, 0, &s));
    assert_eq!(r.state(), State::WaitBlob);
    assert!(r.read().is_none());

    r.blob(s.blob.hash, s.body.clone());
    let mut last = r.pump(10);
    while last == ReplicaStep::Working {
        assert!(r.read().is_none(), "no reads while loading");
        last = r.pump(10);
    }
    assert_eq!(last, ReplicaStep::Loaded { turn: 5 });
    assert_eq!(r.state(), State::Live);
    let read = r.read().expect("live: the reader is there");
    assert_eq!(read.seat(), 2);
    assert_eq!(read.team(), 1);
    assert_eq!(read.top_bar().map(|t| t.gold), Ok(312));

    // A diverging batch sends the replica to resync: no reads until the next base is loaded.
    log.borrow_mut().diverge_on_call = Some(0);
    r.on_msg(journal(1, 1, vec![pump_batch(5)]));
    pump_all_steps(&mut r);
    assert_eq!(r.state(), State::Resyncing);
    assert!(r.read().is_none());
}

fn pump_all_steps(r: &mut Replica) {
    for _ in 0..50 {
        if r.pump(1000) == ReplicaStep::Idle {
            return;
        }
    }
}

#[test]
fn an_engine_without_a_core_has_no_reader() {
    let mut rig = common::rig(cfg_always());
    let s = snap(3);
    common::join(&mut rig, 1, 1, 0, &s);
    pump_all(&mut rig);
    assert_eq!(rig.r.state(), State::Live);
    assert!(rig.r.read().is_none());
}

#[test]
fn the_fake_records_every_call() {
    let mut f = FakeRead::new(0, 0);
    f.catalogs.push((
        CatalogTable::Eras,
        vec![testing::catalog_entry(
            3,
            "ERA_MEDIEVAL",
            "Medieval",
            "ERA_ATLAS",
            1,
        )],
    ));
    assert_eq!(f.call_count(), 0);
    let _ = f.seat();
    let _ = f.team();
    assert_eq!(f.call_count(), 0, "seat and team are not reads of the core");

    assert_eq!(f.top_bar(), Ok(top_bar()));
    assert_eq!(f.catalog(CatalogTable::Eras).map(|c| c.len()), Ok(1));
    assert_eq!(
        f.catalog(CatalogTable::Technologies).map(|c| c.len()),
        Ok(0)
    );
    assert_eq!(f.game_info().map(|i| i.turn), Ok(0));
    assert_eq!(f.city_builds(7), Ok(Vec::new()));
    assert_eq!(
        f.calls,
        vec!["top_bar", "catalog", "catalog", "game_info", "city_builds"]
    );

    f.error = Some(ReadError::Unsupported);
    assert_eq!(f.units(), Err(ReadError::Unsupported));
    assert_eq!(f.call_count(), 6, "a failed read is a call too");
}

#[test]
fn the_fake_serves_the_ui2_queries_and_can_lack_them() {
    use replica::{
        CityOrder, OrderType, PlayerResource, PolicyBranchState, ResourceUsage, TechState,
        TechStatus,
    };
    let mut f = FakeRead::new(1, 1);
    f.queues.insert(
        4,
        vec![CityOrder {
            slot: 0,
            order: OrderType::Construct,
            item: 40,
            turns: Some(6),
            needed: 312,
        }],
    );
    f.techs.push(TechState {
        tech: 7,
        status: TechStatus::CanResearch,
        queue: Some(1),
        turns: Some(4),
        progress: 0,
        cost: 412,
    });
    f.resources.push(PlayerResource {
        resource: 3,
        usage: ResourceUsage::Strategic,
        available: 2,
        used: 4,
        total: 6,
    });
    f.branches.push(PolicyBranchState {
        branch: 0,
        unlocked: true,
        owned: 3,
        total: 5,
    });
    f.promotions.insert(9, vec![2, 17]);

    assert_eq!(f.city_queue(4).map(|q| q[0].item), Ok(40));
    assert_eq!(f.city_queue(5), Ok(Vec::new()), "a city without entries");
    assert_eq!(f.techs().map(|t| t[0].queue), Ok(Some(1)));
    assert_eq!(f.player_resources().map(|r| r[0].total), Ok(6));
    assert_eq!(f.policy_branches().map(|b| b[0].owned), Ok(3));
    assert_eq!(f.unit_promotions(9), Ok(vec![2, 17]));
    assert_eq!(
        f.calls,
        vec![
            "city_queue",
            "city_queue",
            "techs",
            "player_resources",
            "policy_branches",
            "unit_promotions"
        ]
    );

    // A core without the UI v2 queries: those reads are unsupported, the others still answer.
    f.ui2_error = Some(ReadError::Unsupported);
    assert_eq!(f.techs(), Err(ReadError::Unsupported));
    assert_eq!(f.unit_promotions(9), Err(ReadError::Unsupported));
    assert_eq!(f.city_queue(4), Err(ReadError::Unsupported));
    assert_eq!(f.player_resources(), Err(ReadError::Unsupported));
    assert_eq!(f.policy_branches(), Err(ReadError::Unsupported));
    assert_eq!(f.top_bar(), Ok(top_bar()));
    assert_eq!(f.call_count(), 12, "a failed read is a call too");
}

#[test]
fn a_core_older_than_the_query_is_unsupported() {
    // What the core of before the UI v2 queries answers for kinds 23..27 and the policy branch catalog.
    for m in [
        "gc_query kind 24: unknown query kind",
        "gc_query kind 13: Q_CATALOG: table out of range",
    ] {
        assert_eq!(
            ReadError::from(GameError::Core(GcError::InvalidArg(m.into()))),
            ReadError::Unsupported,
            "{m}"
        );
    }
    // A bad argument of a known query stays an error.
    assert!(matches!(
        ReadError::from(GameError::Core(GcError::InvalidArg(
            "gc_query kind 23: Q_CITY_QUEUE: the player has no such city".into()
        ))),
        ReadError::Core(_)
    ));
}

#[test]
fn not_implemented_is_unsupported_and_the_rest_is_a_core_error() {
    assert_eq!(
        ReadError::from(GameError::Core(GcError::NotImplemented)),
        ReadError::Unsupported
    );
    assert!(matches!(
        ReadError::from(GameError::NotStarted),
        ReadError::Core(_)
    ));
    assert!(matches!(
        ReadError::from(GameError::Core(GcError::InvalidArg("city".into()))),
        ReadError::Core(_)
    ));
}
