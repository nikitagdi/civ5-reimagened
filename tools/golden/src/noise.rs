//! Noise and thresholds.
//!
//! Noise is the metric between two captures of the original game of one scene (S1a against S1b).
//! `threshold = max(1.5 * noise, noise + margin, floor)` per mask and metric.

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::metrics::MaskMetrics;

/// Factor of the noise in the threshold (G-D9).
pub const NOISE_FACTOR: f64 = 1.5;

/// The three metrics that have thresholds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Metric {
    FlipMean,
    OneMinusSsim,
    De2000Mean,
}

impl Metric {
    pub const ALL: [Metric; 3] = [Metric::FlipMean, Metric::OneMinusSsim, Metric::De2000Mean];

    /// Key in `thresholds.toml`.
    pub fn key(self) -> &'static str {
        match self {
            Metric::FlipMean => "flip_mean",
            Metric::OneMinusSsim => "one_minus_ssim",
            Metric::De2000Mean => "de2000_mean",
        }
    }

    pub fn value(self, m: &MaskMetrics) -> f64 {
        match self {
            Metric::FlipMean => m.flip.mean,
            Metric::OneMinusSsim => m.one_minus_ssim,
            Metric::De2000Mean => m.de2000.mean,
        }
    }
}

/// Margin and floor of one metric.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub margin: f64,
    pub floor: f64,
}

impl Rule {
    /// `max(1.5 * noise, noise + margin, floor)`.
    pub fn threshold(&self, noise: f64) -> f64 {
        (NOISE_FACTOR * noise)
            .max(noise + self.margin)
            .max(self.floor)
    }
}

/// `thresholds.toml`: starting margins and floors, and the noise proposals written by `golden noise`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Thresholds {
    pub flip_mean: Rule,
    pub one_minus_ssim: Rule,
    pub de2000_mean: Rule,
    /// `[noise.<mask>] <metric> = value`, measured on S1a against S1b.
    #[serde(default)]
    pub noise: BTreeMap<String, BTreeMap<String, f64>>,
}

impl Thresholds {
    pub fn parse(text: &str) -> Result<Self> {
        toml::from_str(text).context("thresholds.toml")
    }

    pub fn rule(&self, m: Metric) -> Rule {
        match m {
            Metric::FlipMean => self.flip_mean,
            Metric::OneMinusSsim => self.one_minus_ssim,
            Metric::De2000Mean => self.de2000_mean,
        }
    }

    /// Threshold of a metric on a mask. Without a measured noise the noise is 0, which gives the floor.
    pub fn threshold(&self, mask: &str, m: Metric) -> f64 {
        let noise = self
            .noise
            .get(mask)
            .and_then(|t| t.get(m.key()))
            .copied()
            .unwrap_or(0.0);
        self.rule(m).threshold(noise)
    }
}

/// Text of the `[noise.*]` tables for `thresholds.toml` plus the resulting thresholds as comments
/// (a proposal; changing thresholds is a record in BUILD section 9). `noise` is the metrics per
/// mask of S1a against S1b.
pub fn proposal(rules: &Thresholds, noise: &BTreeMap<String, MaskMetrics>, date: &str) -> String {
    let mut out = format!("# proposal {date}: S1a against S1b\n");
    for (mask, m) in noise {
        out.push_str(&format!("[noise.{mask}]\n"));
        for metric in Metric::ALL {
            out.push_str(&format!("{} = {:.6}\n", metric.key(), metric.value(m)));
        }
    }
    out.push_str("# threshold = max(1.5 * noise, noise + margin, floor)\n");
    for (mask, m) in noise {
        out.push_str(&format!("# threshold.{mask}:"));
        for metric in Metric::ALL {
            let t = rules.rule(metric).threshold(metric.value(m));
            out.push_str(&format!(" {}={:.6}", metric.key(), t));
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::MaskStats;

    const FILE: &str = include_str!("../thresholds.toml");

    #[test]
    fn starting_values_of_spec_4_8() {
        let t = Thresholds::parse(FILE).unwrap();
        assert_eq!(
            t.flip_mean,
            Rule {
                margin: 0.02,
                floor: 0.05
            }
        );
        assert_eq!(
            t.one_minus_ssim,
            Rule {
                margin: 0.02,
                floor: 0.10
            }
        );
        assert_eq!(
            t.de2000_mean,
            Rule {
                margin: 0.5,
                floor: 2.0
            }
        );
    }

    #[test]
    fn threshold_formula() {
        let r = Rule {
            margin: 0.02,
            floor: 0.05,
        };
        // The floor wins when the noise is small.
        assert_eq!(r.threshold(0.0), 0.05);
        assert_eq!(r.threshold(0.01), 0.05);
        // noise + margin wins for noise below 0.04 (x1.5 is less than +0.02): 0.03 + 0.02 = 0.05.
        assert!((r.threshold(0.035) - 0.055).abs() < 1e-12);
        // 1.5 * noise wins for big noise.
        assert!((r.threshold(0.2) - 0.3).abs() < 1e-12);
    }

    #[test]
    fn threshold_from_measured_noise_and_default() {
        let mut t = Thresholds::parse(FILE).unwrap();
        assert_eq!(t.threshold("land", Metric::De2000Mean), 2.0);
        t.noise
            .entry("land".into())
            .or_default()
            .insert("de2000_mean".into(), 3.0);
        assert!((t.threshold("land", Metric::De2000Mean) - 4.5).abs() < 1e-12);
        assert_eq!(t.threshold("water", Metric::De2000Mean), 2.0);
    }

    #[test]
    fn proposal_text_parses_back() {
        let t = Thresholds::parse(FILE).unwrap();
        let stat = MaskStats {
            mean: 0.1,
            p95: 0.3,
            count: 10,
        };
        let m = MaskMetrics {
            flip: stat,
            one_minus_ssim: 0.03,
            de2000: stat,
        };
        let mut noise = BTreeMap::new();
        noise.insert("land".to_string(), m);
        let text = proposal(&t, &noise, "2026-10-21");
        let back = Thresholds::parse(&format!("{FILE}\n{text}")).unwrap();
        assert_eq!(back.noise["land"]["flip_mean"], 0.1);
        assert_eq!(back.noise["land"]["one_minus_ssim"], 0.03);
    }
}
