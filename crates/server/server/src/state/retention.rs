//! Cleaning up the disk of the server: finished games, their logs, and recordings.
//!
//! One rule engine, [`select_evictions`], and three users of it:
//!
//! - **saves**: `games/<id>/` of a finished game goes when it is not among the newest `keep_games` games and
//!   has been finished for `finished_max_age_days`. A game that runs, sleeps, faulted (its newest save can be
//!   loaded from the lobby) or has no readable meta is never touched, nor is a game the gateway has an actor
//!   for;
//! - **logs**: `logs/game-<id>.jsonl*` and `logs/worker-<id>.log*` of a game whose directory is gone go; the
//!   rest is held under `logs_max_mb` by dropping the oldest files first. The file a live game writes to
//!   is never dropped;
//! - **recordings**: `recordings/<name>/` (written by the recording writer of P2) are the newest
//!   `recordings_games` ones within `recordings_max_mb`, except the names the caller protects (live games).
//!
//! The functions take the time and the set of live games as arguments and touch only the directories they are
//! given, so tests run them on a temporary directory with a fake clock. [`spawn_loop`] runs a pass at start and
//! every `interval_min` minutes.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::meta::{self, MetaStatus};
use crate::Shared;
use crate::config::Retention;

const MB: u64 = 1024 * 1024;
const DAY_MS: u64 = 24 * 60 * 60 * 1000;
/// A log file younger than this is never deleted as "its game is gone": the game may be starting.
const ORPHAN_GRACE_MS: u64 = 60 * 60 * 1000;

/// One thing that can be deleted: a game directory, a log file, a recording.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    /// Disk use, bytes.
    pub bytes: u64,
    /// Higher is newer; ranks the entries ("the last N games").
    pub rank: u64,
    /// Unix ms of the last activity (the age rule and the byte cap's order use it).
    pub last_ms: u64,
    /// Never deleted; still counted in the ranks and the bytes.
    pub protected: bool,
}

/// What to keep. Every rule is optional.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Policy {
    /// The newest this many entries are safe from the age rule.
    pub keep_at_least: usize,
    /// Entries beyond the newest this many go.
    pub keep_at_most: Option<usize>,
    /// An entry (beyond `keep_at_least`) idle for longer than this goes, ms.
    pub max_age_ms: Option<u64>,
    /// The total is brought under this by dropping the oldest unprotected entries; it wins over the counts.
    pub max_bytes: Option<u64>,
}

/// The names to delete, in the order the rules found them.
pub fn select_evictions(entries: &[Entry], now_ms: u64, p: &Policy) -> Vec<String> {
    let mut order: Vec<&Entry> = entries.iter().collect();
    order.sort_by(|a, b| b.rank.cmp(&a.rank).then_with(|| a.name.cmp(&b.name)));
    let mut gone: Vec<&Entry> = Vec::new();
    let mut alive: Vec<&Entry> = Vec::new();
    for (i, e) in order.into_iter().enumerate() {
        let too_many = p.keep_at_most.is_some_and(|n| i >= n);
        let too_old = i >= p.keep_at_least
            && p.max_age_ms
                .is_some_and(|age| now_ms.saturating_sub(e.last_ms) > age);
        if !e.protected && (too_many || too_old) {
            gone.push(e);
        } else {
            alive.push(e);
        }
    }
    if let Some(cap) = p.max_bytes {
        let mut total: u64 = alive.iter().map(|e| e.bytes).sum();
        // The least recently active first.
        let mut by_idle: Vec<&Entry> = alive.iter().copied().filter(|e| !e.protected).collect();
        by_idle.sort_by(|a, b| a.last_ms.cmp(&b.last_ms).then_with(|| a.rank.cmp(&b.rank)));
        for e in by_idle {
            if total <= cap {
                break;
            }
            total = total.saturating_sub(e.bytes);
            gone.push(e);
        }
    }
    gone.into_iter().map(|e| e.name.clone()).collect()
}

// ---------------------------------------------------------------- disk helpers

fn ms_of(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// Total bytes and newest modification time (Unix ms) of a file, or of everything under a directory.
fn measure(path: &Path) -> (u64, u64) {
    let Ok(md) = fs::symlink_metadata(path) else {
        return (0, 0);
    };
    let mtime = md.modified().map_or(0, ms_of);
    if !md.is_dir() {
        return (md.len(), mtime);
    }
    // A directory's own time changes with every file made or removed in it; its content tells the activity.
    let (mut bytes, mut newest, mut any) = (0, 0, false);
    if let Ok(rd) = fs::read_dir(path) {
        for e in rd.flatten() {
            let (b, t) = measure(&e.path());
            bytes += b;
            newest = newest.max(t);
            any = true;
        }
    }
    (bytes, if any { newest } else { mtime })
}

fn remove(path: &Path) -> std::io::Result<()> {
    if fs::symlink_metadata(path)?.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

/// What a pass did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Report {
    /// Game directories deleted.
    pub games: Vec<u32>,
    /// Log files deleted.
    pub logs: usize,
    /// Recordings deleted.
    pub recordings: Vec<String>,
    pub freed_bytes: u64,
}

impl Report {
    pub fn is_empty(&self) -> bool {
        self.games.is_empty() && self.logs == 0 && self.recordings.is_empty()
    }
}

// ---------------------------------------------------------------- saves

/// Deletes the directories of old finished games under `<state_dir>/games`.
pub fn prune_games(
    state_dir: &Path,
    now_ms: u64,
    live: &HashSet<u32>,
    cfg: &Retention,
    report: &mut Report,
) {
    let games = state_dir.join("games");
    let metas: HashMap<u32, meta::GameMeta> = meta::read_all(state_dir)
        .unwrap_or_default()
        .into_iter()
        .map(|m| (m.id, m))
        .collect();
    let mut entries = Vec::new();
    let Ok(rd) = fs::read_dir(&games) else {
        return;
    };
    for item in rd.flatten() {
        let path = item.path();
        let Some(id) = item
            .file_name()
            .to_str()
            .and_then(|n| n.parse::<u32>().ok())
            .filter(|_| path.is_dir())
        else {
            continue;
        };
        let (bytes, newest) = measure(&path);
        // Only a game that finished, has no actor and has a meta that says so can go.
        let finished = metas
            .get(&id)
            .filter(|m| m.status == MetaStatus::Finished)
            .map(|m| m.finished_ms.unwrap_or(newest));
        entries.push(Entry {
            name: id.to_string(),
            bytes,
            rank: u64::from(id),
            last_ms: finished.unwrap_or(newest),
            protected: finished.is_none() || live.contains(&id),
        });
    }
    let policy = Policy {
        keep_at_least: cfg.keep_games as usize,
        keep_at_most: None,
        max_age_ms: Some(u64::from(cfg.finished_max_age_days) * DAY_MS),
        max_bytes: None,
    };
    for name in select_evictions(&entries, now_ms, &policy) {
        let path = games.join(&name);
        match remove(&path) {
            Ok(()) => {
                let bytes = entries
                    .iter()
                    .find(|e| e.name == name)
                    .map_or(0, |e| e.bytes);
                tracing::info!(game = %name, bytes, "retention: deleted the saves of an old finished game");
                report.freed_bytes += bytes;
                if let Ok(id) = name.parse() {
                    report.games.push(id);
                }
            }
            Err(e) => {
                tracing::warn!(game = %name, error = %e, "retention: cannot delete a game directory")
            }
        }
    }
}

// ---------------------------------------------------------------- logs

/// `game-12.jsonl.2` -> `(Some(12), live = false)`; `worker-12.log` -> `(Some(12), live = true)`; names that
/// belong to no game number (`worker-describe.log`) give `None`.
fn log_owner(name: &str) -> Option<(Option<u32>, bool)> {
    let (rest, tail) = if let Some(r) = name.strip_prefix("game-") {
        (r, ".jsonl")
    } else if let Some(r) = name.strip_prefix("worker-") {
        (r, ".log")
    } else {
        return None;
    };
    let (id, ext) = rest.split_once('.')?;
    let ext = format!(".{ext}");
    let live = ext == tail;
    if !live {
        let n = ext.strip_prefix(tail)?.strip_prefix('.')?;
        if n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
    }
    Some((id.parse().ok(), live))
}

/// Cleans `<state_dir>/logs`: the logs of games whose directory is gone, then the oldest files beyond the cap.
pub fn prune_logs(
    state_dir: &Path,
    now_ms: u64,
    live: &HashSet<u32>,
    cfg: &Retention,
    report: &mut Report,
) {
    let logs = state_dir.join("logs");
    let Ok(rd) = fs::read_dir(&logs) else {
        return;
    };
    let mut entries = Vec::new();
    for item in rd.flatten() {
        let Some(name) = item.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Some((owner, is_live_file)) = log_owner(&name) else {
            continue;
        };
        let path = item.path();
        if !path.is_file() {
            continue;
        }
        let (bytes, mtime) = measure(&path);
        let running = owner.is_some_and(|id| live.contains(&id));
        let orphan = owner.is_some_and(|id| {
            !running
                && !state_dir.join("games").join(id.to_string()).exists()
                && now_ms.saturating_sub(mtime) > ORPHAN_GRACE_MS
        });
        if orphan {
            if remove(&path).is_ok() {
                report.logs += 1;
                report.freed_bytes += bytes;
            }
            continue;
        }
        entries.push(Entry {
            name,
            bytes,
            rank: mtime,
            last_ms: mtime,
            // The file a running game writes to; turned-over copies are fair game.
            protected: running && is_live_file,
        });
    }
    let policy = Policy {
        max_bytes: Some(cfg.logs_max_mb.saturating_mul(MB)),
        ..Policy::default()
    };
    for name in select_evictions(&entries, now_ms, &policy) {
        if remove(&logs.join(&name)).is_ok() {
            report.logs += 1;
            report.freed_bytes += entries
                .iter()
                .find(|e| e.name == name)
                .map_or(0, |e| e.bytes);
        }
    }
}

// ---------------------------------------------------------------- recordings

/// Keeps the newest `recordings_games` entries of `<state_dir>/recordings` within `recordings_max_mb`;
/// the names in `protected` (recordings of live games) stay whatever the rules say. The index file and any
/// loose file whose name starts with `index` are left alone. Nothing happens if the directory is missing.
pub fn prune_recordings(
    state_dir: &Path,
    now_ms: u64,
    protected: &HashSet<String>,
    cfg: &Retention,
    report: &mut Report,
) {
    let dir = state_dir.join("recordings");
    let Ok(rd) = fs::read_dir(&dir) else {
        return;
    };
    let mut entries = Vec::new();
    for item in rd.flatten() {
        let Some(name) = item.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if name.starts_with("index") || name.ends_with(".tmp") {
            continue;
        }
        let (bytes, newest) = measure(&item.path());
        entries.push(Entry {
            protected: protected.contains(&name),
            name,
            bytes,
            rank: newest,
            last_ms: newest,
        });
    }
    let policy = Policy {
        keep_at_least: 0,
        keep_at_most: Some(cfg.recordings_games as usize),
        max_age_ms: None,
        max_bytes: Some(cfg.recordings_max_mb.saturating_mul(MB)),
    };
    for name in select_evictions(&entries, now_ms, &policy) {
        if remove(&dir.join(&name)).is_ok() {
            tracing::info!(recording = %name, "retention: deleted an old recording");
            report.freed_bytes += entries
                .iter()
                .find(|e| e.name == name)
                .map_or(0, |e| e.bytes);
            report.recordings.push(name);
        }
    }
}

/// The recordings the pass must not touch: the newest one of every game the gateway has an actor for (it is being
/// written), and the ones a client report pins until its `until_ms`. A pin that has run out is removed here.
pub fn protected_recordings(state_dir: &Path, now_ms: u64, live: &HashSet<u32>) -> HashSet<String> {
    let dir = recordings_dir(state_dir);
    let mut out = HashSet::new();
    let mut newest: HashMap<u32, (u64, String)> = HashMap::new();
    let Ok(rd) = fs::read_dir(&dir) else {
        return out;
    };
    for item in rd.flatten() {
        let Some(name) = item.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Some((game, created)) = crate::game::recording::parse_dir_name(&name) else {
            continue;
        };
        if let Some(pin) = recording::reader::read_pin(&item.path()) {
            if pin.until_ms > now_ms {
                out.insert(name.clone());
            } else {
                let _ = fs::remove_file(item.path().join(recording::writer::PIN));
            }
        }
        if live.contains(&game) && newest.get(&game).is_none_or(|(c, _)| created > *c) {
            newest.insert(game, (created, name));
        }
    }
    out.extend(newest.into_values().map(|(_, n)| n));
    out
}

/// A client report keeps the recording of its game: `pin.json` in the newest recording of `game` (or the exact
/// `g<game>-<created_ms>` when the report names it). At most `cfg.max_pins` recordings are pinned; the one pinned
/// longest ago lets go. Returns the name of the recording, or `None` when the game has none.
pub fn pin_recording(
    state_dir: &Path,
    game: u32,
    created_ms: Option<u64>,
    report_id: &str,
    now_ms: u64,
    cfg: &crate::config::Recording,
) -> Option<String> {
    let dir = recordings_dir(state_dir);
    let mut mine: Vec<(u64, String)> = Vec::new();
    let mut pinned: Vec<(u64, String)> = Vec::new();
    for item in fs::read_dir(&dir).ok()?.flatten() {
        let name = item.file_name().to_str()?.to_owned();
        let Some((g, created)) = crate::game::recording::parse_dir_name(&name) else {
            continue;
        };
        if g == game && created_ms.is_none_or(|c| c == created) {
            mine.push((created, name.clone()));
        }
        if let Some(p) = recording::reader::read_pin(&item.path())
            && p.until_ms > now_ms
        {
            pinned.push((p.pinned_ms, name));
        }
    }
    let (_, name) = mine.into_iter().max_by_key(|(c, _)| *c)?;
    let pin = recording::reader::Pin {
        until_ms: now_ms + u64::from(cfg.pin_days) * DAY_MS,
        report: report_id.to_owned(),
        pinned_ms: now_ms,
    };
    let text = serde_json::to_vec(&pin).ok()?;
    recording::writer::write_atomic(&dir.join(&name).join(recording::writer::PIN), &text).ok()?;
    pinned.retain(|(_, n)| *n != name);
    pinned.sort();
    // The pin just made counts: keep `max_pins - 1` of the older ones.
    let keep = (cfg.max_pins as usize).saturating_sub(1);
    while pinned.len() > keep {
        let (_, old) = pinned.remove(0);
        let _ = fs::remove_file(dir.join(old).join(recording::writer::PIN));
    }
    Some(name)
}

// ---------------------------------------------------------------- the pass

/// One pass over the state directory. `live` are the games the gateway has an actor for.
pub fn run_pass(state_dir: &Path, now_ms: u64, live: &HashSet<u32>, cfg: &Retention) -> Report {
    let mut report = Report::default();
    prune_games(state_dir, now_ms, live, cfg, &mut report);
    // After the games: the logs of a deleted game are orphans now.
    prune_logs(state_dir, now_ms, live, cfg, &mut report);
    let protected = protected_recordings(state_dir, now_ms, live);
    prune_recordings(state_dir, now_ms, &protected, cfg, &mut report);
    report
}

/// Runs a pass now and then every `cfg.interval_min` minutes, on a blocking thread. Does nothing when
/// `cfg.enabled` is off or the server has no `state_dir`.
pub fn spawn_loop(shared: Arc<Shared>) {
    let cfg = shared.config.retention.clone();
    let Some(state_dir) = shared.config.state_dir.clone() else {
        return;
    };
    if !cfg.enabled {
        return;
    }
    tokio::spawn(async move {
        let every = Duration::from_secs(u64::from(cfg.interval_min.max(1)) * 60);
        loop {
            let live: HashSet<u32> = shared.games.list().iter().map(|g| g.info.id).collect();
            let (dir, c, now) = (
                state_dir.clone(),
                cfg.clone(),
                shared.config.wall_clock.unix_ms(),
            );
            match tokio::task::spawn_blocking(move || run_pass(&dir, now, &live, &c)).await {
                Ok(r) if !r.is_empty() => tracing::info!(
                    games = r.games.len(),
                    logs = r.logs,
                    recordings = r.recordings.len(),
                    freed_mb = r.freed_bytes / MB,
                    "retention pass done"
                ),
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e, "the retention pass failed"),
            }
            tokio::time::sleep(every).await;
        }
    });
}

/// `<state_dir>/recordings`, where P2 writes.
pub fn recordings_dir(state_dir: &Path) -> PathBuf {
    state_dir.join("recordings")
}

#[cfg(test)]
mod tests {
    use std::fs::File;

    use protocol::{AutoChoices, GameSettings, TimerMode, TurnMode};

    use super::meta::GameMeta;
    use super::*;

    const NOW: u64 = 100 * DAY_MS;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("civ5-retention-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("games")).unwrap();
        fs::create_dir_all(dir.join("logs")).unwrap();
        dir
    }

    fn settings() -> GameSettings {
        GameSettings {
            name: "t".into(),
            world_size: "WORLDSIZE_DUEL".into(),
            map_script: "m.lua".into(),
            map_options: Vec::new(),
            speed: "GAMESPEED_STANDARD".into(),
            start_era: "ERA_ANCIENT".into(),
            handicap: "HANDICAP_PRINCE".into(),
            minor_civs: 0,
            turn_mode: TurnMode::Hybrid,
            timer: TimerMode::Off,
            game_options: Vec::new(),
            seed: Some(1),
            map_seed: None,
            auto_choices: AutoChoices::default(),
            diplo_auto_decline: true,
        }
    }

    /// A game dir with a meta of the given status, a 1000-byte save and `finished` days before `NOW`.
    fn game(dir: &Path, id: u32, status: MetaStatus, finished_days_ago: Option<u64>) {
        let g = dir.join("games").join(id.to_string());
        fs::create_dir_all(g.join("ring")).unwrap();
        fs::write(g.join("ring/00000001-t0001.c5sv"), vec![0u8; 1000]).unwrap();
        GameMeta {
            id,
            name: format!("g{id}"),
            status,
            settings: settings(),
            host: "h".into(),
            seats: Vec::new(),
            seed: 1,
            core_build: "b".into(),
            created_ms: 0,
            finished_ms: finished_days_ago.map(|d| NOW - d * DAY_MS),
            turn: 5,
        }
        .write(&g)
        .unwrap();
    }

    fn set_mtime(path: &Path, ms: u64) {
        let f = File::options().write(true).open(path).unwrap();
        f.set_modified(UNIX_EPOCH + Duration::from_millis(ms))
            .unwrap();
    }

    fn cfg() -> Retention {
        Retention {
            keep_games: 2,
            ..Retention::default()
        }
    }

    fn exists(dir: &Path, rel: &str) -> bool {
        dir.join(rel).exists()
    }

    #[test]
    fn the_age_rule_keeps_the_newest_and_everything_not_finished() {
        let dir = scratch("games");
        // 1: old finished, 2: old finished, 3: old finished, 4: running, 5: hibernating, 6: faulted,
        // 7: finished 40 days ago, 8: finished 29 days ago, 9: finished 1 day ago.
        game(&dir, 1, MetaStatus::Finished, Some(90));
        game(&dir, 2, MetaStatus::Finished, Some(80));
        game(&dir, 3, MetaStatus::Finished, Some(70));
        game(&dir, 4, MetaStatus::Running, None);
        game(&dir, 5, MetaStatus::Hibernating, None);
        game(&dir, 6, MetaStatus::Faulted, None);
        game(&dir, 7, MetaStatus::Finished, Some(40));
        game(&dir, 8, MetaStatus::Finished, Some(29));
        game(&dir, 9, MetaStatus::Finished, Some(1));
        let r = run_pass(&dir, NOW, &HashSet::new(), &cfg());
        // keep_games = 2: games 9 and 8 are the newest; 7 is 40 days old and goes with 3, 2, 1.
        let mut gone = r.games.clone();
        gone.sort_unstable();
        assert_eq!(gone, vec![1, 2, 3, 7]);
        for kept in [4, 5, 6, 8, 9] {
            assert!(
                exists(&dir, &format!("games/{kept}/meta.json")),
                "game {kept}"
            );
        }
        assert!(r.freed_bytes >= 4000);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_game_with_an_actor_or_without_a_meta_is_never_deleted() {
        let dir = scratch("live");
        game(&dir, 1, MetaStatus::Finished, Some(90));
        game(&dir, 2, MetaStatus::Finished, Some(90));
        // No meta at all.
        fs::create_dir_all(dir.join("games/3")).unwrap();
        fs::write(dir.join("games/3/x"), b"x").unwrap();
        // Not a game directory.
        fs::create_dir_all(dir.join("games/notes")).unwrap();
        let live: HashSet<u32> = [1].into();
        let cfg = Retention {
            keep_games: 0,
            ..Retention::default()
        };
        let r = run_pass(&dir, NOW, &live, &cfg);
        assert_eq!(r.games, vec![2]);
        assert!(exists(&dir, "games/1/meta.json"));
        assert!(exists(&dir, "games/3/x"));
        assert!(exists(&dir, "games/notes"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn nothing_is_deleted_within_the_age_even_beyond_the_count() {
        let dir = scratch("young");
        for id in 1..=5 {
            game(&dir, id, MetaStatus::Finished, Some(3));
        }
        let r = run_pass(&dir, NOW, &HashSet::new(), &cfg());
        assert!(r.is_empty(), "{r:?}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn logs_of_a_deleted_game_go_and_a_young_orphan_stays() {
        let dir = scratch("orphans");
        game(&dir, 5, MetaStatus::Running, None);
        let old = NOW - 2 * DAY_MS;
        for f in [
            "game-5.jsonl",
            "game-5.jsonl.1",
            "worker-5.log",
            "game-9.jsonl",
            "game-9.jsonl.2",
            "worker-9.log.3",
            "worker-describe.log",
            "unrelated.txt",
        ] {
            fs::write(dir.join("logs").join(f), vec![1u8; 100]).unwrap();
            set_mtime(&dir.join("logs").join(f), old);
        }
        // Game 11 has no directory but its log was written a minute ago: it may be starting.
        fs::write(dir.join("logs/game-11.jsonl"), b"x").unwrap();
        set_mtime(&dir.join("logs/game-11.jsonl"), NOW - 60_000);
        let r = run_pass(&dir, NOW, &HashSet::new(), &cfg());
        assert_eq!(r.logs, 3);
        for kept in [
            "game-5.jsonl",
            "game-5.jsonl.1",
            "worker-5.log",
            "worker-describe.log",
            "unrelated.txt",
            "game-11.jsonl",
        ] {
            assert!(exists(&dir, &format!("logs/{kept}")), "{kept}");
        }
        for gone in ["game-9.jsonl", "game-9.jsonl.2", "worker-9.log.3"] {
            assert!(!exists(&dir, &format!("logs/{gone}")), "{gone}");
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_log_cap_drops_the_oldest_but_never_the_file_of_a_running_game() {
        let dir = scratch("logcap");
        game(&dir, 1, MetaStatus::Running, None);
        game(&dir, 2, MetaStatus::Hibernating, None);
        let mb = vec![7u8; MB as usize];
        // Oldest first: the live log of running game 1 is the oldest of all.
        for (i, f) in [
            "game-1.jsonl",
            "game-2.jsonl.2",
            "game-2.jsonl.1",
            "game-2.jsonl",
        ]
        .iter()
        .enumerate()
        {
            fs::write(dir.join("logs").join(f), &mb).unwrap();
            set_mtime(
                &dir.join("logs").join(f),
                NOW - 10_000_000 + i as u64 * 1000,
            );
        }
        let live: HashSet<u32> = [1].into();
        let cfg = Retention {
            logs_max_mb: 2,
            ..Retention::default()
        };
        let r = run_pass(&dir, NOW, &live, &cfg);
        assert_eq!(r.logs, 2);
        assert!(exists(&dir, "logs/game-1.jsonl"), "the live log stays");
        assert!(!exists(&dir, "logs/game-2.jsonl.2"));
        assert!(!exists(&dir, "logs/game-2.jsonl.1"));
        assert!(exists(&dir, "logs/game-2.jsonl"));
        let _ = fs::remove_dir_all(&dir);
    }

    fn recording(dir: &Path, name: &str, mb: usize, mtime: u64) {
        let r = dir.join("recordings").join(name);
        fs::create_dir_all(&r).unwrap();
        fs::write(r.join("seg-0000.c5rc"), vec![3u8; mb * MB as usize]).unwrap();
        set_mtime(&r.join("seg-0000.c5rc"), mtime);
    }

    #[test]
    fn recordings_keep_the_newest_n_within_the_byte_cap_and_protect_live_ones() {
        let dir = scratch("rec");
        for i in 0..7u64 {
            recording(&dir, &format!("g{i}"), 1, NOW - (10 - i) * DAY_MS);
        }
        fs::write(dir.join("recordings/index.json"), b"{}").unwrap();
        let cfg = Retention {
            recordings_games: 5,
            recordings_max_mb: 1024,
            ..Retention::default()
        };
        let mut r = Report::default();
        prune_recordings(&dir, NOW, &HashSet::new(), &cfg, &mut r);
        r.recordings.sort();
        assert_eq!(r.recordings, vec!["g0", "g1"]);
        assert!(exists(&dir, "recordings/index.json"));

        // The byte cap wins over the count: 3 MB left allowed, 5 kept, the two oldest go; g2 is live.
        let cfg = Retention {
            recordings_games: 5,
            recordings_max_mb: 3,
            ..Retention::default()
        };
        let mut r = Report::default();
        let live: HashSet<String> = ["g2".to_owned()].into();
        prune_recordings(&dir, NOW, &live, &cfg, &mut r);
        r.recordings.sort();
        assert_eq!(r.recordings, vec!["g3", "g4"]);
        assert!(exists(&dir, "recordings/g2"), "live recording stays");
        assert!(exists(&dir, "recordings/g6"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_live_game_and_a_pin_protect_their_recordings_and_pins_are_capped_and_expire() {
        let dir = scratch("recpin");
        // Four games, one recording each (g<game>-<created>), game 4 is live; sizes would evict all but two.
        for g in 1..=4u64 {
            recording(&dir, &format!("g{g}-{g}"), 1, NOW - (10 - g) * DAY_MS);
        }
        let cfg = crate::config::Recording {
            max_pins: 2,
            pin_days: 14,
            ..crate::config::Recording::default()
        };
        // Reports for games 1, 2 and 3: the third pin lets go of the oldest.
        assert_eq!(
            pin_recording(&dir, 1, None, "r1", NOW, &cfg).as_deref(),
            Some("g1-1")
        );
        assert_eq!(
            pin_recording(&dir, 2, None, "r2", NOW + 1, &cfg).as_deref(),
            Some("g2-2")
        );
        assert_eq!(
            pin_recording(&dir, 3, None, "r3", NOW + 2, &cfg).as_deref(),
            Some("g3-3")
        );
        assert_eq!(
            pin_recording(&dir, 99, None, "r4", NOW, &cfg),
            None,
            "no recording, no pin"
        );
        let live: HashSet<u32> = [4].into();
        let prot = protected_recordings(&dir, NOW + 3, &live);
        let mut names: Vec<_> = prot.iter().cloned().collect();
        names.sort();
        assert_eq!(names, ["g2-2", "g3-3", "g4-4"]);
        let rc = Retention {
            recordings_games: 1,
            recordings_max_mb: 1024,
            ..Retention::default()
        };
        let r = run_pass(&dir, NOW + 3, &live, &rc);
        assert_eq!(r.recordings, vec!["g1-1"]);
        assert!(exists(&dir, "recordings/g2-2") && exists(&dir, "recordings/g3-3"));
        assert!(exists(&dir, "recordings/g4-4"));
        // Fifteen days later the pins have run out: only the live game stays.
        let later = NOW + 15 * DAY_MS;
        let r = run_pass(&dir, later, &live, &rc);
        let mut gone = r.recordings.clone();
        gone.sort();
        assert_eq!(gone, vec!["g2-2", "g3-3"]);
        assert!(exists(&dir, "recordings/g4-4"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_report_names_the_recording_it_pins_exactly() {
        let dir = scratch("recexact");
        recording(&dir, "g5-100", 1, NOW);
        recording(&dir, "g5-200", 1, NOW);
        let cfg = crate::config::Recording::default();
        assert_eq!(
            pin_recording(&dir, 5, Some(100), "r", NOW, &cfg).as_deref(),
            Some("g5-100")
        );
        assert_eq!(
            pin_recording(&dir, 5, None, "r", NOW, &cfg).as_deref(),
            Some("g5-200")
        );
        assert_eq!(pin_recording(&dir, 5, Some(300), "r", NOW, &cfg), None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn evictions_are_pure_and_ranked() {
        let e = |n: &str, rank, bytes| Entry {
            name: n.into(),
            bytes,
            rank,
            last_ms: rank,
            protected: false,
        };
        let v = [e("a", 1, 10), e("b", 2, 10), e("c", 3, 10), e("d", 4, 10)];
        let p = Policy {
            keep_at_most: Some(3),
            ..Policy::default()
        };
        assert_eq!(select_evictions(&v, 0, &p), vec!["a"]);
        let p = Policy {
            max_bytes: Some(15),
            ..Policy::default()
        };
        assert_eq!(select_evictions(&v, 0, &p), vec!["a", "b", "c"]);
        let p = Policy {
            keep_at_least: 3,
            max_age_ms: Some(1),
            ..Policy::default()
        };
        // Only "a" is beyond the floor, and 1 ms is old enough for it.
        assert_eq!(select_evictions(&v, 100, &p), vec!["a"]);
    }
}
