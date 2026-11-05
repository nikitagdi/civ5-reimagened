//! One line per record, for `log`, `verify` and `step`.

use gamecore::replica::{JournalRecord, decode_journal};
use protocol::Codec;
use recording::format::{Body, Entry, Verdict, WorkerEv};

/// `+12.345s`: the time since the recording opened.
pub fn at(e: &Entry) -> String {
    format!("+{:>9.3}s", e.mono_us as f64 / 1e6)
}

/// A journal record in one short phrase.
pub fn journal_record(r: &JournalRecord) -> String {
    match r {
        JournalRecord::Command {
            player,
            kind,
            args,
            text,
        } => {
            let t = text
                .as_deref()
                .map_or_else(String::new, |t| format!(" {t:?}"));
            format!("command seat {player} kind {kind} args {args:?}{t}")
        }
        JournalRecord::Net { player, message } => {
            format!(
                "net seat {player} {} {:?}",
                message.name(),
                &message.args[..message.args.len().min(6)]
            )
        }
        JournalRecord::Pump {
            slices,
            status,
            turn,
        } => {
            format!("pump {slices} slices, status {status}, turn {turn}")
        }
        JournalRecord::Slot { player, op, nick } => {
            format!(
                "slot seat {player} op {op} {}",
                nick.as_deref().unwrap_or("")
            )
        }
        JournalRecord::Check { digest } => format!("check {digest:016x}"),
    }
}

/// The records of a GCJ1 batch, or why it does not decode.
pub fn journal_text(bytes: &[u8]) -> String {
    match decode_journal(bytes) {
        Ok(recs) => recs
            .iter()
            .map(journal_record)
            .collect::<Vec<_>>()
            .join("; "),
        Err(e) => format!("(does not decode: {e})"),
    }
}

/// The seat a record belongs to, for `--player`.
pub fn player_of(b: &Body) -> Option<i8> {
    match b {
        Body::Order(o) => Some(o.player),
        Body::Net(n) => Some(n.player),
        Body::Seat(s) => Some(s.player),
        Body::Result(r) => Some(r.player),
        Body::Resync(r) => Some(r.player),
        Body::BatchMeta(m) => Some(m.player),
        Body::OrderSpan(o) => Some(o.player),
        Body::Report(r) => r.player,
        _ => None,
    }
}

/// The body in one line.
pub fn body_text(b: &Body) -> String {
    match b {
        Body::Header(h) => format!(
            "header game {} {:?} gateway {} core {:?} seed {}",
            h.game, h.name, h.gateway, h.core_build, h.seed
        ),
        Body::Order(o) => {
            let v = match &o.verdict {
                Verdict::Forwarded => "forwarded".to_owned(),
                Verdict::Repeat => "repeat".to_owned(),
                Verdict::Pending => "pending (dropped)".to_owned(),
                Verdict::Expired => "expired".to_owned(),
                Verdict::Refused { code, why } => format!("REFUSED {code}: {why}"),
                Verdict::SendFailed { why } => format!("NOT SENT: {why}"),
            };
            format!(
                "order seat {} id {} turn {} {:?} -> {v}",
                o.player, o.cmd.id, o.cmd.turn, o.cmd.cmd
            )
        }
        Body::Net(n) => format!(
            "net seat {} {} bytes{}",
            n.player,
            n.bytes.len(),
            if n.taken { "" } else { " DROPPED" }
        ),
        Body::Seat(s) => format!(
            "seat {} {} {} {}",
            s.player,
            s.event,
            s.nick.as_deref().unwrap_or(""),
            s.detail
        ),
        Body::Journal(j) => format!(
            "journal {:?} epoch {} jseq {} turn {}: {}",
            j.phase,
            j.epoch,
            j.jseq,
            j.turn,
            journal_text(&j.bytes)
        ),
        Body::Result(r) => format!(
            "result seat {} id {} {}{}{}",
            r.player,
            r.id,
            r.outcome,
            r.code
                .as_deref()
                .map_or_else(String::new, |c| format!(" {c}")),
            r.reason
                .as_deref()
                .map_or_else(String::new, |c| format!(": {c}"))
        ),
        Body::Turn(t) => format!(
            "turn state {} {} {} finished {:?}",
            t.turn, t.phase, t.mode, t.finished
        ),
        Body::Keyframe(k) => format!(
            "KEYFRAME turn {} {:?} epoch {} jseq {} raw {:016x} {} bytes{}{}",
            k.turn,
            k.kind,
            k.epoch,
            k.jseq,
            k.raw_hash,
            k.bytes,
            if k.base { " (base)" } else { "" },
            k.file
                .as_deref()
                .map_or(" (body not kept)".to_owned(), |f| format!(" {f}"))
        ),
        Body::Worker(w) => match &w.ev {
            WorkerEv::Start { pid, turn, restart } => {
                format!("worker start pid {pid:?} turn {turn} restart {restart}")
            }
            WorkerEv::Exit {
                hang,
                failure,
                code,
                killed,
                lost,
            } => format!(
                "WORKER {} failure {failure:?} code {code:?} killed {killed} lost orders {lost:?}",
                if *hang { "HANG" } else { "EXIT" }
            ),
            WorkerEv::Fault { why } => format!("WORKER FAULT {why}"),
            WorkerEv::CallBegin { turn, what } => format!("core call begins (turn {turn}): {what}"),
            WorkerEv::CallEnd { turn, what } => format!("core call ends (turn {turn}): {what}"),
            WorkerEv::Fatal { message, code } => format!("WORKER FATAL {code}: {message}"),
            WorkerEv::Note(t) => format!("note: {t}"),
        },
        Body::Resync(r) => format!(
            "resync seat {} epoch {} (chain {}) at jseq {} have {:?}: {}",
            r.player,
            r.epoch,
            r.chain_epoch,
            r.at_jseq,
            r.have_raw_hash.map(|h| format!("{h:016x}")),
            r.reason
        ),
        Body::BatchMeta(m) => format!(
            "batch seat {} seq {} {} bytes tags {:?}",
            m.player, m.seq, m.bytes, m.tags
        ),
        Body::OrderSpan(o) => format!("{} seat {} order {}: {} us", o.stage, o.player, o.id, o.us),
        Body::Trace(t) => match protocol::Postcard.decode::<turntrace::TurnTrace>(&t.bytes) {
            Ok(tr) => format!(
                "trace of the end of turn {}: {} records",
                t.turn,
                tr.recs.len()
            ),
            Err(_) => format!(
                "trace of the end of turn {}: {} bytes",
                t.turn,
                t.bytes.len()
            ),
        },
        Body::Report(r) => format!(
            "REPORT {} from {} (seat {:?}, turn {:?})",
            r.id, r.nick, r.player, r.turn
        ),
        Body::Save(s) => format!(
            "save {} turn {} {} {} ({} bytes)",
            s.seq, s.turn, s.kind, s.file, s.bytes
        ),
    }
}
