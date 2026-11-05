//! Full traversal of the object graph: forces every pointer, type and string to resolve.

use std::collections::HashSet;

use crate::types::NameSource;
use crate::{GrannyError, GrannyFile, Object, Ptr, Value};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WalkStats {
    pub objects: usize,
    pub arrays: usize,
    pub array_elements: usize,
    pub strings: usize,
    pub types: usize,
    pub members: usize,
    pub unknown_names: usize,
}

struct Walker<'f> {
    stack: Vec<Object<'f>>,
    seen_objects: HashSet<(Ptr, Ptr)>,
    seen_arrays: HashSet<(Ptr, Ptr, usize)>,
    stats: WalkStats,
}

impl<'f> Walker<'f> {
    fn push_ref(&mut self, o: Option<Object<'f>>) {
        if let Some(o) = o
            && self.seen_objects.insert((o.at(), o.type_def().at))
        {
            self.stack.push(o);
        }
    }

    fn value(&mut self, v: Value<'f>) -> Result<(), GrannyError> {
        match v {
            Value::Inline(o) if o.type_def().has_pointers() => self.stack.push(o),
            Value::Reference(r) | Value::VariantReference(r) => self.push_ref(r.get()?),
            Value::Array(a) => {
                let d = a.resolve()?;
                self.stats.arrays += 1;
                self.stats.array_elements += d.len();
                if let (Some(at), Some(ty)) = (d.at(), d.element_type())
                    && ty.has_pointers()
                    && self.seen_arrays.insert((at, ty.at, d.len()))
                {
                    self.stack.extend(d.iter());
                }
            }
            Value::ArrayOfReferences(a) => {
                self.stats.arrays += 1;
                for o in a.iter()? {
                    self.stats.array_elements += 1;
                    self.push_ref(o?);
                }
            }
            Value::String(s) => self.stats.strings += usize::from(s.get()?.is_some()),
            Value::Fixed(f) => {
                for e in f.iter() {
                    self.value(e)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
}

impl GrannyFile<'_> {
    /// Visits every object reachable from the root, resolving all pointers; the first malformed
    /// pointer, type or string is returned as an error. `visit` sees each non-inline object once.
    pub fn walk_with(&self, mut visit: impl FnMut(&Object<'_>)) -> Result<WalkStats, GrannyError> {
        let root = self.root()?;
        let mut w = Walker {
            stack: vec![root],
            seen_objects: HashSet::new(),
            seen_arrays: HashSet::new(),
            stats: WalkStats::default(),
        };
        while let Some(o) = w.stack.pop() {
            w.stats.objects += 1;
            visit(&o);
            for i in 0..o.members().len() {
                if let Some(v) = o.value(i) {
                    w.value(v)?;
                }
            }
        }
        let mut stats = w.stats;
        for t in self.parsed_types() {
            stats.types += 1;
            stats.members += t.members.len();
            stats.unknown_names += t
                .members
                .iter()
                .filter(|m| m.name_source == NameSource::Unknown)
                .count();
        }
        Ok(stats)
    }

    pub fn walk(&self) -> Result<WalkStats, GrannyError> {
        self.walk_with(|_| {})
    }
}
