//! `probe --kind dds`: decode every visible .dds and summarise formats and failures.

use std::collections::BTreeMap;
use std::panic::{self, AssertUnwindSafe};

use rayon::prelude::*;

use super::{TextureError, decode_dds, inspect_dds};
use crate::game::GameSource;

/// Examples kept per failure reason.
const EXAMPLES: usize = 5;

#[derive(Debug, Default, Clone)]
pub struct FormatStat {
    pub count: u64,
    /// Source file bytes.
    pub bytes: u64,
    pub decoded: u64,
    /// `(width, height)` of level 0 to count.
    pub sizes: BTreeMap<(u32, u32), u64>,
    /// Files whose declared mip count is a full chain.
    pub full_chain: u64,
}

#[derive(Debug, Default, Clone)]
pub struct DdsProbe {
    pub total: u64,
    pub decoded: u64,
    /// Format name to stats, counting every file whose header parsed.
    pub formats: BTreeMap<String, FormatStat>,
    /// Failure reason to (count, example file names).
    pub failures: BTreeMap<String, (u64, Vec<String>)>,
    /// Files that could not be read from disk or a pack.
    pub io_errors: Vec<String>,
    /// Decoder panics; always a bug.
    pub panics: Vec<String>,
}

impl DdsProbe {
    pub fn unsupported(&self) -> u64 {
        self.failures
            .iter()
            .filter(|(reason, _)| reason.starts_with("unsupported"))
            .map(|(_, (n, _))| n)
            .sum()
    }

    fn fail(&mut self, reason: String, name: &str) {
        let (n, examples) = self.failures.entry(reason).or_default();
        *n += 1;
        if examples.len() < EXAMPLES {
            examples.push(name.to_string());
        }
    }
}

enum Outcome {
    Io(String),
    Panic,
    Done {
        info: Option<(String, u32, u32, u32)>,
        bytes: u64,
        error: Option<TextureError>,
    },
}

/// Decodes all source mips of every visible .dds. Panics inside the decoder are caught and
/// reported, so one bad file does not hide the rest of the report.
pub fn probe_dds(src: &GameSource) -> DdsProbe {
    let entries: Vec<_> = src.iter_ext("dds").collect();
    let results: Vec<_> = entries
        .par_iter()
        .map(|entry| {
            let outcome = match src.read(entry) {
                Err(e) => Outcome::Io(e.to_string()),
                Ok(bytes) => {
                    panic::catch_unwind(AssertUnwindSafe(|| {
                        let info = inspect_dds(&bytes)
                            .ok()
                            .map(|i| (i.format_name, i.width, i.height, i.mip_count));
                        let error = decode_dds(&bytes).err();
                        (info, error, bytes.len() as u64)
                    }))
                    .map_or(Outcome::Panic, |(info, error, bytes)| {
                        Outcome::Done { info, bytes, error }
                    })
                }
            };
            (entry.name.as_str(), outcome)
        })
        .collect();

    let mut report = DdsProbe::default();
    for (name, outcome) in results {
        report.total += 1;
        match outcome {
            Outcome::Io(e) => report.io_errors.push(format!("{name}: {e}")),
            Outcome::Panic => report.panics.push(name.to_string()),
            Outcome::Done { info, bytes, error } => {
                if let Some((format, w, h, mips)) = info {
                    let stat = report.formats.entry(format).or_default();
                    stat.count += 1;
                    stat.bytes += bytes;
                    *stat.sizes.entry((w, h)).or_default() += 1;
                    if mips as usize >= super::full_chain_len(w, h) {
                        stat.full_chain += 1;
                    }
                    if error.is_none() {
                        stat.decoded += 1;
                    }
                }
                match error {
                    None => report.decoded += 1,
                    Some(e) => report.fail(e.reason(), name),
                }
            }
        }
    }
    report
}
