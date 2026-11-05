//! The icons of the resources (world-objects WO-D11, WOB4): with the models of the landmarks the
//! improvements, camps and ruins are drawn as models and lose their icons; the resource keeps its
//! strategic-view icon on the ground, as a layer of its own that the switch `YieldIcons` turns off
//! (the golden scenes turn it off: the capture of the original has no such icon before the UI).
//! The icon is B1's (`entities::icons`, the south-east part of the plot).
//!
//! SIMPLIFIED(SIMP-012): the strategic-view icon of the art define on the ground instead of the
//! Lua icon of the original UI (R-D5).

use crate::entities::PlotDecor;

/// The plots as the icon layer of the models of `assets-b2` sees them: only the resource of a plot
/// is left.
pub fn resource_only<'a>(plots: impl IntoIterator<Item = &'a PlotDecor>) -> Vec<PlotDecor> {
    plots
        .into_iter()
        .filter(|p| p.resource.is_some())
        .map(|p| PlotDecor {
            x: p.x,
            y: p.y,
            resource: p.resource.clone(),
            ..PlotDecor::default()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::icons::{Slot, plot_icons};

    #[test]
    fn only_the_resource_keeps_its_icon() {
        let plots = [
            PlotDecor {
                x: 1,
                y: 1,
                resource: Some("ART_DEF_RESOURCE_IRON".into()),
                improvement: Some("ART_DEF_IMPROVEMENT_MINE".into()),
                pillaged: true,
                ..PlotDecor::default()
            },
            PlotDecor {
                x: 2,
                y: 1,
                improvement: Some("ART_DEF_IMPROVEMENT_FARM".into()),
                ..PlotDecor::default()
            },
            PlotDecor {
                x: 3,
                y: 1,
                improvement: Some("ART_DEF_IMPROVEMENT_BARBARIAN_CAMP".into()),
                special: true,
                ..PlotDecor::default()
            },
        ];
        let kept = resource_only(&plots);
        assert_eq!(kept.len(), 1);
        let icons = plot_icons(&kept[0]);
        assert_eq!(icons.len(), 1);
        assert_eq!(icons[0].0, Slot::Resource);
        assert_eq!(icons[0].1, "ART_DEF_RESOURCE_IRON");
        assert_eq!(
            icons[0].2, 1.0,
            "the pillage of the improvement does not dim it"
        );
    }
}
