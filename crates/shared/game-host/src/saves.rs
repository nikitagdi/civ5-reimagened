//! The server save file `C5SV` version 3 (game-host-mp 4.11, GH-D9, GH-D10): bytes in, bytes out, no I/O.
//! The worker shell writes the files, the gateway reads their headers to list the saves of the lobby
//! ([`read_header`]).
//!
//! SIMPLIFIED(SIMP-019): our own `C5SV` file around the core's `GCS1` instead of `.Civ5Save`; saves by the
//! policy N-D14 (start of the human phase, every 60 s with an order applied, on request, at shutdown)
//! instead of one per turn. Version 2 (the browser game of B1, `autosave.rs`) is not read here.
//!
//! Layout (little-endian):
//!
//! | field | type |
//! |---|---|
//! | magic | `"C5SV"` |
//! | version | u16 = [`VERSION`] |
//! | header length | u32 (the sections start there) |
//! | seq | u64 |
//! | game | u32 |
//! | turn | i32 |
//! | saved at | u64, Unix ms |
//! | kind | u8: 0 ring, 1 keep |
//! | build id | u16 length + UTF-8 |
//! | sections | u8 count, then per section: tag `[u8; 4]`, length u32, xxh64 (seed 0) u64 |
//! | header checksum | u64, xxh64 (seed 0) of everything before it |
//!
//! then the sections one after the other in the order of the table: `GCS1` (the core's save, compressed with
//! `wire::compress`), `SEAT`, `SETG`, `JRNL`, `CMDS`, `CMEM`, and `MODS` only for a game with mods (all postcard;
//! `CMEM` may be missing: a save from before the memory of cities is read with an empty memory; a save without
//! `MODS` is a game without mods, byte for byte what this build wrote before the mod system). A section the
//! reader does not know is skipped, one it needs and does not find is an error.

use protocol::{
    CmdId, Codec, GameId, GameSettings, JournalEntry, PlayerId, Postcard, SaveKind, SlotSetup,
    TeamId, xxh64,
};

use crate::seats::CityMemo;

/// The format version this build writes and reads.
pub const VERSION: u16 = 3;

const MAGIC: &[u8; 4] = b"C5SV";
const TAG_GCS1: [u8; 4] = *b"GCS1";
const TAG_SEAT: [u8; 4] = *b"SEAT";
const TAG_SETG: [u8; 4] = *b"SETG";
const TAG_JRNL: [u8; 4] = *b"JRNL";
const TAG_CMDS: [u8; 4] = *b"CMDS";
const TAG_CMEM: [u8; 4] = *b"CMEM";
const TAG_MODS: [u8; 4] = *b"MODS";

/// Most sections a header lists, and the biggest core save after unpacking: bounds for a damaged file.
const MAX_SECTIONS: usize = 32;
const MAX_GCS1: usize = 1 << 30;
/// Bounds for the `MODS` section of a damaged or hostile file: the `mods.lock` text and the number of mod states.
const MAX_MODS_LOCK: usize = 256 * 1024;
const MAX_MODS_STATES: usize = protocol::MAX_MODS;

/// Directories of a game's save directory.
pub const RING_DIR: &str = "ring";
pub const KEEP_DIR: &str = "keep";
/// Extension of a finished save; a file being written has `.tmp` after it.
pub const EXTENSION: &str = "c5sv";

/// What the header says about a save.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveHeader {
    /// Grows by one with every save of a game, also across a restart.
    pub seq: u64,
    pub game: GameId,
    pub turn: i32,
    /// Unix time, milliseconds.
    pub saved_at_ms: u64,
    pub kind: SaveKind,
    /// Core build id with the FIX/HYG flags: only the build that wrote a save loads it.
    pub build_id: String,
}

/// `MODS`: the frozen mod set of the game and the state of each server-side mod. Opaque to this crate (it does
/// not know the manifest types): `lock` is the `mods.lock` text of the set (it carries the set hash), and the
/// mod host checks it with `mod_manifest::parse_lock`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ModsSection {
    pub lock: String,
    /// `persist()` of every server-side mod that has state, by mod id, in load order.
    pub states: Vec<ModState>,
}

/// The saved state of one server-side mod.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ModState {
    pub id: String,
    pub state: Vec<u8>,
}

impl ModsSection {
    fn within_limits(&self) -> bool {
        self.lock.len() <= MAX_MODS_LOCK && self.states.len() <= MAX_MODS_STATES
    }
}

/// The sections of a save besides the core's own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveSections {
    /// `SEAT`: who held which slot.
    pub seats: Vec<SlotSetup>,
    /// `SETG`: the settings of the game.
    pub settings: GameSettings,
    /// `JRNL`: up to 300 lines of the journal of every human seat.
    pub journals: Vec<(PlayerId, Vec<JournalEntry>)>,
    /// `CMDS`: the last order of the seat the core took.
    pub cmds: Vec<(PlayerId, CmdId)>,
    /// `CMEM`: the last seen values of foreign cities, by team.
    pub city_memo: Vec<(TeamId, Vec<CityMemo>)>,
    /// `MODS`: `None` for a game without mods; then the section is not written.
    pub mods: Option<ModsSection>,
}

/// What the lobby needs of a save: the header, the seats and the settings. Reading it does not unpack the
/// core's part.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveInfo {
    pub header: SaveHeader,
    pub seats: Vec<SlotSetup>,
    pub settings: GameSettings,
}

/// A whole save, read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Save {
    pub info: SaveInfo,
    /// The core's save (`GCS1`), unpacked: what `Core::start(Load)` takes.
    pub gcs1: Vec<u8>,
    pub journals: Vec<(PlayerId, Vec<JournalEntry>)>,
    pub cmds: Vec<(PlayerId, CmdId)>,
    /// `None`: the save has no `CMEM` section.
    pub city_memo: Option<Vec<(TeamId, Vec<CityMemo>)>>,
    /// `None`: the save has no `MODS` section (a game without mods).
    pub mods: Option<ModsSection>,
}

/// Why a file is not a usable save. The text is for the log and the player.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveError {
    NotASave,
    /// Written by another version of the format.
    Version(u16),
    /// Shorter than its header says.
    Cut,
    /// A checksum does not match.
    Damaged(String),
    /// A section is missing or does not parse.
    Section(String),
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SaveError::NotASave => write!(f, "not a C5SV save"),
            SaveError::Version(v) => write!(f, "C5SV version {v}, this build reads {VERSION}"),
            SaveError::Cut => write!(f, "the save is cut short"),
            SaveError::Damaged(what) => write!(f, "the save is damaged ({what})"),
            SaveError::Section(what) => write!(f, "the save is unusable: {what}"),
        }
    }
}

impl std::error::Error for SaveError {}

fn kind_byte(k: SaveKind) -> u8 {
    match k {
        SaveKind::Ring => 0,
        SaveKind::Keep => 1,
    }
}

fn byte_kind(b: u8) -> Option<SaveKind> {
    match b {
        0 => Some(SaveKind::Ring),
        1 => Some(SaveKind::Keep),
        _ => None,
    }
}

fn pack<T: serde::Serialize>(v: &T) -> Result<Vec<u8>, SaveError> {
    // Postcard into a `Vec` fails only for a type it cannot encode; these are plain data. A failure is an
    // error of the save, not an empty section that is only found broken at wake time.
    Postcard
        .encode(v)
        .map_err(|e| SaveError::Section(format!("a section cannot be encoded: {e}")))
}

/// The file of a save: the header, then `GCS1` packed by `wire::compress`, then the other sections.
///
/// Panics when a section cannot be encoded (never for the real types); the server uses [`try_encode_with`].
pub fn encode(header: &SaveHeader, gcs1: &[u8], sections: &SaveSections) -> Vec<u8> {
    encode_with(header, gcs1, sections, wire::compress)
}

/// [`encode`] with the packer of the `GCS1` section given: `pack_gcs1` makes the whole section body
/// (`wire::compress_with`: the raw length, then a standard zstd frame). [`decode`] reads it with
/// `wire::decompress`, so saves of every packer load. The server packs with the C zstd library (6-7 times
/// smaller than the pure Rust encoder's fastest level; see `worker/saver.rs`).
pub fn encode_with(
    header: &SaveHeader,
    gcs1: &[u8],
    sections: &SaveSections,
    pack_gcs1: impl FnOnce(&[u8]) -> Vec<u8>,
) -> Vec<u8> {
    try_encode_with(header, gcs1, sections, pack_gcs1).expect("the sections of a save encode")
}

/// [`encode_with`] that reports a section it cannot encode instead of writing it empty.
pub fn try_encode_with(
    header: &SaveHeader,
    gcs1: &[u8],
    sections: &SaveSections,
    pack_gcs1: impl FnOnce(&[u8]) -> Vec<u8>,
) -> Result<Vec<u8>, SaveError> {
    let setg = pack(&sections.settings)?;
    encode_raw(header, gcs1, sections, setg, pack_gcs1)
}

/// [`try_encode_with`] with the bytes of the `SETG` section given (tests write the v12 layout with it).
fn encode_raw(
    header: &SaveHeader,
    gcs1: &[u8],
    sections: &SaveSections,
    setg: Vec<u8>,
    pack_gcs1: impl FnOnce(&[u8]) -> Vec<u8>,
) -> Result<Vec<u8>, SaveError> {
    let mut memo: Vec<&(TeamId, Vec<CityMemo>)> = sections.city_memo.iter().collect();
    memo.sort_by_key(|(t, _)| *t);
    let mut bodies: Vec<([u8; 4], Vec<u8>)> = vec![
        (TAG_GCS1, pack_gcs1(gcs1)),
        (TAG_SEAT, pack(&sections.seats)?),
        (TAG_SETG, setg),
        (TAG_JRNL, pack(&sections.journals)?),
        (TAG_CMDS, pack(&sections.cmds)?),
        (TAG_CMEM, pack(&memo)?),
    ];
    if let Some(mods) = &sections.mods {
        if !mods.within_limits() {
            return Err(SaveError::Section("the MODS section is too big".into()));
        }
        bodies.push((TAG_MODS, pack(mods)?));
    }
    Ok(assemble(header, &bodies))
}

/// The file from the header fields and the section bodies, in order.
fn assemble(header: &SaveHeader, bodies: &[([u8; 4], Vec<u8>)]) -> Vec<u8> {
    let build = clip(&header.build_id);
    let len = 4 + 2 + 4 + 8 + 4 + 4 + 8 + 1 + 2 + build.len() + 1 + bodies.len() * 16 + 8;
    let mut h = Vec::with_capacity(len);
    h.extend_from_slice(MAGIC);
    h.extend_from_slice(&VERSION.to_le_bytes());
    h.extend_from_slice(&u32::try_from(len).unwrap_or(u32::MAX).to_le_bytes());
    h.extend_from_slice(&header.seq.to_le_bytes());
    h.extend_from_slice(&header.game.to_le_bytes());
    h.extend_from_slice(&header.turn.to_le_bytes());
    h.extend_from_slice(&header.saved_at_ms.to_le_bytes());
    h.push(kind_byte(header.kind));
    h.extend_from_slice(&(build.len() as u16).to_le_bytes());
    h.extend_from_slice(build);
    h.push(bodies.len() as u8);
    for (tag, body) in bodies {
        h.extend_from_slice(tag);
        h.extend_from_slice(&u32::try_from(body.len()).unwrap_or(u32::MAX).to_le_bytes());
        h.extend_from_slice(&xxh64(0, body).to_le_bytes());
    }
    let sum = xxh64(0, &h);
    h.extend_from_slice(&sum.to_le_bytes());
    debug_assert_eq!(h.len(), len);
    let mut file = h;
    for (_, body) in bodies {
        file.extend_from_slice(body);
    }
    file
}

/// At most `u16::MAX` bytes of `s`, cut on a character boundary.
fn clip(s: &str) -> &[u8] {
    let mut end = s.len().min(usize::from(u16::MAX));
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s.as_bytes()[..end]
}

/// A cursor over the header bytes.
struct Cursor<'a> {
    b: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], SaveError> {
        let end = self.at.checked_add(n).ok_or(SaveError::Cut)?;
        let s = self.b.get(self.at..end).ok_or(SaveError::Cut)?;
        self.at = end;
        Ok(s)
    }

    fn u8(&mut self) -> Result<u8, SaveError> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, SaveError> {
        Ok(u16::from_le_bytes(
            self.take(2)?.try_into().map_err(|_| SaveError::Cut)?,
        ))
    }

    fn u32(&mut self) -> Result<u32, SaveError> {
        Ok(u32::from_le_bytes(
            self.take(4)?.try_into().map_err(|_| SaveError::Cut)?,
        ))
    }

    fn i32(&mut self) -> Result<i32, SaveError> {
        Ok(i32::from_le_bytes(
            self.take(4)?.try_into().map_err(|_| SaveError::Cut)?,
        ))
    }

    fn u64(&mut self) -> Result<u64, SaveError> {
        Ok(u64::from_le_bytes(
            self.take(8)?.try_into().map_err(|_| SaveError::Cut)?,
        ))
    }
}

/// The header checked and the sections found in the file.
struct Parsed<'a> {
    header: SaveHeader,
    sections: Vec<([u8; 4], &'a [u8])>,
}

impl<'a> Parsed<'a> {
    fn section(&self, tag: [u8; 4]) -> Option<&'a [u8]> {
        self.sections
            .iter()
            .find(|(t, _)| *t == tag)
            .map(|(_, b)| *b)
    }

    /// `SETG`: v13 layout, or the v12 one of saves written before the mod system (no mods).
    fn settings(&self) -> Result<GameSettings, SaveError> {
        let body = self
            .section(TAG_SETG)
            .ok_or_else(|| SaveError::Section("no SETG section".into()))?;
        GameSettings::decode_postcard(body)
            .map_err(|e| SaveError::Section(format!("SETG does not parse: {e}")))
    }

    /// A section that must be there, checked and parsed.
    fn need<T: serde::de::DeserializeOwned>(&self, tag: [u8; 4]) -> Result<T, SaveError> {
        let name = String::from_utf8_lossy(&tag).into_owned();
        let body = self
            .section(tag)
            .ok_or_else(|| SaveError::Section(format!("no {name} section")))?;
        Postcard
            .decode::<T>(body)
            .map_err(|e| SaveError::Section(format!("{name} does not parse: {e}")))
    }
}

fn parse(file: &[u8]) -> Result<Parsed<'_>, SaveError> {
    if file.len() < 4 || &file[..4] != MAGIC {
        return Err(SaveError::NotASave);
    }
    let mut c = Cursor { b: file, at: 4 };
    let version = c.u16()?;
    if version != VERSION {
        return Err(SaveError::Version(version));
    }
    let header_len = c.u32()? as usize;
    if header_len < 8 || header_len > file.len() {
        return Err(SaveError::Cut);
    }
    let (head, tail) = file.split_at(header_len);
    let (before, sum) = head.split_at(header_len - 8);
    let stored = u64::from_le_bytes(sum.try_into().map_err(|_| SaveError::Cut)?);
    if stored != xxh64(0, before) {
        return Err(SaveError::Damaged("header checksum".into()));
    }
    let mut c = Cursor { b: before, at: 10 };
    let seq = c.u64()?;
    let game = c.u32()?;
    let turn = c.i32()?;
    let saved_at_ms = c.u64()?;
    let kind = byte_kind(c.u8()?).ok_or_else(|| SaveError::Damaged("kind".into()))?;
    let n = usize::from(c.u16()?);
    let build_id = String::from_utf8_lossy(c.take(n)?).into_owned();
    let count = usize::from(c.u8()?);
    if count > MAX_SECTIONS {
        return Err(SaveError::Damaged("section table".into()));
    }
    let mut table = Vec::with_capacity(count);
    for _ in 0..count {
        let tag: [u8; 4] = c.take(4)?.try_into().map_err(|_| SaveError::Cut)?;
        let len = c.u32()? as usize;
        let sum = c.u64()?;
        table.push((tag, len, sum));
    }
    if c.at != before.len() {
        return Err(SaveError::Damaged("header length".into()));
    }
    let mut at = 0usize;
    let mut sections = Vec::with_capacity(count);
    for (tag, len, sum) in table {
        let end = at.checked_add(len).ok_or(SaveError::Cut)?;
        let body = tail.get(at..end).ok_or(SaveError::Cut)?;
        if xxh64(0, body) != sum {
            let name = String::from_utf8_lossy(&tag).into_owned();
            return Err(SaveError::Damaged(format!("section {name}")));
        }
        sections.push((tag, body));
        at = end;
    }
    if at != tail.len() {
        return Err(SaveError::Damaged("trailing bytes".into()));
    }
    Ok(Parsed {
        header: SaveHeader {
            seq,
            game,
            turn,
            saved_at_ms,
            kind,
            build_id,
        },
        sections,
    })
}

/// The header, seats and settings of a save file; every checksum of the file is checked, the core's part is
/// not unpacked. The gateway builds the list of saves of the lobby from it.
///
/// # Errors
/// The file is not a usable C5SV version 3 save.
pub fn read_header(file: &[u8]) -> Result<SaveInfo, SaveError> {
    let p = parse(file)?;
    let seats = p.need(TAG_SEAT)?;
    let settings = p.settings()?;
    Ok(SaveInfo {
        header: p.header,
        seats,
        settings,
    })
}

/// A whole save: [`read_header`] and the core's part unpacked, the journals, the last orders and the memory
/// of cities.
///
/// # Errors
/// The file is not a usable C5SV version 3 save.
pub fn decode(file: &[u8]) -> Result<Save, SaveError> {
    let p = parse(file)?;
    let packed = p
        .section(TAG_GCS1)
        .ok_or_else(|| SaveError::Section("no GCS1 section".into()))?;
    let gcs1 = wire::decompress(packed, MAX_GCS1)
        .map_err(|e| SaveError::Section(format!("GCS1 does not unpack: {e}")))?;
    let city_memo = match p.section(TAG_CMEM) {
        Some(_) => Some(p.need(TAG_CMEM)?),
        None => None,
    };
    let mods = match p.section(TAG_MODS) {
        Some(_) => {
            let m: ModsSection = p.need(TAG_MODS)?;
            if !m.within_limits() {
                return Err(SaveError::Section("the MODS section is too big".into()));
            }
            Some(m)
        }
        None => None,
    };
    Ok(Save {
        info: SaveInfo {
            seats: p.need(TAG_SEAT)?,
            settings: p.settings()?,
            header: p.header.clone(),
        },
        gcs1,
        journals: p.need(TAG_JRNL)?,
        cmds: p.need(TAG_CMDS)?,
        city_memo,
        mods,
    })
}

// ---------------------------------------------------------------- file names and the ring

/// `00000007-t0012.c5sv`.
pub fn file_name(seq: u64, turn: i32) -> String {
    format!("{seq:08}-t{turn:04}.{EXTENSION}")
}

/// The path of a save relative to the save directory of its game: `ring/<name>` or `keep/<name>`.
pub fn rel_path(kind: SaveKind, seq: u64, turn: i32) -> String {
    let dir = match kind {
        SaveKind::Ring => RING_DIR,
        SaveKind::Keep => KEEP_DIR,
    };
    format!("{dir}/{}", file_name(seq, turn))
}

/// `00000007-t0012.c5sv` -> `(7, 12)`; anything else (a `.tmp` file, another name) is `None`.
pub fn parse_file_name(name: &str) -> Option<(u64, i32)> {
    let stem = name.strip_suffix(".c5sv")?;
    let (seq, turn) = stem.split_once("-t")?;
    if seq.len() != 8 || !seq.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if turn.len() < 4 || !turn.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((seq.parse().ok()?, turn.parse().ok()?))
}

/// Where a save of the start of turn `turn` goes: `keep/` when `keep_every` divides the turn (kept for
/// good), else the ring. Saves in the middle of a turn always go to the ring.
pub fn start_kind(turn: i32, keep_every: u32) -> SaveKind {
    let k = i64::from(keep_every);
    if k > 0 && i64::from(turn) > 0 && i64::from(turn) % k == 0 {
        SaveKind::Keep
    } else {
        SaveKind::Ring
    }
}

/// The files of the ring to delete so that `ring` newest are left; `seqs` in any order.
pub fn ring_excess(seqs: &[u64], ring: usize) -> Vec<u64> {
    let mut v = seqs.to_vec();
    v.sort_unstable();
    v.dedup();
    let drop = v.len().saturating_sub(ring.max(1));
    v.truncate(drop);
    v
}

#[cfg(test)]
mod tests {
    use protocol::{AutoChoices, CivPick, JournalKind, SlotControl, TimerMode, TurnMode};

    use super::*;

    /// A type whose encoding fails: a section like it used to become an empty section.
    struct Unencodable;
    impl serde::Serialize for Unencodable {
        fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
            Err(serde::ser::Error::custom("no"))
        }
    }

    #[test]
    fn a_section_that_cannot_be_encoded_is_an_error_not_an_empty_section() {
        assert!(matches!(pack(&Unencodable), Err(SaveError::Section(_))));
        assert!(pack(&vec![1u8, 2, 3]).is_ok());
    }

    fn settings() -> GameSettings {
        GameSettings {
            name: "t".into(),
            world_size: "WORLDSIZE_DUEL".into(),
            map_script: "m.lua".into(),
            map_options: vec![("a".into(), 1)],
            speed: "GAMESPEED_STANDARD".into(),
            start_era: "ERA_ANCIENT".into(),
            handicap: "HANDICAP_PRINCE".into(),
            minor_civs: 0,
            turn_mode: TurnMode::Hybrid,
            timer: TimerMode::Off,
            game_options: vec![],
            seed: Some(3),
            map_seed: None,
            auto_choices: AutoChoices::default(),
            diplo_auto_decline: true,
            mods: Vec::new(),
        }
    }

    fn header() -> SaveHeader {
        SaveHeader {
            seq: 7,
            game: 3,
            turn: 12,
            saved_at_ms: 1_700_000_000_000,
            kind: SaveKind::Keep,
            build_id: "core-1+FIX".into(),
        }
    }

    fn sections() -> SaveSections {
        SaveSections {
            seats: vec![
                SlotSetup {
                    player: 0,
                    civ: CivPick::Civ("CIVILIZATION_ROME".into()),
                    control: SlotControl::Human { nick: "ann".into() },
                },
                SlotSetup {
                    player: 1,
                    civ: CivPick::Random,
                    control: SlotControl::Ai,
                },
            ],
            settings: settings(),
            journals: vec![(
                0,
                vec![JournalEntry {
                    turn: 11,
                    kind: JournalKind::Session,
                    text: "hello".into(),
                    at: Some((3, 4)),
                    simplification: None,
                }],
            )],
            cmds: vec![(0, 41)],
            city_memo: vec![(
                1,
                vec![CityMemo {
                    owner: 0,
                    id: 5,
                    population: 3,
                    hp: 10,
                    max_hp: 20,
                    flags: 1,
                    buildings: vec![2, 4],
                }],
            )],
            mods: None,
        }
    }

    /// A save written before protocol v13 holds `SETG` without the mod list: it still loads, with no mods.
    #[test]
    fn a_v12_save_still_loads() {
        let now = settings();
        let mut v12 = Postcard.encode(&now).unwrap();
        assert_eq!(v12.pop(), Some(0)); // the empty `mods` list is the last byte
        let file = encode_raw(&header(), b"core bytes", &sections(), v12, |g| {
            wire::compress(g)
        })
        .unwrap();
        let s = decode(&file).unwrap();
        assert_eq!(s.info.settings, now);
        assert!(s.info.settings.mods.is_empty());
        assert_eq!(read_header(&file).unwrap().settings, now);
    }

    #[test]
    fn a_save_goes_there_and_back() {
        let gcs1: Vec<u8> = (0..5000u32).flat_map(|i| (i % 251).to_le_bytes()).collect();
        let file = encode(&header(), &gcs1, &sections());
        let s = decode(&file).unwrap();
        assert_eq!(s.gcs1, gcs1);
        assert_eq!(s.info.header, header());
        assert_eq!(s.info.seats, sections().seats);
        assert_eq!(s.info.settings, settings());
        assert_eq!(s.journals, sections().journals);
        assert_eq!(s.cmds, sections().cmds);
        assert_eq!(s.city_memo, Some(sections().city_memo));
        // The same state is the same bytes.
        assert_eq!(file, encode(&header(), &gcs1, &sections()));
    }

    #[test]
    fn the_header_read_needs_no_unpacking_and_gives_seats_and_settings() {
        let file = encode(&header(), b"core bytes", &sections());
        let info = read_header(&file).unwrap();
        assert_eq!(info.header.build_id, "core-1+FIX");
        assert_eq!((info.header.seq, info.header.turn), (7, 12));
        assert_eq!(info.seats.len(), 2);
        assert_eq!(info.settings.name, "t");
    }

    #[test]
    fn damage_is_found_in_the_header_and_in_every_section() {
        let file = encode(&header(), &[7u8; 300], &sections());
        // A flipped byte anywhere is an error, never a silently different save.
        for at in (0..file.len()).step_by(3) {
            let mut bad = file.clone();
            bad[at] ^= 0x40;
            assert!(decode(&bad).is_err(), "byte {at} flipped");
        }
        for cut in [0, 3, 10, 40, file.len() - 1] {
            assert!(decode(&file[..cut]).is_err(), "cut at {cut}");
        }
        assert_eq!(decode(b"C5SX....").unwrap_err(), SaveError::NotASave);
        let mut v2 = file.clone();
        v2[4] = 2;
        assert_eq!(decode(&v2).unwrap_err(), SaveError::Version(2));
    }

    #[test]
    fn a_save_without_cmem_reads_with_no_memory() {
        // Build the file by hand without the CMEM section.
        let full = encode(&header(), b"core", &sections());
        let p = parse(&full).unwrap();
        let keep: Vec<([u8; 4], &[u8])> = p
            .sections
            .iter()
            .copied()
            .filter(|(t, _)| *t != TAG_CMEM)
            .collect();
        let build = header().build_id;
        let mut h = Vec::new();
        h.extend_from_slice(MAGIC);
        h.extend_from_slice(&VERSION.to_le_bytes());
        let len = 4 + 2 + 4 + 8 + 4 + 4 + 8 + 1 + 2 + build.len() + 1 + keep.len() * 16 + 8;
        h.extend_from_slice(&(len as u32).to_le_bytes());
        h.extend_from_slice(&7u64.to_le_bytes());
        h.extend_from_slice(&3u32.to_le_bytes());
        h.extend_from_slice(&12i32.to_le_bytes());
        h.extend_from_slice(&1u64.to_le_bytes());
        h.push(1);
        h.extend_from_slice(&(build.len() as u16).to_le_bytes());
        h.extend_from_slice(build.as_bytes());
        h.push(keep.len() as u8);
        for (t, b) in &keep {
            h.extend_from_slice(t);
            h.extend_from_slice(&(b.len() as u32).to_le_bytes());
            h.extend_from_slice(&xxh64(0, b).to_le_bytes());
        }
        let sum = xxh64(0, &h);
        h.extend_from_slice(&sum.to_le_bytes());
        for (_, b) in &keep {
            h.extend_from_slice(b);
        }
        let s = decode(&h).unwrap();
        assert_eq!(s.city_memo, None);
        assert_eq!(s.gcs1, b"core");
    }

    fn mods_section() -> ModsSection {
        ModsSection {
            lock: "# mods.lock
version = 1
"
            .into(),
            states: vec![
                ModState {
                    id: "echo".into(),
                    state: vec![1, 2, 3],
                },
                ModState {
                    id: "hello".into(),
                    state: Vec::new(),
                },
            ],
        }
    }

    fn tags(file: &[u8]) -> Vec<[u8; 4]> {
        parse(file)
            .unwrap()
            .sections
            .iter()
            .map(|(t, _)| *t)
            .collect()
    }

    /// A game without mods writes exactly the six sections it always did.
    #[test]
    fn no_mods_writes_no_mods_section() {
        let file = encode(&header(), b"core", &sections());
        assert_eq!(
            tags(&file),
            [TAG_GCS1, TAG_SEAT, TAG_SETG, TAG_JRNL, TAG_CMDS, TAG_CMEM]
        );
        assert_eq!(decode(&file).unwrap().mods, None);
    }

    #[test]
    fn the_mods_section_goes_there_and_back() {
        let mut sec = sections();
        sec.mods = Some(mods_section());
        let file = encode(&header(), b"core", &sec);
        assert_eq!(tags(&file).last(), Some(&TAG_MODS));
        let s = decode(&file).unwrap();
        assert_eq!(s.mods, Some(mods_section()));
        // The other sections are untouched by it.
        assert_eq!(s.cmds, sec.cmds);
        assert_eq!(s.city_memo, Some(sec.city_memo.clone()));
        assert_eq!(file, encode(&header(), b"core", &sec));
        // The lobby's header read ignores it.
        assert_eq!(read_header(&file).unwrap().settings, settings());
    }

    /// A reader that does not know `MODS` skips it (the rule for unknown sections): rebuilt without it, the
    /// same file reads as a game without mods.
    #[test]
    fn a_reader_without_mods_support_skips_the_section() {
        let mut sec = sections();
        sec.mods = Some(mods_section());
        let file = encode(&header(), b"core", &sec);
        let p = parse(&file).unwrap();
        let kept: Vec<([u8; 4], Vec<u8>)> = p
            .sections
            .iter()
            .filter(|(t, _)| *t != TAG_MODS)
            .map(|(t, b)| (*t, b.to_vec()))
            .collect();
        let old = assemble(&p.header, &kept);
        assert_eq!(old, encode(&header(), b"core", &sections()));
        assert_eq!(decode(&old).unwrap().mods, None);
    }

    #[test]
    fn the_mods_section_is_bounded() {
        // Seven sections fit the table with room to spare.
        const { assert!(7 + 8 <= MAX_SECTIONS) };
        let mut sec = sections();
        let mut m = mods_section();
        m.lock = "x".repeat(MAX_MODS_LOCK + 1);
        sec.mods = Some(m);
        assert!(matches!(
            try_encode_with(&header(), b"core", &sec, wire::compress),
            Err(SaveError::Section(_))
        ));
        // A hostile file that carries an oversized section anyway is refused on read.
        let mut big = mods_section();
        big.lock = "x".repeat(MAX_MODS_LOCK + 1);
        let plain = encode(&header(), b"core", &sections());
        let p = parse(&plain).unwrap();
        let mut bodies: Vec<([u8; 4], Vec<u8>)> =
            p.sections.iter().map(|(t, b)| (*t, b.to_vec())).collect();
        bodies.push((TAG_MODS, pack(&big).unwrap()));
        assert!(matches!(
            decode(&assemble(&p.header, &bodies)),
            Err(SaveError::Section(_))
        ));
        // A MODS body that does not parse is an error, never silently "no mods".
        let last = bodies.len() - 1;
        bodies[last].1 = vec![0xFF, 0xFF, 0xFF];
        assert!(matches!(
            decode(&assemble(&p.header, &bodies)),
            Err(SaveError::Section(_))
        ));
    }

    #[test]
    fn file_names_round_trip_and_odd_names_are_not_saves() {
        assert_eq!(file_name(7, 12), "00000007-t0012.c5sv");
        assert_eq!(parse_file_name("00000007-t0012.c5sv"), Some((7, 12)));
        assert_eq!(parse_file_name("00000007-t12345.c5sv"), Some((7, 12345)));
        for bad in [
            "7-t0012.c5sv",
            "00000007-t12.c5sv",
            "00000007-t0012.c5sv.tmp",
            "x.c5sv",
        ] {
            assert_eq!(parse_file_name(bad), None, "{bad}");
        }
        assert_eq!(rel_path(SaveKind::Keep, 3, 10), "keep/00000003-t0010.c5sv");
    }

    #[test]
    fn keep_goes_with_every_tenth_turn_and_the_ring_loses_the_oldest() {
        assert_eq!(start_kind(10, 10), SaveKind::Keep);
        assert_eq!(start_kind(11, 10), SaveKind::Ring);
        assert_eq!(start_kind(10, 0), SaveKind::Ring);
        assert_eq!(ring_excess(&[5, 1, 3, 2, 4], 3), vec![1, 2]);
        assert_eq!(ring_excess(&[1, 2], 10), Vec::<u64>::new());
    }
}
