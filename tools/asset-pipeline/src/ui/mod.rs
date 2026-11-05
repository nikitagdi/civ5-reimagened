//! The `ui` asset set: UI files by package priority, textures, fonts, atlases, texts and sounds
//!.

pub mod atlas;
pub mod audio;
pub mod audio3d;
pub mod cyrillic;
pub mod oracle;
pub mod overrides;
pub mod patches;
pub mod pkg;
pub mod refs;
pub mod sql;
pub mod tables;
pub mod tex;
pub mod text;
pub mod vfs;

mod build;

pub use build::{
    CollisionSummary, LayerRow, OverrideSummary, SET_NAME, UiError, UiOptions, UiReportDoc,
    UiSummary, build_ui, collision_summary, icon_font_entries, layer_rows, open_vfs,
    override_report, xml_texts,
};
