//! A recording read into memory: the entries of every segment in order, the keyframes and their bodies.

use std::path::Path;

use anyhow::{Context, Result, bail};
use recording::Recording;
use recording::format::{Body, Entry, KeyframeRec, KfKind};
use recording::reader::{SegmentRead, Tail};

/// One entry with its place in the files.
pub struct Item {
    /// Index of the segment in the list.
    pub seg: usize,
    /// Index of the entry in its segment.
    pub idx: usize,
    /// The turn of the segment's keyframe (0 before the first one).
    pub turn: i32,
    pub entry: Entry,
}

pub struct Store {
    pub rec: Recording,
    pub segs: Vec<SegmentRead>,
}

impl Store {
    pub fn open(path: &Path) -> Result<Store> {
        let rec = Recording::open(path).map_err(anyhow::Error::msg)?;
        let segs = rec.read_all().map_err(anyhow::Error::msg)?;
        if segs.is_empty() {
            bail!("{}: the recording has no segment", path.display());
        }
        Ok(Store { rec, segs })
    }

    /// Every entry, in file order.
    pub fn items(&self) -> impl Iterator<Item = Item> + '_ {
        self.segs.iter().enumerate().flat_map(|(seg, s)| {
            s.entries.iter().enumerate().map(move |(idx, e)| Item {
                seg,
                idx,
                turn: s.header.turn,
                entry: e.clone(),
            })
        })
    }

    /// The keyframe record that opens segment `seg`, if it has one.
    pub fn keyframe_of(&self, seg: usize) -> Option<&KeyframeRec> {
        match self.segs.get(seg)?.entries.first().map(|e| &e.body) {
            Some(Body::Keyframe(k)) => Some(k),
            _ => None,
        }
    }

    /// The segment whose keyframe is turn `turn`.
    pub fn seg_of_turn(&self, turn: i32) -> Option<usize> {
        (0..self.segs.len()).find(|&i| self.keyframe_of(i).is_some_and(|k| k.turn == turn))
    }

    /// The settings and seats of the game (the last header that carries them).
    pub fn game_meta(
        &self,
    ) -> (
        Option<protocol::GameSettings>,
        Vec<protocol::SlotSetup>,
        String,
    ) {
        let (mut settings, mut seats, mut build) = (None, Vec::new(), String::new());
        for it in self.items() {
            if let Body::Header(h) = &it.entry.body {
                if h.settings.is_some() {
                    settings.clone_from(&h.settings);
                }
                if !h.seats.is_empty() {
                    seats.clone_from(&h.seats);
                }
                if !h.core_build.is_empty() {
                    build.clone_from(&h.core_build);
                }
            }
        }
        (settings, seats, build)
    }

    /// Torn tails, as text.
    pub fn damage(&self) -> Vec<String> {
        self.segs
            .iter()
            .filter_map(|s| match &s.tail {
                Tail::Torn(why) => Some(format!(
                    "{}: {why} ({} records read)",
                    s.path
                        .file_name()
                        .map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
                    s.entries.len()
                )),
                Tail::Clean => None,
            })
            .collect()
    }

    /// The unpacked GCS1 of the keyframe of segment `seg`: a full body is decoded, a patch is applied to the snapshot of
    /// the keyframe before it (found, patch by patch, from the nearest full body that is still on disk).
    pub fn raw_of(&self, seg: usize) -> Result<Vec<u8>> {
        let k = self
            .keyframe_of(seg)
            .with_context(|| format!("segment {seg} has no keyframe"))?;
        let file = k
            .file
            .as_deref()
            .with_context(|| format!("turn {}: the body of the keyframe was not kept", k.turn))?;
        let body = self
            .rec
            .keyframe_body(file, k.hash)
            .map_err(anyhow::Error::msg)
            .with_context(|| format!("turn {}", k.turn))?;
        match k.kind {
            KfKind::Full => wire::replica::decode_snapshot(&body)
                .map_err(|e| anyhow::anyhow!("turn {}: {e}", k.turn)),
            KfKind::Patch => {
                let from = k.from_raw_hash.unwrap_or_default();
                // The keyframe before this one whose raw snapshot has the hash the patch starts from.
                let prev = (0..seg)
                    .rev()
                    .find(|&i| self.keyframe_of(i).is_some_and(|p| p.raw_hash == from))
                    .with_context(|| {
                        format!(
                            "turn {}: the keyframe the patch starts from is not in the recording",
                            k.turn
                        )
                    })?;
                let base = self.raw_of(prev).with_context(|| {
                    format!(
                        "turn {}: the snapshot the patch starts from is not readable",
                        k.turn
                    )
                })?;
                wire::replica::apply_patch(&body, &base, from)
                    .map_err(|e| anyhow::anyhow!("turn {}: {e}", k.turn))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use recording::format::{HeaderRec, KeyframeRec, KfKind, VERSION};
    use recording::writer::{Config, RecWriter, Stats};

    use super::*;

    #[test]
    fn a_recording_opens_and_a_full_keyframe_unpacks_to_its_snapshot() {
        let dir = std::env::temp_dir().join(format!("civ5-replay-store-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let header = HeaderRec {
            format: VERSION,
            game: 3,
            gateway: "t".into(),
            core_build: "core-x".into(),
            seed: 1,
            wall_at_mono0_ms: 5,
            created_ms: 5,
            name: "t".into(),
            settings: None,
            seats: Vec::new(),
        };
        let raw = vec![7u8; 5000];
        let body = wire::replica::encode_snapshot(&raw).unwrap();
        let mut w = RecWriter::create(&dir, Config::default(), &header, Arc::new(Stats::default()))
            .unwrap();
        let entry = |body| recording::Entry {
            mono_us: 1,
            wall_ms: 6,
            body,
        };
        w.write(&entry(Body::Header(header.clone()))).unwrap();
        w.keyframe(
            entry(Body::Keyframe(KeyframeRec {
                turn: 12,
                epoch: 1,
                jseq: 0,
                raw_hash: protocol::xxh64(0, &raw),
                hash: protocol::xxh64(0, &body),
                kind: KfKind::Full,
                from_raw_hash: None,
                file: None,
                bytes: body.len() as u32,
                base: true,
            })),
            Some(&body),
        )
        .unwrap();
        w.finish(Some(9)).unwrap();
        let s = Store::open(&dir).unwrap();
        assert_eq!(s.segs.len(), 2);
        assert_eq!(s.seg_of_turn(12), Some(1));
        assert_eq!(s.seg_of_turn(13), None);
        assert_eq!(s.raw_of(1).unwrap(), raw);
        assert!(s.raw_of(0).is_err(), "segment 0 has no keyframe");
        assert!(s.damage().is_empty());
        assert_eq!(s.game_meta().2, "core-x");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
