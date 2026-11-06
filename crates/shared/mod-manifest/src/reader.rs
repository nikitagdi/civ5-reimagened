//! A strict reader over `toml::Table`: every key must be consumed, otherwise `finish` reports it as unknown.
//! Errors carry the key path.

use toml::Value;

use crate::error::ManifestError;

pub(crate) type Res<T> = Result<T, ManifestError>;

pub(crate) fn kind(v: &Value) -> &'static str {
    match v {
        Value::String(_) => "string",
        Value::Integer(_) => "integer",
        Value::Float(_) => "float",
        Value::Boolean(_) => "boolean",
        Value::Datetime(_) => "datetime",
        Value::Array(_) => "array",
        Value::Table(_) => "table",
    }
}

pub(crate) fn wrong_type(path: &str, expected: &'static str, v: &Value) -> ManifestError {
    ManifestError::WrongType {
        path: path.to_string(),
        expected,
        found: kind(v),
    }
}

pub(crate) struct Tbl<'a> {
    map: &'a toml::Table,
    path: String,
    used: Vec<&'static str>,
}

impl<'a> Tbl<'a> {
    pub(crate) fn new(map: &'a toml::Table, path: impl Into<String>) -> Self {
        Self {
            map,
            path: path.into(),
            used: Vec::new(),
        }
    }

    pub(crate) fn from_value(v: &'a Value, path: &str) -> Res<Self> {
        match v {
            Value::Table(t) => Ok(Self::new(t, path)),
            other => Err(wrong_type(path, "table", other)),
        }
    }

    pub(crate) fn path_of(&self, key: &str) -> String {
        if self.path.is_empty() {
            key.to_string()
        } else {
            format!("{}.{key}", self.path)
        }
    }

    pub(crate) fn get(&mut self, key: &'static str) -> Option<&'a Value> {
        self.used.push(key);
        self.map.get(key)
    }

    pub(crate) fn require(&mut self, key: &'static str) -> Res<&'a Value> {
        self.get(key).ok_or_else(|| ManifestError::Missing {
            path: self.path_of(key),
        })
    }

    pub(crate) fn str_opt(&mut self, key: &'static str) -> Res<Option<&'a str>> {
        match self.get(key) {
            None => Ok(None),
            Some(Value::String(s)) => Ok(Some(s)),
            Some(other) => Err(wrong_type(&self.path_of(key), "string", other)),
        }
    }

    pub(crate) fn str_req(&mut self, key: &'static str) -> Res<&'a str> {
        self.str_opt(key)?.ok_or_else(|| ManifestError::Missing {
            path: self.path_of(key),
        })
    }

    /// Integer in `lo..=hi`.
    pub(crate) fn int_opt(&mut self, key: &'static str, lo: i64, hi: i64) -> Res<Option<i64>> {
        match self.get(key) {
            None => Ok(None),
            Some(Value::Integer(n)) if (lo..=hi).contains(n) => Ok(Some(*n)),
            Some(Value::Integer(n)) => Err(ManifestError::invalid(
                self.path_of(key),
                format!("{n} is out of range {lo}..={hi}"),
            )),
            Some(other) => Err(wrong_type(&self.path_of(key), "integer", other)),
        }
    }

    pub(crate) fn table_opt(&mut self, key: &'static str) -> Res<Option<Tbl<'a>>> {
        match self.get(key) {
            None => Ok(None),
            Some(v) => Ok(Some(Tbl::from_value(v, &self.path_of(key))?)),
        }
    }

    /// Array of strings; each item is returned with its path (`parts[1]`).
    pub(crate) fn strings_opt(&mut self, key: &'static str) -> Res<Option<Vec<(String, &'a str)>>> {
        let Some(v) = self.get(key) else {
            return Ok(None);
        };
        let path = self.path_of(key);
        let Value::Array(items) = v else {
            return Err(wrong_type(&path, "array", v));
        };
        items
            .iter()
            .enumerate()
            .map(|(i, item)| match item {
                Value::String(s) => Ok((format!("{path}[{i}]"), s.as_str())),
                other => Err(wrong_type(&format!("{path}[{i}]"), "string", other)),
            })
            .collect::<Res<Vec<_>>>()
            .map(Some)
    }

    /// All entries as (path, key, value), for tables with free-form keys (`[deps]`). Marks everything as used.
    pub(crate) fn entries(self) -> Vec<(String, &'a str, &'a Value)> {
        self.map
            .iter()
            .map(|(k, v)| (self.path_of(k), k.as_str(), v))
            .collect()
    }

    /// Reports the first key nobody asked for (keys are visited in sorted order).
    pub(crate) fn finish(self) -> Res<()> {
        for k in self.map.keys() {
            if !self.used.contains(&k.as_str()) {
                return Err(ManifestError::UnknownKey {
                    path: self.path_of(k),
                });
            }
        }
        Ok(())
    }
}
