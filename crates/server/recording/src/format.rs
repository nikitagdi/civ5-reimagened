//! Record types and framing of a segment file.
//!
//! File: [`SegHeader`] (16 bytes), then one zstd stream. Inside the stream every record is
//! `[u32 LE n][u8 tag][postcard(Entry)]` with `n` = 1 + the length of the postcard bytes. The tag repeats the variant of
//! [`Body`] so a reader that does not know a newer variant can skip it by length.

use protocol::{Cmd, RecPhase};
use serde::{Deserialize, Serialize};

/// First four bytes of a segment.
pub const MAGIC: [u8; 4] = *b"C5RC";
/// Format version of the segment and of the record types (append only: new variants at the end).
pub const VERSION: u16 = 1;
/// Length of the segment header.
pub const HEADER_LEN: usize = 16;

/// The header of a segment file: magic, version, flags, game, turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SegHeader {
    pub version: u16,
    pub flags: u16,
    pub game: u32,
    /// The turn of the keyframe the segment starts with (0 for the segment before the first keyframe).
    pub turn: i32,
}

impl SegHeader {
    pub fn new(game: u32, turn: i32) -> Self {
        SegHeader {
            version: VERSION,
            flags: 0,
            game,
            turn,
        }
    }

    pub fn encode(&self) -> [u8; HEADER_LEN] {
        let mut b = [0u8; HEADER_LEN];
        b[..4].copy_from_slice(&MAGIC);
        b[4..6].copy_from_slice(&self.version.to_le_bytes());
        b[6..8].copy_from_slice(&self.flags.to_le_bytes());
        b[8..12].copy_from_slice(&self.game.to_le_bytes());
        b[12..16].copy_from_slice(&self.turn.to_le_bytes());
        b
    }

    /// # Errors
    /// Not a segment: too short, wrong magic.
    pub fn decode(b: &[u8]) -> Result<Self, String> {
        if b.len() < HEADER_LEN {
            return Err(format!("a segment header has {} bytes, not 16", b.len()));
        }
        if b[..4] != MAGIC {
            return Err("not a C5RC segment (wrong magic)".into());
        }
        let u16_at = |i: usize| u16::from_le_bytes([b[i], b[i + 1]]);
        let u32_at = |i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
        Ok(SegHeader {
            version: u16_at(4),
            flags: u16_at(6),
            game: u32_at(8),
            turn: i32::from_le_bytes([b[12], b[13], b[14], b[15]]),
        })
    }
}

/// One record: when (microseconds since the recording opened on the gateway's clock, and calendar milliseconds) and what.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub mono_us: u64,
    pub wall_ms: u64,
    pub body: Body,
}

/// What a record says. Variant order is the file format (the tag): append only.
// Records are built and matched once each; the size difference is not worth a Box.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Body {
    /// Tag 0: the first record of a recording.
    Header(HeaderRec),
    /// Tag 1: an order of a seat as the gateway received it, with the verdict of the gateway.
    Order(OrderRec),
    /// Tag 2: the UI messages (GCN1) of a seat.
    Net(NetRec),
    /// Tag 3: a seat event (attach, detach, claim, kick, forced end of turn ...).
    Seat(SeatRec),
    /// Tag 4: a GCJ1 batch of the core, as the core wrote it.
    Journal(JournalRec),
    /// Tag 5: the answer to an order.
    Result(ResultRec),
    /// Tag 6: the turn state.
    Turn(TurnRec),
    /// Tag 7: the snapshot of the start of a human phase; the first record of its segment.
    Keyframe(KeyframeRec),
    /// Tag 8: the worker: start, exit, hang, fault, the long core calls.
    Worker(WorkerRec),
    /// Tag 9: a seat asked for a new snapshot.
    Resync(ResyncRec),
    /// Tag 10: the size and tags of every `GameBatch` sent to a seat (written within the meta budget of the recorder,
    /// SIMP-exe-982).
    BatchMeta(BatchMetaRec),
    /// Tag 11: the timeline of one end of a turn in the shared envelope: `postcard(turntrace::TurnTrace)`.
    Trace(TraceRec),
    /// Tag 12: a client report arrived for this game.
    Report(ReportRec),
    /// Tag 13: a save the worker wrote.
    Save(SaveRec),
    /// Tag 14: how long the gateway had an order (sent to the worker until its final answer).
    OrderSpan(OrderSpanRec),
    /// Tag 15: a message of a mod, in either direction (protocol v13). A reader that does not know it skips it.
    Mod(ModRec),
}

impl Body {
    /// The tag of the variant (the byte in front of the record).
    pub fn tag(&self) -> u8 {
        match self {
            Body::Header(_) => 0,
            Body::Order(_) => 1,
            Body::Net(_) => 2,
            Body::Seat(_) => 3,
            Body::Journal(_) => 4,
            Body::Result(_) => 5,
            Body::Turn(_) => 6,
            Body::Keyframe(_) => 7,
            Body::Worker(_) => 8,
            Body::Resync(_) => 9,
            Body::BatchMeta(_) => 10,
            Body::Trace(_) => 11,
            Body::Report(_) => 12,
            Body::Save(_) => 13,
            Body::OrderSpan(_) => 14,
            Body::Mod(_) => 15,
        }
    }

    /// The name of the variant, for the tool's output and filters.
    pub fn name(&self) -> &'static str {
        tag_name(self.tag())
    }

    /// Records that must reach the disk at once (the process may be about to die, or a human is about to look).
    pub fn urgent(&self) -> bool {
        match self {
            Body::Worker(w) => !matches!(w.ev, WorkerEv::Start { .. }),
            Body::Resync(_) | Body::Report(_) | Body::Keyframe(_) => true,
            Body::Result(r) => r.code.is_some(),
            _ => false,
        }
    }
}

/// The name of a tag (`order`, `journal` ...); `"unknown"` for a tag this build does not know.
pub fn tag_name(tag: u8) -> &'static str {
    match tag {
        0 => "header",
        1 => "order",
        2 => "net",
        3 => "seat",
        4 => "journal",
        5 => "result",
        6 => "turn",
        7 => "keyframe",
        8 => "worker",
        9 => "resync",
        10 => "batch",
        11 => "trace",
        12 => "report",
        13 => "save",
        14 => "orderspan",
        15 => "mod",
        _ => "unknown",
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeaderRec {
    pub format: u16,
    pub game: u32,
    /// The gateway's build.
    pub gateway: String,
    /// The core build id of the worker (`WorkerReady.build_id`); empty until the worker is ready.
    pub core_build: String,
    pub seed: u64,
    /// Calendar milliseconds at `mono_us` 0.
    pub wall_at_mono0_ms: u64,
    pub created_ms: u64,
    pub name: String,
    /// The settings of the game (for `civ5-replay export`).
    pub settings: Option<protocol::GameSettings>,
    /// Who sits where (`WorkerReady.seats`); empty before the worker is ready.
    pub seats: Vec<protocol::SlotSetup>,
}

/// What the gateway did with an order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Verdict {
    /// Sent to the worker.
    Forwarded,
    /// An id the seat already sent and got an answer to; the answer was sent again.
    Repeat,
    /// An id still being worked on; dropped.
    Pending,
    /// An id older than the ring; answered `Expired`.
    Expired,
    /// Refused at the gateway (`code` is the `RejectCode`), never seen by the core.
    Refused { code: String, why: String },
    /// The worker was not there.
    SendFailed { why: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrderRec {
    pub player: i8,
    pub cmd: Cmd,
    pub verdict: Verdict,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetRec {
    pub player: i8,
    /// The GCN1 bytes as received.
    pub bytes: Vec<u8>,
    /// Passed to the worker; `false`: dropped (too big, empty, over the rate).
    pub taken: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeatRec {
    pub player: i8,
    /// `attach`, `detach`, `claim`, `kick`, `give_up`, `force_end_turn`, `lost` ...
    pub event: String,
    pub nick: Option<String>,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalRec {
    pub phase: RecPhase,
    pub epoch: u32,
    /// Number of the batch in the replica stream (`Human` only; 0 for the others).
    pub jseq: u64,
    pub turn: i32,
    /// `GCJ1` bytes.
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResultRec {
    pub player: i8,
    pub id: u32,
    /// `Applied`, `Queued`, `Lost`, `Rejected`.
    pub outcome: String,
    pub code: Option<String>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnRec {
    pub turn: i32,
    pub phase: String,
    pub mode: String,
    /// The seats that have ended the turn.
    pub finished: Vec<i8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum KfKind {
    Full,
    Patch,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyframeRec {
    /// The turn of the human phase this snapshot opens.
    pub turn: i32,
    pub epoch: u32,
    /// The number of the last batch before the snapshot; the batches that follow continue from it.
    pub jseq: u64,
    /// xxh64 of the unpacked GCS1.
    pub raw_hash: u64,
    /// xxh64 of the stored body.
    pub hash: u64,
    pub kind: KfKind,
    /// For a patch: the `raw_hash` of the snapshot it applies to.
    pub from_raw_hash: Option<u64>,
    /// File under `kf/`; `None`: the body was not kept (the per-game cap).
    pub file: Option<String>,
    pub bytes: u32,
    /// The first snapshot after a worker start: the chain begins here.
    pub base: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkerEv {
    Start {
        pid: Option<u32>,
        turn: i32,
        restart: bool,
    },
    /// The worker ended (`hang`: killed for silence).
    Exit {
        hang: bool,
        failure: Option<String>,
        code: Option<i32>,
        killed: bool,
        /// Orders per seat that were lost with it.
        lost: Vec<(i8, u32)>,
    },
    /// The game is faulted.
    Fault {
        why: String,
    },
    /// A core call that may run long began (`WorkerMsg::RecMark`).
    CallBegin {
        turn: i32,
        what: String,
    },
    /// A marked call that ran past a second ended.
    CallEnd {
        turn: i32,
        what: String,
    },
    Fatal {
        message: String,
        code: u8,
    },
    /// Anything else (the writer's own notes: a cap was reached).
    Note(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerRec {
    pub ev: WorkerEv,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResyncRec {
    pub player: i8,
    pub epoch: u32,
    pub chain_epoch: u32,
    pub have_raw_hash: Option<u64>,
    pub at_jseq: u64,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BatchMetaRec {
    pub player: i8,
    pub seq: u64,
    pub bytes: u32,
    pub tags: Vec<u8>,
}

/// `cmd.worker`: from the order going to the worker's pipe to its final answer, microseconds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrderSpanRec {
    pub player: i8,
    pub id: u32,
    pub stage: String,
    pub us: u64,
}

/// A message of a mod (`ClientMsg::Mod` as the game took it, or what the server half sent: `GameMsg::Mod`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModRec {
    /// Position in the game's load order.
    pub mod_idx: u16,
    pub ch: u8,
    /// `true`: a client sent it to the server half; `false`: the server half sent it to the clients.
    pub c2s: bool,
    /// Client to server: the sender. Server to client: the seat that caused it (`from` of the message), if any.
    pub player: Option<i8>,
    /// Server to client: the seats it was sent to (empty: nobody has the mod).
    pub to: Vec<i8>,
    /// Client to server: it reached the mod (`false`: the host limits dropped it).
    pub taken: bool,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceRec {
    /// The turn that was ended.
    pub turn: i32,
    /// `postcard(turntrace::TurnTrace)`.
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportRec {
    pub id: String,
    pub player: Option<i8>,
    pub nick: String,
    pub turn: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaveRec {
    pub seq: u64,
    pub turn: i32,
    pub kind: String,
    pub file: String,
    pub bytes: u64,
}

/// The framed bytes of an entry: `[u32 n][u8 tag][postcard]`.
///
/// # Errors
/// postcard refused the record (not reachable for these types).
pub fn encode_entry(e: &Entry) -> Result<Vec<u8>, String> {
    let payload = postcard::to_stdvec(e).map_err(|x| x.to_string())?;
    let n = u32::try_from(payload.len() + 1).map_err(|_| "a record over 4 GiB".to_owned())?;
    let mut out = Vec::with_capacity(payload.len() + 5);
    out.extend_from_slice(&n.to_le_bytes());
    out.push(e.body.tag());
    out.extend_from_slice(&payload);
    Ok(out)
}

/// What [`decode_next`] found at the start of a buffer.
#[derive(Debug)]
pub enum Next {
    /// A record and the bytes it took.
    Entry(Box<Entry>, usize),
    /// A record this build cannot decode (a newer variant or damaged bytes): its tag and size, skipped.
    Skipped { tag: u8, len: usize, total: usize },
    /// Fewer bytes than the record says: a torn tail.
    Partial,
    /// No bytes left.
    End,
}

/// Decodes the record at the start of `buf`.
pub fn decode_next(buf: &[u8]) -> Next {
    if buf.is_empty() {
        return Next::End;
    }
    if buf.len() < 5 {
        return Next::Partial;
    }
    let n = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
    if n == 0 || buf.len() < 4 + n {
        return Next::Partial;
    }
    let tag = buf[4];
    let payload = &buf[5..4 + n];
    let skipped = Next::Skipped {
        tag,
        len: n - 1,
        total: 4 + n,
    };
    if tag == 0 {
        // The header holds `GameSettings`, which grew a field in protocol v13 (postcard is positional): the
        // current layout must fill the record exactly, otherwise it is read as the v12 layout.
        return match decode_header(payload) {
            Some(e) => Next::Entry(Box::new(e), 4 + n),
            None => skipped,
        };
    }
    match postcard::from_bytes::<Entry>(payload) {
        Ok(e) if e.body.tag() == tag => Next::Entry(Box::new(e), 4 + n),
        _ => skipped,
    }
}

/// `HeaderRec` before protocol v13: `GameSettings` without `mods`.
#[derive(Deserialize)]
#[cfg_attr(test, derive(Serialize))]
struct HeaderRecV12 {
    format: u16,
    game: u32,
    gateway: String,
    core_build: String,
    seed: u64,
    wall_at_mono0_ms: u64,
    created_ms: u64,
    name: String,
    settings: Option<protocol::GameSettingsV12>,
    seats: Vec<protocol::SlotSetup>,
}

fn decode_header(payload: &[u8]) -> Option<Entry> {
    if let Ok((e, rest)) = postcard::take_from_bytes::<Entry>(payload)
        && rest.is_empty()
        && e.body.tag() == 0
    {
        return Some(e);
    }
    let (mono_us, r) = postcard::take_from_bytes::<u64>(payload).ok()?;
    let (wall_ms, r) = postcard::take_from_bytes::<u64>(r).ok()?;
    let (tag, r) = postcard::take_from_bytes::<u32>(r).ok()?;
    let (h, rest) = postcard::take_from_bytes::<HeaderRecV12>(r).ok()?;
    if tag != 0 || !rest.is_empty() {
        return None;
    }
    Some(Entry {
        mono_us,
        wall_ms,
        body: Body::Header(HeaderRec {
            format: h.format,
            game: h.game,
            gateway: h.gateway,
            core_build: h.core_build,
            seed: h.seed,
            wall_at_mono0_ms: h.wall_at_mono0_ms,
            created_ms: h.created_ms,
            name: h.name,
            settings: h.settings.map(Into::into),
            seats: h.seats,
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(body: Body) -> Entry {
        Entry {
            mono_us: 5,
            wall_ms: 1_700_000_000_000,
            body,
        }
    }

    /// A recording written before protocol v13 has `GameSettings` without `mods` in its header: it still reads.
    #[test]
    fn a_v12_header_still_reads() {
        let settings = protocol::GameSettingsV12 {
            name: "g".into(),
            world_size: "w".into(),
            map_script: "m".into(),
            map_options: vec![],
            speed: "s".into(),
            start_era: "e".into(),
            handicap: "h".into(),
            minor_civs: 2,
            turn_mode: protocol::TurnMode::Hybrid,
            timer: protocol::TimerMode::Off,
            game_options: vec![("o".into(), 1)],
            seed: Some(5),
            map_seed: None,
            auto_choices: protocol::AutoChoices::default(),
            diplo_auto_decline: false,
        };
        let old = HeaderRecV12 {
            format: 1,
            game: 3,
            gateway: "gw".into(),
            core_build: "cb".into(),
            seed: 9,
            wall_at_mono0_ms: 1,
            created_ms: 2,
            name: "n".into(),
            settings: Some(settings),
            seats: vec![],
        };
        let payload = postcard::to_stdvec(&(5u64, 6u64, 0u32, &old)).unwrap();
        let mut rec = ((payload.len() + 1) as u32).to_le_bytes().to_vec();
        rec.push(0);
        rec.extend_from_slice(&payload);
        let Next::Entry(e, used) = decode_next(&rec) else {
            panic!("the v12 header is skipped");
        };
        assert_eq!(used, rec.len());
        let Body::Header(h) = e.body else {
            panic!("not a header")
        };
        assert_eq!(
            (e.mono_us, e.wall_ms, h.game, h.name.as_str()),
            (5, 6, 3, "n")
        );
        assert!(
            h.settings
                .is_some_and(|s| s.mods.is_empty() && s.minor_civs == 2)
        );
        // The current layout round trips as before.
        let now = Entry {
            mono_us: 1,
            wall_ms: 2,
            body: Body::Header(HeaderRec {
                format: 1,
                game: 3,
                gateway: "gw".into(),
                core_build: String::new(),
                seed: 1,
                wall_at_mono0_ms: 1,
                created_ms: 2,
                name: "n".into(),
                settings: None,
                seats: vec![],
            }),
        };
        let bytes = encode_entry(&now).unwrap();
        assert!(matches!(decode_next(&bytes), Next::Entry(e, _) if *e == now));
    }

    /// Protocol v15 orders are recorded like the older ones. A reader whose `Command` lacks a variant (a
    /// v14 build reading a v15 recording) skips that order record by its length and reads on.
    #[test]
    fn v15_orders_record_and_unknown_commands_are_skipped() {
        let order = entry(Body::Order(OrderRec {
            player: 1,
            cmd: Cmd {
                id: 4,
                turn: 30,
                cmd: protocol::Command::CityQueuePush {
                    city: 2,
                    order: 1,
                    item: 3,
                    front: false,
                },
            },
            verdict: Verdict::Forwarded,
        }));
        let bytes = encode_entry(&order).unwrap();
        assert!(
            matches!(decode_next(&bytes), Next::Entry(e, n) if *e == order && n == bytes.len())
        );

        // Entry { mono_us, wall_ms, Body::Order (1) { player, Cmd { id, turn, Command tag 99 } } }.
        let payload = postcard::to_stdvec(&(5u64, 6u64, 1u32, 1i8, 4u32, 30i32, 99u32)).unwrap();
        let mut rec = ((payload.len() + 1) as u32).to_le_bytes().to_vec();
        rec.push(1);
        rec.extend_from_slice(&payload);
        let skipped_len = rec.len();
        rec.extend_from_slice(&bytes);
        let Next::Skipped { tag: 1, total, .. } = decode_next(&rec) else {
            panic!("an order with an unknown command is not skipped");
        };
        assert_eq!(total, skipped_len);
        assert!(matches!(decode_next(&rec[total..]), Next::Entry(e, _) if *e == order));
    }

    #[test]
    fn the_header_round_trips_and_bad_ones_are_refused() {
        let h = SegHeader::new(936, 126);
        let b = h.encode();
        assert_eq!(&b[..4], b"C5RC");
        assert_eq!(SegHeader::decode(&b).unwrap(), h);
        assert!(SegHeader::decode(&b[..15]).is_err());
        let mut bad = b;
        bad[0] = b'X';
        assert!(SegHeader::decode(&bad).is_err());
    }

    #[test]
    fn tags_match_the_variants_and_entries_round_trip() {
        let bodies = vec![
            Body::Net(NetRec {
                player: 1,
                bytes: vec![1, 2, 3],
                taken: true,
            }),
            Body::Journal(JournalRec {
                phase: RecPhase::Ai,
                epoch: 9,
                jseq: 0,
                turn: 3,
                bytes: vec![0; 40],
            }),
            Body::Worker(WorkerRec {
                ev: WorkerEv::Exit {
                    hang: true,
                    failure: Some("Hang".into()),
                    code: None,
                    killed: true,
                    lost: vec![(0, 2)],
                },
            }),
            Body::Trace(TraceRec {
                turn: 4,
                bytes: vec![9],
            }),
            Body::Mod(ModRec {
                mod_idx: 1,
                ch: 2,
                c2s: false,
                player: Some(0),
                to: vec![0, 1],
                taken: true,
                payload: vec![7; 5],
            }),
        ];
        for b in bodies {
            let e = entry(b);
            let framed = encode_entry(&e).unwrap();
            assert_eq!(framed[4], e.body.tag());
            match decode_next(&framed) {
                Next::Entry(got, n) => {
                    assert_eq!(*got, e);
                    assert_eq!(n, framed.len());
                }
                other => panic!("{other:?}"),
            }
            // A cut record is a torn tail, not a panic.
            for cut in 1..framed.len() {
                assert!(
                    matches!(decode_next(&framed[..cut]), Next::Partial),
                    "cut {cut}"
                );
            }
        }
        assert!(matches!(decode_next(&[]), Next::End));
    }

    /// Tag 15 is the mod record, appended after the 15 of the first format: the tags before it did not move, and a
    /// reader that has no `Mod` variant reaches the records behind it by the length in front.
    #[test]
    fn the_mod_record_is_tag_15_and_older_tags_stay() {
        let m = Body::Mod(ModRec {
            mod_idx: 0,
            ch: 0,
            c2s: true,
            player: None,
            to: vec![],
            taken: false,
            payload: vec![],
        });
        assert_eq!((m.tag(), m.name()), (15, "mod"));
        assert_eq!(
            Body::OrderSpan(OrderSpanRec {
                player: 0,
                id: 0,
                stage: String::new(),
                us: 0
            })
            .tag(),
            14
        );
        assert_eq!(tag_name(16), "unknown");
    }

    #[test]
    fn a_record_of_an_unknown_variant_is_skipped_by_its_length() {
        let e = entry(Body::Net(NetRec {
            player: 0,
            bytes: vec![],
            taken: false,
        }));
        let mut framed = encode_entry(&e).unwrap();
        // The tag byte no longer matches the variant: a build that knows more variants than this one would see
        // bytes it cannot read in the same way.
        framed[4] = 77;
        match decode_next(&framed) {
            Next::Skipped { tag, total, .. } => {
                assert_eq!((tag, total), (77, framed.len()));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn faults_and_resyncs_are_urgent_and_nets_are_not() {
        assert!(
            Body::Worker(WorkerRec {
                ev: WorkerEv::Fault { why: "x".into() }
            })
            .urgent()
        );
        assert!(
            !Body::Worker(WorkerRec {
                ev: WorkerEv::Start {
                    pid: None,
                    turn: 1,
                    restart: false
                }
            })
            .urgent()
        );
        assert!(
            !Body::Net(NetRec {
                player: 0,
                bytes: vec![],
                taken: true
            })
            .urgent()
        );
    }
}
