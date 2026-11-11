//! `data-fit="shrink"` (with `data-fit-min`): a text that does not fit its box at its font size is shown
//! smaller.
//!
//! The box keeps the size layout gave it at the original font size; after layout the runs of the root are
//! shaped again with every font size scaled by one factor, at most four times: the first guess is the ratio
//! of the room to the natural width, each next one shrinks by the ratio still missing. Line heights stay as
//! they were, so the smaller text stays centered on the same baselines' lines.

use ui2_text::TextMeasure;

use crate::text::ifc::{Fitted, Ifc, Run};

/// Most shapings of one fit.
pub const MAX_FIT_MEASURES: usize = 4;

/// Fits the text of a root into the width layout gave it. Returns the runs shaped.
pub(crate) fn fit(ifc: &mut Ifc, tm: &mut dyn TextMeasure) -> u32 {
    let Some(min) = ifc.fit else {
        return 0;
    };
    let avail = ifc.width;
    let natural = ifc.lines.width;
    if ifc.fit_key == Some((avail, natural)) {
        return 0;
    }
    ifc.fit_key = Some((avail, natural));
    // SIMPLIFIED(SIMP-201): data-fit only shrinks, only a single line, and the box keeps the size it has at the original font size
    if natural <= avail || avail <= 0.0 || ifc.runs.is_empty() || ifc.font_size <= 0.0 {
        ifc.fitted = None;
        return 0;
    }
    let min_scale = min.map_or(0.0, |m| (m / ifc.font_size).min(1.0));
    let mut scale = (avail / natural).max(min_scale);
    let mut shaped = 0;
    let mut best: Option<Fitted> = None;
    for attempt in 0..MAX_FIT_MEASURES {
        let mut runs: Vec<Run> = Vec::with_capacity(ifc.runs.len());
        for r in &ifc.runs {
            let mut style = r.shaped.style;
            style.size = r.shaped.style.size * scale;
            shaped += 1;
            let Ok(s) = tm.shape(&style, &r.shaped.text) else {
                continue;
            };
            let mut vm = r.vm;
            if let Ok(m) = tm.metrics(style.face, style.size, style.wght) {
                vm.metrics = m;
            }
            runs.push(Run {
                shaped: s,
                vm,
                ..r.clone()
            });
        }
        let lines = ifc.break_runs(tm, &runs, None, avail);
        let w = lines.width;
        best = Some(Fitted { runs, lines, scale });
        if w <= avail + 1e-3 || scale <= min_scale || attempt + 1 == MAX_FIT_MEASURES {
            break;
        }
        scale = (scale * avail / w).min(scale * 0.99).max(min_scale);
    }
    ifc.fitted = best.map(Box::new);
    shaped
}
