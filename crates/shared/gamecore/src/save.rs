//! What a `GCS1` save says about itself (`rt_save.h`), read without the core.
//!
//! The 32-byte fixed header (magic, format version, header size, section count, body length, xxh64
//! of the body), then the first section, `header`: `key=value` lines with `format`, `build_id`,
//! `turn`. The checksum and the build id are checked by `gc_load_game`; here they are only read, so
//! a host can label a save (lobby, file header) before it has a game.

/// The `header` section of a `GCS1` save.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveInfo {
    /// `GCS1` format version.
    pub format: u32,
    /// Build of the core that wrote it; `gc_load_game` refuses saves of other builds.
    pub build_id: String,
    /// Game turn at the save.
    pub turn: i32,
}

const FIXED_HEADER: usize = 32;

impl SaveInfo {
    /// Reads the fixed header and the `header` section.
    ///
    /// # Errors
    /// The bytes are not a `GCS1` save, or its header section is missing or unreadable.
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() < 4 || &bytes[..4] != b"GCS1" {
            return Err("not a GCS1 save".into());
        }
        if bytes.len() < FIXED_HEADER {
            return Err("truncated GCS1 header".into());
        }
        let u32_at = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().expect("4 bytes"));
        let format = u32_at(4);
        let body = &bytes[FIXED_HEADER..];
        let name_len = body
            .get(..2)
            .map(|b| usize::from(u16::from_le_bytes([b[0], b[1]])))
            .ok_or("no sections")?;
        let name = body.get(2..2 + name_len).ok_or("truncated section name")?;
        if name != b"header" {
            return Err("the first section is not \"header\"".into());
        }
        let at = 2 + name_len;
        let size = body
            .get(at..at + 4)
            .map(|b| u32::from_le_bytes(b.try_into().expect("4 bytes")) as usize)
            .ok_or("truncated section size")?;
        let text = body
            .get(at + 4..at + 4 + size)
            .ok_or("truncated header section")?;
        let text = std::str::from_utf8(text).map_err(|_| "header section is not text")?;
        let value = |key: &str| {
            text.lines()
                .find_map(|l| l.strip_prefix(key)?.strip_prefix('='))
                .map(str::trim)
        };
        Ok(SaveInfo {
            format,
            build_id: value("build_id").ok_or("no build_id")?.to_owned(),
            turn: value("turn")
                .and_then(|t| t.parse().ok())
                .ok_or("no turn")?,
        })
    }
}
