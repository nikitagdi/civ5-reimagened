//! What a bot measures and the histogram `H` of its result.

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::{Value, json};

/// A list of samples; `summary` is the `H` of `result.json`.
#[derive(Debug, Default, Clone)]
pub struct Hist(Vec<f64>);

/// `{n, min, p50, p95, p99, max, mean}`; empty fields are `null` when there are no samples.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct H {
    pub n: usize,
    pub min: Option<f64>,
    pub p50: Option<f64>,
    pub p95: Option<f64>,
    pub p99: Option<f64>,
    pub max: Option<f64>,
    pub mean: Option<f64>,
}

fn round1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

impl Hist {
    pub fn push(&mut self, v: f64) {
        if v.is_finite() {
            self.0.push(v);
        }
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn summary(&self) -> H {
        if self.0.is_empty() {
            return H {
                n: 0,
                min: None,
                p50: None,
                p95: None,
                p99: None,
                max: None,
                mean: None,
            };
        }
        let mut v = self.0.clone();
        v.sort_by(f64::total_cmp);
        // Nearest rank.
        let at = |p: f64| v[(((p / 100.0) * v.len() as f64).ceil() as usize).clamp(1, v.len()) - 1];
        H {
            n: v.len(),
            min: Some(round1(v[0])),
            p50: Some(round1(at(50.0))),
            p95: Some(round1(at(95.0))),
            p99: Some(round1(at(99.0))),
            max: Some(round1(v[v.len() - 1])),
            mean: Some(round1(v.iter().sum::<f64>() / v.len() as f64)),
        }
    }
}

/// `{msgs, raw_bytes}` of one message kind.
#[derive(Debug, Default, Clone, Copy)]
pub struct KindBytes {
    pub msgs: u64,
    /// Size of the message as postcard bytes before compression.
    pub raw_bytes: u64,
}

#[derive(Debug, Default, Clone)]
pub struct Stats {
    pub turns_played: i32,
    /// Order latency: the frame handed to the socket to the batch with the answer read.
    pub latency_ms: Hist,
    pub rtt_ms: Hist,
    pub down_body: u64,
    pub down_msgs: u64,
    pub down_by_kind: BTreeMap<String, KindBytes>,
    pub up_body: u64,
    pub up_msgs: u64,
    /// Keyframes (without the basis): count and largest raw size.
    pub keyframes_n: u64,
    pub keyframe_max_bytes: u64,
    /// The view of the first keyframe that checked out: `(turn, view hash)`.
    pub first_view: Option<(i32, u64)>,
    /// `MapBasis` messages; `basis_skipped` is the entries with `Fresh` where none came.
    pub basis_n: u64,
    pub basis_skipped: u64,
    pub basis_max_bytes: u64,
    /// Batches with `MapBasis` and `Keyframe` together: count and largest wire size.
    pub cold_n: u64,
    pub cold_max_bytes: u64,
    /// `(down_body, up_body)` per turn.
    pub per_turn: BTreeMap<i32, (u64, u64)>,
    pub hash_checked: u64,
    pub hash_mismatch: u64,
    pub need_keyframe: BTreeMap<String, u64>,
    pub queries_sent: u64,
    pub contexts_used: u64,
    pub reconnects: Vec<Value>,
    pub refused_offline: u64,
    pub host_changes: Vec<Value>,
    pub errors: Vec<String>,
    pub lobby_errors: Vec<String>,
    pub ignored_diffs: u64,
}

impl Stats {
    pub fn error(&mut self, text: impl Into<String>) {
        let text = text.into();
        // The list stays readable on a long run: the first 200 are kept.
        if self.errors.len() < 200 {
            self.errors.push(text);
        }
    }

    /// The `bytes` object of `result.json`.
    pub fn bytes_json(&self) -> Value {
        let by_kind: BTreeMap<&String, Value> = self
            .down_by_kind
            .iter()
            .map(|(k, v)| (k, json!({ "msgs": v.msgs, "raw_bytes": v.raw_bytes })))
            .collect();
        let per_turn: Vec<Value> = self
            .per_turn
            .iter()
            .map(|(t, (d, u))| json!({ "turn": t, "down_body": d, "up_body": u }))
            .collect();
        json!({
            "down_body": self.down_body,
            "down_msgs": self.down_msgs,
            "down_by_kind": by_kind,
            "up_body": self.up_body,
            "up_msgs": self.up_msgs,
            "keyframes": { "n": self.keyframes_n, "max_bytes": self.keyframe_max_bytes },
            "basis": { "n": self.basis_n, "skipped": self.basis_skipped, "max_bytes": self.basis_max_bytes },
            "cold_batches": { "n": self.cold_n, "max_bytes": self.cold_max_bytes },
            "per_turn": per_turn,
        })
    }
}
