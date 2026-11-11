//! The sample mode: a source that plays a sample file instead of a live game.
//!
//! [`StaticSource`] fills the same store through the same [`ModelWriter`] as [`UiSource`](crate::UiSource), with the
//! same `frame` signature, so a host swaps one for the other. It writes the roots of the sample once (the store
//! diffs them as for any other source) and then does nothing: there is no replica, no session read, no game read.
//!
//! The sample's `ui` part belongs to the actions module (another owner than the source) and its `state` and
//! anchors to the host: they are handed out by [`StaticSource::ui`], [`StaticSource::state`] and
//! [`StaticSource::anchors`] and never written here.

use std::collections::BTreeMap;

use serde_json::Value as Json;
use ui2_model::r#gen::{Ui, put_root};
use ui2_model::model::PutError;
use ui2_model::{ModelWriter, Sample, SampleError, Schema, Value, validate};

use crate::{SourceFrame, SourceStats};

/// A source that plays a [`Sample`]: its roots are written on the first frame after [`StaticSource::new`] or
/// [`StaticSource::reset`], later frames write nothing.
pub struct StaticSource {
    schema: &'static Schema,
    sample: Sample,
    written: bool,
    errors: Vec<SampleError>,
}

impl StaticSource {
    /// A source of the built-in schema's sample.
    #[must_use]
    pub fn new(sample: Sample) -> StaticSource {
        StaticSource::with_schema(Schema::v1(), sample)
    }

    /// A source of a sample parsed against `schema` (the schema of the store it writes to).
    #[must_use]
    pub fn with_schema(schema: &'static Schema, sample: Sample) -> StaticSource {
        StaticSource {
            schema,
            sample,
            written: false,
            errors: Vec::new(),
        }
    }

    /// One frame: writes the sample's roots if they were not written yet. The frame's replica, session and the
    /// UI root are not looked at (the signature is [`UiSource::frame`](crate::UiSource::frame)'s). `deferred` is
    /// always 0; a root that cannot be written is left out and listed by [`StaticSource::errors`].
    pub fn frame(
        &mut self,
        _f: SourceFrame<'_, '_>,
        w: &mut ModelWriter<'_>,
        _ui: &Ui,
    ) -> SourceStats {
        self.write(w)
    }

    /// Like [`StaticSource::frame`], for a host that has no session to hand over in sample mode.
    pub fn write(&mut self, w: &mut ModelWriter<'_>) -> SourceStats {
        let mut stats = SourceStats::default();
        if self.written {
            return stats;
        }
        self.written = true;
        self.errors.clear();
        for (rid, value) in self.sample.roots() {
            let name = self.schema.root(*rid).name.as_str();
            // A sample made for another schema must not write a root of the wrong type.
            let ty = self.schema.root_type(*rid);
            if let Err(e) = validate(self.schema, &ty, value) {
                self.errors.push(SampleError::Root {
                    root: name.to_string(),
                    msg: e.to_string(),
                });
                continue;
            }
            match put_root(w, name, value) {
                None => self.errors.push(SampleError::UnknownRoot(name.to_string())),
                Some(Ok(())) => stats.roots_built += 1,
                Some(Err(PutError::Value(e))) => self.errors.push(SampleError::Root {
                    root: name.to_string(),
                    msg: e.to_string(),
                }),
                Some(Err(PutError::Store(e))) => self.errors.push(SampleError::Store {
                    root: name.to_string(),
                    msg: e.to_string(),
                }),
            }
        }
        for e in &self.errors {
            tracing::warn!(target: "ui2_source", "sample root not written: {e}");
        }
        stats
    }

    /// Forgets that the roots were written: the next frame writes them again (a reloaded sample, a new store).
    pub fn reset(&mut self) {
        self.written = false;
    }

    /// Replaces the sample (hot reload) and writes it on the next frame.
    pub fn set_sample(&mut self, sample: Sample) {
        self.sample = sample;
        self.written = false;
    }

    /// The roots the last write could not put into the store (empty for a sound sample).
    #[must_use]
    pub fn errors(&self) -> &[SampleError] {
        &self.errors
    }

    /// The sample played.
    #[must_use]
    pub fn sample(&self) -> &Sample {
        &self.sample
    }

    /// The sample's `ui` root value for the actions module (`None`: the sample has no `ui` part).
    #[must_use]
    pub fn ui(&self) -> Option<&Value> {
        self.sample.ui().map(|(_, v)| v)
    }

    /// The sample's skin-defined `state` object.
    #[must_use]
    pub fn state(&self) -> &BTreeMap<String, Json> {
        self.sample.state()
    }

    /// The sample's anchors: screen points in CSS pixels.
    #[must_use]
    pub fn anchors(&self) -> &BTreeMap<String, (f32, f32)> {
        self.sample.anchors()
    }
}
