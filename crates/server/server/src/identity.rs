//! Accounts (spec accounts-login-lobby 1, 3, 4): nick, argon2id password hash, devices, live connections.
//!
//! `accounts.json` (format 2) keeps one [`Account`] per lowercase nick: the display nick, the PHC hash of the password
//! (`None` for a legacy record migrated from `identities.json`, which nobody has claimed yet) and up to 16 devices. A
//! device is one browser that logged in; the server keeps only the sha256 of its token. A device not seen for 90 days
//! is dropped, unless the nick is reserved by a seat or a room (set by the lobby through [`Identity::reserve`]).
//!
//! SIMPLIFIED(SIMP-exe-840): the original game has no accounts (LAN/Steam identities); ours are a nick, a password of
//! 8 to 128 characters and a list of device tokens. No e-mail, no password change screen, no recovery except the admin
//! reset.
//!
//! Live connections are memory only: account key -> [`LiveConn`]s. [`on_login`] is the one place that says which of
//! them a new login kicks (now: all of them; multi-device later: only the same device).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use argon2::password_hash::{PasswordHash, SaltString};
use argon2::{Algorithm, Argon2, Params, PasswordHasher, PasswordVerifier, Version};
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

use crate::clock::WallClock;
use crate::state::sha256::sha256_hex;
use crate::state::{FORMAT, files};
use protocol::{RejectReason, Token};

pub const MAX_NICK_CHARS: usize = 24;

/// Password length in characters.
pub const MIN_PASSWORD_CHARS: usize = 8;
pub const MAX_PASSWORD_CHARS: usize = 128;

/// A device not seen for this long dies (GS-D2), unless its nick is reserved.
pub const TOKEN_TTL_MS: u64 = 90 * 24 * 60 * 60 * 1000;

/// Devices of one account; the one with the oldest `last_seen_ms` makes room for a new one.
pub const MAX_DEVICES: usize = 16;

/// `format` of `accounts.json`.
pub const ACCOUNTS_FORMAT: u32 = 2;

/// Argon2id: 19 MiB, 2 passes, 1 lane (the OWASP minimum profile).
const ARGON_M_KIB: u32 = 19_456;
const ARGON_T: u32 = 2;
const ARGON_P: u32 = 1;

/// Unicode Cf (invisible, spoofable) chars; std has no category API.
fn is_format_char(c: char) -> bool {
    matches!(c,
        '\u{00AD}'
        | '\u{061C}'
        | '\u{180E}'
        | '\u{200B}'..='\u{200F}'
        | '\u{202A}'..='\u{202E}'
        | '\u{2060}'..='\u{2064}'
        | '\u{2066}'..='\u{206F}'
        | '\u{FEFF}'
        | '\u{FFF9}'..='\u{FFFB}')
}

/// `None` if empty, over 24 chars, or with Cc/Cf chars.
pub fn normalize_nick(raw: &str) -> Option<String> {
    let nick = raw.trim();
    let len = nick.chars().count();
    if len == 0 || len > MAX_NICK_CHARS || nick.chars().any(|c| c.is_control() || is_format_char(c))
    {
        return None;
    }
    Some(nick.to_owned())
}

/// The account key of a nick.
pub fn nick_key(nick: &str) -> String {
    nick.to_lowercase()
}

/// Constant-time equality (the length is not hidden, the content is).
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    let mut diff = a.len() ^ b.len();
    for i in 0..a.len().max(b.len()) {
        diff |= usize::from(a.get(i).copied().unwrap_or(0) ^ b.get(i).copied().unwrap_or(0));
    }
    diff == 0
}

/// A password or token from configuration. `Debug` hides it; comparison is constant-time.
#[derive(Clone)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// True if `candidate` equals the secret.
    pub fn matches(&self, candidate: &str) -> bool {
        ct_eq(self.0.as_bytes(), candidate.as_bytes())
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(***)")
    }
}

// ------------------------------------------------------------------ passwords

fn argon() -> Argon2<'static> {
    let params = Params::new(ARGON_M_KIB, ARGON_T, ARGON_P, None).expect("fixed argon2 parameters");
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
}

/// The PHC string of `password` with a fresh 16-byte salt. CPU heavy: call it from `spawn_blocking`.
pub fn hash_password(password: &str) -> Result<String, String> {
    let mut salt = [0u8; 16];
    getrandom::fill(&mut salt).map_err(|e| e.to_string())?;
    let salt = SaltString::encode_b64(&salt).map_err(|e| e.to_string())?;
    argon()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| e.to_string())
}

/// Whether `password` is the one `hash` (a PHC string) was made from. CPU heavy.
pub fn verify_password(hash: &str, password: &str) -> bool {
    PasswordHash::new(hash).is_ok_and(|p| argon().verify_password(password.as_bytes(), &p).is_ok())
}

/// A hash nobody knows the password of: verifying against it costs the same as a real account (an unknown nick must not
/// answer faster).
pub fn dummy_hash() -> &'static str {
    static DUMMY: OnceLock<String> = OnceLock::new();
    DUMMY.get_or_init(|| {
        hash_password("no account has this password").unwrap_or_else(|_| String::new())
    })
}

/// Rules for a new password: 8..=128 characters, nothing else.
pub fn password_ok(password: &str) -> bool {
    let n = password.chars().count();
    (MIN_PASSWORD_CHARS..=MAX_PASSWORD_CHARS).contains(&n)
}

// ------------------------------------------------------------------ data

#[derive(Clone)]
struct Device {
    id: u64,
    token_sha256: [u8; 32],
    created_ms: u64,
    last_seen_ms: u64,
    label: String,
}

struct Account {
    nick: String,
    pw_hash: Option<String>,
    created_ms: u64,
    pw_changed_ms: u64,
    devices: Vec<Device>,
}

/// One live connection of an account (memory only).
pub struct LiveConn {
    pub session: u64,
    pub device: u64,
    pub kick: oneshot::Sender<()>,
}

/// Which live connections a new login of `new_device` kicks. NOW: all of them (replace on login).
// SIMPLIFIED(SIMP-exe-842): one live connection per account; the original MP lets one player take over a slot from
// another machine only by a rejoin. Several devices at once need the seat fan-out of spec 8.
/// Multi-device later (step 1): only those of `new_device`; nothing else in the server changes.
pub fn on_login(existing: &[LiveConn], new_device: u64) -> Vec<u64> {
    let _ = new_device;
    existing.iter().map(|c| c.session).collect()
}

#[derive(Default)]
struct Inner {
    accounts: HashMap<String, Account>,
    live: HashMap<String, Vec<LiveConn>>,
    reserved: HashSet<String>,
}

#[derive(Serialize, Deserialize)]
struct FileDevice {
    id: u64,
    token_sha256: String,
    created_ms: u64,
    last_seen_ms: u64,
    label: String,
}

#[derive(Serialize, Deserialize)]
struct FileAccount {
    nick: String,
    pw_hash: Option<String>,
    created_ms: u64,
    pw_changed_ms: u64,
    devices: Vec<FileDevice>,
}

#[derive(Serialize, Deserialize, Default)]
struct FileData {
    accounts: Vec<FileAccount>,
}

/// `identities.json` of format 1 (read once, for the migration).
#[derive(Deserialize)]
struct LegacyRecord {
    nick: String,
    token: String,
    last_seen_ms: u64,
}

#[derive(Deserialize, Default)]
struct LegacyData {
    records: Vec<LegacyRecord>,
}

fn sha_bytes(data: &[u8]) -> [u8; 32] {
    let hex = sha256_hex(data);
    let mut out = [0u8; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(hex.get(i * 2..i * 2 + 2).unwrap_or("00"), 16).unwrap_or(0);
    }
    out
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex32(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64 || !s.is_ascii() {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

fn random_u64() -> u64 {
    let mut b = [0u8; 8];
    // A failed draw gives a fixed id; ids only tell devices of one account apart.
    let _ = getrandom::fill(&mut b);
    u64::from_le_bytes(b)
}

/// "Chrome / Android" from a `User-Agent`, at most 40 characters (for a later device list).
pub fn device_label(user_agent: &str) -> String {
    let ua = user_agent;
    let browser = if ua.contains("Edg/") || ua.contains("EdgA/") {
        "Edge"
    } else if ua.contains("OPR/") {
        "Opera"
    } else if ua.contains("Firefox/") || ua.contains("FxiOS/") {
        "Firefox"
    } else if ua.contains("Chrome/") || ua.contains("CriOS/") {
        "Chrome"
    } else if ua.contains("Safari/") {
        "Safari"
    } else if ua.is_empty() {
        "unknown"
    } else {
        "other"
    };
    let os = if ua.contains("Android") {
        "Android"
    } else if ua.contains("iPhone") || ua.contains("iPad") || ua.contains("iPod") {
        "iOS"
    } else if ua.contains("Windows") {
        "Windows"
    } else if ua.contains("Mac OS X") || ua.contains("Macintosh") {
        "macOS"
    } else if ua.contains("CrOS") {
        "ChromeOS"
    } else if ua.contains("Linux") {
        "Linux"
    } else {
        "unknown"
    };
    format!("{browser} / {os}").chars().take(40).collect()
}

/// What a successful WebSocket login yields.
#[derive(Debug)]
pub struct Grant {
    /// The nick as registered (display case).
    pub nick: String,
    /// The token of the device that logged in: the stored one for a token login, a fresh one after a password login.
    pub token: Token,
    pub device: u64,
    /// Sessions kicked by this login (`on_login`).
    pub replaced: Vec<u64>,
}

/// What a registration or an HTTP login yields: no live session yet.
#[derive(Debug)]
pub struct Registered {
    pub nick: String,
    pub token: Token,
    pub device: u64,
}

/// What a login proves.
pub enum Proof<'a> {
    /// A device token.
    Token(&'a Token),
    /// The password was verified against `hash` (outside the lock); the account must still have that hash.
    Password { hash: &'a str },
}

/// The password side of a nick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PwState {
    /// No account.
    Unknown,
    /// A legacy record: logs in by token only until somebody registers the nick.
    Legacy,
    /// The PHC hash.
    Hash(String),
}

/// Why a registration was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegisterError {
    /// The nick has an account with a password.
    Taken,
    /// The nick has a seat or a room and the request has no valid old token.
    SeatHeld,
    /// The nick has a live session and the request has no valid old token.
    InUse,
    Internal,
}

/// Account table with an optional file behind it.
pub struct Identity {
    inner: Mutex<Inner>,
    path: Option<PathBuf>,
    wall: Arc<dyn WallClock>,
}

impl Identity {
    /// In-memory table (tests, servers without a `state_dir`).
    pub fn in_memory(wall: Arc<dyn WallClock>) -> Self {
        Self {
            inner: Mutex::new(Inner::default()),
            path: None,
            wall,
        }
    }

    /// Loads `accounts.json` (`path`). Without it, a sibling `identities.json` (format 1) is converted into legacy
    /// accounts and renamed `identities.json.migrated`. A file from a newer build is an error.
    pub fn open(path: PathBuf, wall: Arc<dyn WallClock>) -> anyhow::Result<Self> {
        let mut inner = Inner::default();
        let mut migrated = false;
        if let Some(data) = files::read_versioned::<FileData>(&path, ACCOUNTS_FORMAT)? {
            for a in data.accounts {
                let devices = a
                    .devices
                    .into_iter()
                    .filter_map(|d| {
                        let Some(sha) = unhex32(&d.token_sha256) else {
                            tracing::warn!(nick = %a.nick, "accounts.json: bad token hash, device dropped");
                            return None;
                        };
                        Some(Device {
                            id: d.id,
                            token_sha256: sha,
                            created_ms: d.created_ms,
                            last_seen_ms: d.last_seen_ms,
                            label: d.label,
                        })
                    })
                    .collect();
                inner.accounts.insert(
                    nick_key(&a.nick),
                    Account {
                        nick: a.nick,
                        pw_hash: a.pw_hash,
                        created_ms: a.created_ms,
                        pw_changed_ms: a.pw_changed_ms,
                        devices,
                    },
                );
            }
        } else if let Some(legacy) = legacy_path(&path)
            && let Some(data) = files::read_versioned::<LegacyData>(&legacy, FORMAT)?
        {
            for r in data.records {
                let Some(token) = token_from_hex(&r.token) else {
                    tracing::warn!(nick = %r.nick, "identities.json: bad token, record dropped");
                    continue;
                };
                inner.accounts.insert(
                    nick_key(&r.nick),
                    Account {
                        nick: r.nick,
                        pw_hash: None,
                        created_ms: r.last_seen_ms,
                        pw_changed_ms: 0,
                        devices: vec![Device {
                            id: random_u64(),
                            token_sha256: sha_bytes(&token.0),
                            created_ms: r.last_seen_ms,
                            last_seen_ms: r.last_seen_ms,
                            label: "legacy".into(),
                        }],
                    },
                );
            }
            migrated = true;
        }
        let me = Self {
            inner: Mutex::new(inner),
            path: Some(path.clone()),
            wall,
        };
        if migrated {
            me.persist(&me.lock());
            if let Some(legacy) = legacy_path(&path) {
                let mut moved = legacy.as_os_str().to_owned();
                moved.push(".migrated");
                match std::fs::rename(&legacy, &moved) {
                    Ok(()) => tracing::info!(
                        accounts = me.len(),
                        "identities.json converted into accounts.json (old file kept as identities.json.migrated)"
                    ),
                    Err(e) => tracing::error!(error = %e, "cannot rename identities.json"),
                }
            }
        }
        Ok(me)
    }

    /// A WebSocket login of `nick` for `session`. On success the live connections that [`on_login`] names are told to go
    /// through their `kick`, and `kick` is taken (left in place on failure, so the caller can try another proof).
    pub fn login(
        &self,
        nick: &str,
        proof: Proof<'_>,
        label: &str,
        session: u64,
        kick: &mut Option<oneshot::Sender<()>>,
    ) -> Result<Grant, RejectReason> {
        let key = nick_key(nick);
        let now = self.wall.unix_ms();
        // A fresh token is drawn before the lock (it is only used by a password login).
        let fresh = match proof {
            Proof::Password { .. } => Some(random_token().map_err(|e| {
                tracing::error!(error = %e, "cannot draw a token");
                // No better reason in the protocol; the client retries the login.
                RejectReason::Full
            })?),
            Proof::Token(_) => None,
        };
        let mut inner = self.lock();
        let reserved = inner.reserved.contains(&key);
        let acct = inner
            .accounts
            .get_mut(&key)
            .ok_or(RejectReason::BadPassword)?;
        let (device, token) = match (proof, fresh) {
            (Proof::Token(t), _) => {
                let sha = sha_bytes(&t.0);
                let d = acct
                    .devices
                    .iter_mut()
                    .find(|d| {
                        ct_eq(&d.token_sha256, &sha)
                            && (reserved || now.saturating_sub(d.last_seen_ms) <= TOKEN_TTL_MS)
                    })
                    .ok_or(RejectReason::BadPassword)?;
                d.last_seen_ms = now;
                (d.id, *t)
            }
            (Proof::Password { hash }, Some(fresh)) => {
                if !acct
                    .pw_hash
                    .as_deref()
                    .is_some_and(|h| ct_eq(h.as_bytes(), hash.as_bytes()))
                {
                    return Err(RejectReason::BadPassword);
                }
                let id = add_device(acct, &fresh, label, now);
                (id, fresh)
            }
            (Proof::Password { .. }, None) => return Err(RejectReason::Full),
        };
        let display = acct.nick.clone();
        let sender = kick.take().ok_or(RejectReason::Full)?;
        let live = inner.live.entry(key).or_default();
        let to_kick = on_login(live, device);
        let mut replaced = Vec::new();
        let mut keep = Vec::with_capacity(live.len() + 1);
        for c in live.drain(..) {
            if to_kick.contains(&c.session) {
                // The receiver may be gone already.
                let _ = c.kick.send(());
                replaced.push(c.session);
            } else {
                keep.push(c);
            }
        }
        keep.push(LiveConn {
            session,
            device,
            kick: sender,
        });
        *live = keep;
        self.persist(&inner);
        Ok(Grant {
            nick: display,
            token,
            device,
            replaced,
        })
    }

    /// Creates the account `nick` with `hash`. A legacy account (no hash) is claimed when `legacy` is a valid
    /// old token of it, or when the nick is free (no devices, or no seat/room and no live session). Returns the new
    /// device's token.
    pub fn register(
        &self,
        nick: &str,
        hash: String,
        legacy: Option<&Token>,
        label: &str,
    ) -> Result<Registered, RegisterError> {
        let key = nick_key(nick);
        let now = self.wall.unix_ms();
        let fresh = random_token().map_err(|_| RegisterError::Internal)?;
        let mut inner = self.lock();
        purge(&mut inner, now);
        let reserved = inner.reserved.contains(&key);
        let live = inner.live.get(&key).is_some_and(|l| !l.is_empty());
        let registered = match inner.accounts.get_mut(&key) {
            None => {
                if reserved {
                    return Err(RegisterError::SeatHeld);
                }
                let mut acct = Account {
                    nick: nick.to_owned(),
                    pw_hash: Some(hash),
                    created_ms: now,
                    pw_changed_ms: now,
                    devices: Vec::new(),
                };
                let id = add_device(&mut acct, &fresh, label, now);
                inner.accounts.insert(key, acct);
                (nick.to_owned(), id)
            }
            Some(acct) if acct.pw_hash.is_some() => return Err(RegisterError::Taken),
            Some(acct) => {
                let owner = legacy.and_then(|t| {
                    let sha = sha_bytes(&t.0);
                    acct.devices
                        .iter()
                        .position(|d| ct_eq(&d.token_sha256, &sha))
                });
                // SIMPLIFIED(SIMP-exe-845): a legacy nick without a seat, a room or a live session is free for anyone
                // to claim, as it was free for anyone to log in with before accounts (open access).
                // Nobody holds the record (reset by the admin), or nothing ties the nick to a game.
                let free = acct.devices.is_empty() || (!reserved && !live);
                match owner {
                    Some(i) => {
                        // The old browser's token is replaced by the new device's.
                        acct.devices.remove(i);
                    }
                    None if free => acct.devices.clear(),
                    None if reserved => return Err(RegisterError::SeatHeld),
                    None => return Err(RegisterError::InUse),
                }
                acct.pw_hash = Some(hash);
                acct.pw_changed_ms = now;
                let id = add_device(acct, &fresh, label, now);
                (acct.nick.clone(), id)
            }
        };
        self.persist(&inner);
        Ok(Registered {
            nick: registered.0,
            token: fresh,
            device: registered.1,
        })
    }

    /// HTTP login: a new device for the account whose hash is `hash` (verified outside the lock).
    pub fn login_http(
        &self,
        nick: &str,
        hash: &str,
        label: &str,
    ) -> Result<Registered, RejectReason> {
        let key = nick_key(nick);
        let now = self.wall.unix_ms();
        let fresh = random_token().map_err(|_| RejectReason::Full)?;
        let mut inner = self.lock();
        let acct = inner
            .accounts
            .get_mut(&key)
            .ok_or(RejectReason::BadPassword)?;
        if !acct
            .pw_hash
            .as_deref()
            .is_some_and(|h| ct_eq(h.as_bytes(), hash.as_bytes()))
        {
            return Err(RejectReason::BadPassword);
        }
        let device = add_device(acct, &fresh, label, now);
        let nick = acct.nick.clone();
        self.persist(&inner);
        Ok(Registered {
            nick,
            token: fresh,
            device,
        })
    }

    /// Drops the device with `token` (logout) and kicks its live connections. `false` if there was none.
    pub fn logout(&self, nick: &str, token: &Token) -> bool {
        let key = nick_key(nick);
        let sha = sha_bytes(&token.0);
        let mut inner = self.lock();
        let Some(acct) = inner.accounts.get_mut(&key) else {
            return false;
        };
        let Some(i) = acct
            .devices
            .iter()
            .position(|d| ct_eq(&d.token_sha256, &sha))
        else {
            return false;
        };
        let device = acct.devices.remove(i).id;
        if let Some(live) = inner.live.get_mut(&key) {
            let mut keep = Vec::new();
            for c in live.drain(..) {
                if c.device == device {
                    let _ = c.kick.send(());
                } else {
                    keep.push(c);
                }
            }
            *live = keep;
        }
        self.persist(&inner);
        true
    }

    /// Admin reset: clears the hash and the devices, so the owner can register the nick again. Live
    /// connections are kicked. `false` if there is no such account.
    pub fn reset(&self, nick: &str) -> bool {
        let key = nick_key(nick);
        let mut inner = self.lock();
        let Some(acct) = inner.accounts.get_mut(&key) else {
            return false;
        };
        acct.pw_hash = None;
        acct.devices.clear();
        for c in inner.live.remove(&key).unwrap_or_default() {
            let _ = c.kick.send(());
        }
        self.persist(&inner);
        true
    }

    /// The password side of `nick`.
    pub fn password_state(&self, nick: &str) -> PwState {
        match self.lock().accounts.get(&nick_key(nick)) {
            None => PwState::Unknown,
            Some(a) => match &a.pw_hash {
                None => PwState::Legacy,
                Some(h) => PwState::Hash(h.clone()),
            },
        }
    }

    /// Whether `token` is a valid device token of `nick` (the same test as a token login, with no login: nothing is
    /// changed). For the report endpoint (`http/reports.rs`).
    pub fn verify(&self, nick: &str, token: &Token) -> bool {
        let now = self.wall.unix_ms();
        let inner = self.lock();
        let key = nick_key(nick);
        let reserved = inner.reserved.contains(&key);
        let sha = sha_bytes(&token.0);
        inner.accounts.get(&key).is_some_and(|a| {
            a.devices.iter().any(|d| {
                ct_eq(&d.token_sha256, &sha)
                    && (reserved || now.saturating_sub(d.last_seen_ms) <= TOKEN_TTL_MS)
            })
        })
    }

    /// The nick as registered (display case), if `token` is a valid device token of `nick` (the test of [`Self::verify`]).
    /// For `POST /api/auth/adopt`, which turns an old `localStorage` token into the session cookie.
    pub fn adopt(&self, nick: &str, token: &Token) -> Option<String> {
        let key = nick_key(nick);
        let now = self.wall.unix_ms();
        let inner = self.lock();
        let reserved = inner.reserved.contains(&key);
        let sha = sha_bytes(&token.0);
        let acct = inner.accounts.get(&key)?;
        acct.devices
            .iter()
            .any(|d| {
                ct_eq(&d.token_sha256, &sha)
                    && (reserved || now.saturating_sub(d.last_seen_ms) <= TOKEN_TTL_MS)
            })
            .then(|| acct.nick.clone())
    }

    /// Ends `session`'s hold on `nick`; no-op if it is gone already (replaced). Stamps the device's `last_seen`.
    pub fn unregister(&self, nick: &str, session: u64) {
        let key = nick_key(nick);
        let now = self.wall.unix_ms();
        let mut inner = self.lock();
        let Some(live) = inner.live.get_mut(&key) else {
            return;
        };
        let Some(i) = live.iter().position(|c| c.session == session) else {
            return;
        };
        let gone = live.remove(i);
        if live.is_empty() {
            inner.live.remove(&key);
        }
        if let Some(d) = inner
            .accounts
            .get_mut(&key)
            .and_then(|a| a.devices.iter_mut().find(|d| d.id == gone.device))
        {
            d.last_seen_ms = now;
        }
        self.persist(&inner);
    }

    /// Marks `nick` as held by a seat or a room (GS-D3).
    pub fn reserve(&self, nick: &str) {
        self.lock().reserved.insert(nick_key(nick));
    }

    /// Releases the hold.
    pub fn release(&self, nick: &str) {
        self.lock().reserved.remove(&nick_key(nick));
    }

    /// Whether `nick` is held by a seat or a room.
    pub fn is_reserved(&self, nick: &str) -> bool {
        self.lock().reserved.contains(&nick_key(nick))
    }

    /// Number of live connections.
    pub fn live_count(&self) -> usize {
        self.lock().live.values().map(Vec::len).sum()
    }

    /// Number of devices of `nick`.
    pub fn device_count(&self, nick: &str) -> usize {
        self.lock()
            .accounts
            .get(&nick_key(nick))
            .map_or(0, |a| a.devices.len())
    }

    /// Number of accounts.
    pub fn len(&self) -> usize {
        self.lock().accounts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        // Short sections that never panic: ignore poisoning.
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Writes the file. A failed write is logged: the session goes on, only the account's survival of a restart is lost.
    fn persist(&self, inner: &Inner) {
        let Some(path) = &self.path else { return };
        let mut accounts: Vec<FileAccount> = inner
            .accounts
            .values()
            .map(|a| FileAccount {
                nick: a.nick.clone(),
                pw_hash: a.pw_hash.clone(),
                created_ms: a.created_ms,
                pw_changed_ms: a.pw_changed_ms,
                devices: a
                    .devices
                    .iter()
                    .map(|d| FileDevice {
                        id: d.id,
                        token_sha256: hex(&d.token_sha256),
                        created_ms: d.created_ms,
                        last_seen_ms: d.last_seen_ms,
                        label: d.label.clone(),
                    })
                    .collect(),
            })
            .collect();
        accounts.sort_by(|a, b| a.nick.cmp(&b.nick));
        if let Err(e) = files::write_versioned(
            path,
            ACCOUNTS_FORMAT,
            &FileData { accounts },
            files::FileMode::Private,
        ) {
            tracing::error!(error = %format!("{e:#}"), "cannot write accounts.json");
        }
    }
}

/// `identities.json` next to `accounts.json`.
fn legacy_path(accounts: &Path) -> Option<PathBuf> {
    Some(accounts.parent()?.join("identities.json"))
}

/// Adds a device for `token`; the one seen longest ago goes when there are 16.
fn add_device(acct: &mut Account, token: &Token, label: &str, now: u64) -> u64 {
    while acct.devices.len() >= MAX_DEVICES {
        let Some(oldest) = acct
            .devices
            .iter()
            .enumerate()
            .min_by_key(|(_, d)| d.last_seen_ms)
            .map(|(i, _)| i)
        else {
            break;
        };
        acct.devices.remove(oldest);
    }
    let id = random_u64();
    acct.devices.push(Device {
        id,
        token_sha256: sha_bytes(&token.0),
        created_ms: now,
        last_seen_ms: now,
        label: label.chars().take(40).collect(),
    });
    id
}

/// Drops devices not seen for 90 days and legacy records nobody holds, except for reserved nicks.
fn purge(inner: &mut Inner, now: u64) {
    let reserved = &inner.reserved;
    let live = &inner.live;
    inner.accounts.retain(|key, a| {
        if reserved.contains(key) || live.contains_key(key) {
            return true;
        }
        a.devices
            .retain(|d| now.saturating_sub(d.last_seen_ms) <= TOKEN_TTL_MS);
        a.pw_hash.is_some() || !a.devices.is_empty()
    });
}

/// Lowercase hex of a token.
pub fn token_to_hex(t: &Token) -> String {
    t.0.iter().map(|b| format!("{b:02x}")).collect()
}

/// Inverse of [`token_to_hex`]; `None` unless exactly 32 hex digits.
pub fn token_from_hex(s: &str) -> Option<Token> {
    if s.len() != 32 || !s.is_ascii() {
        return None;
    }
    let mut out = [0u8; 16];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(Token(out))
}

/// First 4 hex digits: the only part of a token that may reach a log.
pub fn token_short(t: &Token) -> String {
    format!("{:02x}{:02x}", t.0[0], t.0[1])
}

fn random_token() -> Result<Token, getrandom::Error> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes)?;
    Ok(Token(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::TestWallClock;

    fn wall() -> Arc<TestWallClock> {
        Arc::new(TestWallClock::new(1_000_000))
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("civ5-ident-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A cheap stand-in for a PHC string: the table only compares hashes.
    const H: &str = "$argon2id$test-hash-one";
    const H2: &str = "$argon2id$test-hash-two";

    fn reg(id: &Identity, nick: &str) -> Registered {
        id.register(nick, H.into(), None, "Chrome / Android")
            .unwrap()
    }

    fn login_token(
        id: &Identity,
        nick: &str,
        t: &Token,
        session: u64,
    ) -> (Result<Grant, RejectReason>, oneshot::Receiver<()>) {
        let (tx, rx) = oneshot::channel();
        let mut k = Some(tx);
        (id.login(nick, Proof::Token(t), "x", session, &mut k), rx)
    }

    fn login_pw(
        id: &Identity,
        nick: &str,
        hash: &str,
        session: u64,
    ) -> (Result<Grant, RejectReason>, oneshot::Receiver<()>) {
        let (tx, rx) = oneshot::channel();
        let mut k = Some(tx);
        (
            id.login(nick, Proof::Password { hash }, "x", session, &mut k),
            rx,
        )
    }

    #[test]
    fn nick_rules() {
        assert_eq!(normalize_nick("  alice "), Some("alice".into()));
        assert_eq!(normalize_nick(""), None);
        assert_eq!(normalize_nick("   "), None);
        assert_eq!(normalize_nick("a\nb"), None);
        assert_eq!(normalize_nick("a\u{7f}"), None);
        for bad in [
            "bob\u{200B}",
            "a\u{200D}b",
            "\u{202E}evil",
            "x\u{2060}",
            "a\u{FEFF}",
            "so\u{00AD}ft",
            "a\u{200E}",
        ] {
            assert_eq!(normalize_nick(bad), None, "{bad:?}");
        }
        assert!(normalize_nick(&"я".repeat(24)).is_some());
        assert_eq!(normalize_nick(&"я".repeat(25)), None);
    }

    #[test]
    fn token_hex_roundtrip() {
        let t = Token([0xab; 16]);
        assert_eq!(token_to_hex(&t), "ab".repeat(16));
        assert_eq!(token_from_hex(&token_to_hex(&t)), Some(t));
        assert_eq!(token_from_hex("zz"), None);
        assert_eq!(token_from_hex(&"é".repeat(16)), None);
        assert_eq!(token_short(&t), "abab");
    }

    #[test]
    fn constant_time_compare() {
        assert!(ct_eq(b"secret-pass", b"secret-pass"));
        assert!(!ct_eq(b"secret-pass", b"secret-pasS"));
        assert!(!ct_eq(b"secret", b"secret-pass"));
        assert!(!ct_eq(b"", b"x"));
        assert!(ct_eq(b"", b""));
    }

    #[test]
    fn argon2id_hash_and_verify() {
        let h = hash_password("correct horse").unwrap();
        assert!(h.starts_with("$argon2id$v=19$m=19456,t=2,p=1$"), "{h}");
        assert!(verify_password(&h, "correct horse"));
        assert!(!verify_password(&h, "correct horsE"));
        assert!(!verify_password("not a phc string", "correct horse"));
        // A fresh salt every time.
        assert_ne!(h, hash_password("correct horse").unwrap());
        // The dummy hash is a real hash nobody can open.
        assert!(dummy_hash().starts_with("$argon2id$"));
        assert!(!verify_password(dummy_hash(), ""));
    }

    #[test]
    fn password_rules() {
        assert!(!password_ok("short"));
        assert!(password_ok("12345678"));
        assert!(password_ok(&"я".repeat(128)));
        assert!(!password_ok(&"я".repeat(129)));
        assert!(!password_ok("пароль"));
    }

    #[test]
    fn device_labels_from_user_agents() {
        let android = "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Mobile Safari/537.36";
        assert_eq!(device_label(android), "Chrome / Android");
        let ff = "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:128.0) Gecko/20100101 Firefox/128.0";
        assert_eq!(device_label(ff), "Firefox / Windows");
        let safari = "Mozilla/5.0 (iPhone; CPU iPhone OS 17_5 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Mobile/15E148 Safari/604.1";
        assert_eq!(device_label(safari), "Safari / iOS");
        assert_eq!(device_label(""), "unknown / unknown");
        assert!(device_label(&"x".repeat(500)).chars().count() <= 40);
    }

    #[test]
    fn register_then_login_by_token_or_password() {
        let id = Identity::in_memory(wall());
        let r = reg(&id, "Alice");
        assert_eq!((id.len(), id.device_count("alice")), (1, 1));
        assert_eq!(id.password_state("ALICE"), PwState::Hash(H.into()));

        // Token login: the display nick is the registered one.
        let (g, _rx) = login_token(&id, "alice", &r.token, 1);
        let g = g.unwrap();
        assert_eq!(
            (g.nick.as_str(), g.token, g.device),
            ("Alice", r.token, r.device)
        );
        assert!(g.replaced.is_empty());
        id.unregister("alice", 1);

        // Password login makes a second device with its own token.
        let (g2, _rx) = login_pw(&id, "alice", H, 2);
        let g2 = g2.unwrap();
        assert_ne!(g2.token, r.token);
        assert_eq!(id.device_count("alice"), 2);
        // A wrong hash and an unknown nick are the same refusal.
        assert_eq!(
            login_pw(&id, "alice", H2, 3).0.unwrap_err(),
            RejectReason::BadPassword
        );
        assert_eq!(
            login_pw(&id, "nobody", H, 3).0.unwrap_err(),
            RejectReason::BadPassword
        );
        // A token of another account does not open this one.
        reg(&id, "bob");
        assert_eq!(
            login_token(&id, "bob", &g2.token, 4).0.unwrap_err(),
            RejectReason::BadPassword
        );
    }

    #[test]
    fn a_failed_login_keeps_the_kick_for_the_next_proof() {
        let id = Identity::in_memory(wall());
        let r = reg(&id, "alice");
        let (tx, mut rx) = oneshot::channel();
        let mut k = Some(tx);
        assert!(
            id.login("alice", Proof::Token(&Token([9; 16])), "x", 1, &mut k)
                .is_err()
        );
        assert!(k.is_some());
        id.login("alice", Proof::Token(&r.token), "x", 1, &mut k)
            .unwrap();
        assert!(k.is_none());
        assert!(rx.try_recv().is_err(), "the session itself is not kicked");
    }

    #[test]
    fn the_token_is_stored_only_as_a_hash() {
        let dir = scratch("hashonly");
        let path = dir.join("accounts.json");
        let id = Identity::open(path.clone(), wall()).unwrap();
        let r = reg(&id, "alice");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"format\": 2"), "{text}");
        assert!(!text.contains(&token_to_hex(&r.token)), "{text}");
        assert!(text.contains(&sha256_hex(&r.token.0)), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn on_login_kicks_everyone_for_now() {
        let mk = |session, device| LiveConn {
            session,
            device,
            kick: oneshot::channel().0,
        };
        let live = [mk(1, 10), mk(2, 20), mk(3, 10)];
        assert_eq!(on_login(&live, 10), vec![1, 2, 3]);
        assert_eq!(on_login(&live, 99), vec![1, 2, 3]);
        assert!(on_login(&[], 1).is_empty());
    }

    #[test]
    fn a_second_login_replaces_the_first_and_the_old_session_cannot_drop_the_new() {
        let id = Identity::in_memory(wall());
        let r = reg(&id, "bob");
        let (g1, mut rx1) = login_token(&id, "bob", &r.token, 1);
        g1.unwrap();
        let (g2, mut rx2) = login_pw(&id, "bob", H, 2);
        let g2 = g2.unwrap();
        assert_eq!(g2.replaced, vec![1]);
        assert!(rx1.try_recv().is_ok());
        assert!(rx2.try_recv().is_err());
        // A token login replaces the password session too.
        let (g3, mut rx3) = login_token(&id, "BOB", &g2.token, 3);
        assert_eq!(g3.unwrap().replaced, vec![2]);
        assert!(rx2.try_recv().is_ok());
        // The old session ending must not drop the new one.
        id.unregister("bob", 1);
        id.unregister("bob", 2);
        assert_eq!(id.live_count(), 1);
        assert!(rx3.try_recv().is_err());
        id.unregister("bob", 3);
        assert_eq!(id.live_count(), 0);
    }

    #[test]
    fn registering_a_taken_nick_is_refused_in_any_case() {
        let id = Identity::in_memory(wall());
        reg(&id, "alice");
        assert_eq!(
            id.register("ALICE", H2.into(), None, "x").unwrap_err(),
            RegisterError::Taken
        );
        assert_eq!(id.len(), 1);
    }

    #[test]
    fn a_reserved_nick_cannot_be_registered_from_scratch() {
        let id = Identity::in_memory(wall());
        id.reserve("ghost");
        assert_eq!(
            id.register("Ghost", H.into(), None, "x").unwrap_err(),
            RegisterError::SeatHeld
        );
        id.release("ghost");
        id.register("Ghost", H.into(), None, "x").unwrap();
    }

    /// A legacy account as migrated from `identities.json`.
    fn legacy_table(dir: &Path, clock: Arc<TestWallClock>, token: &Token) -> Identity {
        std::fs::write(
            dir.join("identities.json"),
            format!(
                r#"{{"format": 1, "records": [{{"nick": "Old", "token": "{}", "last_seen_ms": 5}}]}}"#,
                token_to_hex(token)
            ),
        )
        .unwrap();
        Identity::open(dir.join("accounts.json"), clock).unwrap()
    }

    #[test]
    fn identities_json_is_migrated_into_legacy_accounts() {
        let dir = scratch("migrate");
        let token = Token([7; 16]);
        let id = legacy_table(&dir, wall(), &token);
        assert_eq!(id.password_state("old"), PwState::Legacy);
        assert_eq!(id.device_count("old"), 1);
        // The old token still logs in.
        let (g, _rx) = login_token(&id, "OLD", &token, 1);
        assert_eq!(g.unwrap().nick, "Old");
        // The old file is kept under another name; the new one exists and does not hold the token.
        assert!(!dir.join("identities.json").exists());
        assert!(dir.join("identities.json.migrated").exists());
        let text = std::fs::read_to_string(dir.join("accounts.json")).unwrap();
        assert!(!text.contains(&token_to_hex(&token)) && text.contains("\"pw_hash\": null"));
        // A restart reads accounts.json and does not migrate again.
        drop(id);
        let id = Identity::open(dir.join("accounts.json"), wall()).unwrap();
        assert_eq!(id.len(), 1);
        assert!(login_token(&id, "old", &token, 1).0.is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_legacy_nick_is_claimed_with_its_old_token() {
        let dir = scratch("claim-token");
        let token = Token([7; 16]);
        let id = legacy_table(&dir, wall(), &token);
        // Held by a game, and a live stranger-free session: only the owner's token claims it.
        id.reserve("old");
        assert_eq!(
            id.register("old", H.into(), None, "x").unwrap_err(),
            RegisterError::SeatHeld
        );
        assert_eq!(
            id.register("old", H.into(), Some(&Token([8; 16])), "x")
                .unwrap_err(),
            RegisterError::SeatHeld
        );
        let r = id
            .register("old", H.into(), Some(&token), "Chrome / Android")
            .unwrap();
        assert_eq!(r.nick, "Old");
        assert_eq!(id.password_state("old"), PwState::Hash(H.into()));
        // The old token's device was replaced by the new one; the seat is untouched.
        assert!(login_token(&id, "old", &token, 1).0.is_err());
        assert!(login_token(&id, "old", &r.token, 1).0.is_ok());
        assert!(id.is_reserved("old"));
        assert_eq!(id.device_count("old"), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_free_legacy_nick_is_claimed_by_anyone_and_the_old_token_dies() {
        let dir = scratch("claim-free");
        let token = Token([7; 16]);
        let id = legacy_table(&dir, wall(), &token);
        // A live session without a token holds it for now.
        let (g, _rx) = login_token(&id, "old", &token, 1);
        g.unwrap();
        assert_eq!(
            id.register("old", H.into(), None, "x").unwrap_err(),
            RegisterError::InUse
        );
        id.unregister("old", 1);
        let r = id.register("old", H.into(), None, "x").unwrap();
        assert!(
            login_token(&id, "old", &token, 2).0.is_err(),
            "the old owner's token died"
        );
        assert!(login_token(&id, "old", &r.token, 2).0.is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_admin_reset_frees_the_nick_even_with_a_seat_and_kicks_the_live_session() {
        let id = Identity::in_memory(wall());
        let r = reg(&id, "alice");
        id.reserve("alice");
        let (g, mut rx) = login_token(&id, "alice", &r.token, 1);
        g.unwrap();
        assert!(id.reset("Alice"));
        assert!(rx.try_recv().is_ok());
        assert_eq!(id.live_count(), 0);
        assert_eq!(id.password_state("alice"), PwState::Legacy);
        assert!(login_token(&id, "alice", &r.token, 2).0.is_err());
        // The owner registers again; the seat is still reserved for the nick.
        let r2 = id.register("alice", H2.into(), None, "x").unwrap();
        assert!(login_token(&id, "alice", &r2.token, 3).0.is_ok());
        assert!(!id.reset("nobody"));
    }

    #[test]
    fn logout_drops_one_device_and_kicks_its_connection() {
        let id = Identity::in_memory(wall());
        let r = reg(&id, "alice");
        let (g2, _rx2) = login_pw(&id, "alice", H, 1);
        let other = g2.unwrap();
        id.unregister("alice", 1);
        let (g, mut rx) = login_token(&id, "alice", &r.token, 2);
        g.unwrap();
        assert!(id.logout("alice", &r.token));
        assert!(rx.try_recv().is_ok());
        assert_eq!(id.device_count("alice"), 1);
        assert!(!id.logout("alice", &r.token));
        assert!(login_token(&id, "alice", &other.token, 3).0.is_ok());
    }

    #[test]
    fn at_most_16_devices_the_oldest_seen_goes() {
        let clock = wall();
        let id = Identity::in_memory(clock.clone());
        let first = reg(&id, "alice");
        let mut last = first.token;
        for _ in 0..MAX_DEVICES + 3 {
            clock.advance(1000);
            let (g, _rx) = login_pw(&id, "alice", H, 1);
            last = g.unwrap().token;
            id.unregister("alice", 1);
        }
        assert_eq!(id.device_count("alice"), MAX_DEVICES);
        assert!(login_token(&id, "alice", &first.token, 2).0.is_err());
        assert!(login_token(&id, "alice", &last, 2).0.is_ok());
    }

    #[test]
    fn old_devices_expire_unless_the_nick_is_reserved() {
        let clock = wall();
        let id = Identity::in_memory(clock.clone());
        let r = reg(&id, "bob");
        clock.advance(TOKEN_TTL_MS - 1);
        let (g, _rx) = login_token(&id, "bob", &r.token, 1);
        g.unwrap();
        id.unregister("bob", 1);

        clock.advance(TOKEN_TTL_MS + 1);
        assert_eq!(
            login_token(&id, "bob", &r.token, 2).0.unwrap_err(),
            RejectReason::BadPassword
        );
        assert!(!id.verify("bob", &r.token));
        id.reserve("bob");
        assert!(id.verify("bob", &r.token));
        assert!(login_token(&id, "bob", &r.token, 3).0.is_ok());
        // The password still works after the device died.
        id.release("bob");
        id.unregister("bob", 3);
        clock.advance(TOKEN_TTL_MS + 1);
        assert!(login_pw(&id, "bob", H, 4).0.is_ok());
    }

    #[test]
    fn accounts_survive_a_restart() {
        let dir = scratch("persist");
        let path = dir.join("accounts.json");
        let clock = wall();
        let token = {
            let id = Identity::open(path.clone(), clock.clone()).unwrap();
            let r = reg(&id, "alice");
            let (g, _rx) = login_token(&id, "alice", &r.token, 1);
            g.unwrap();
            id.unregister("alice", 1);
            r.token
        };
        let id = Identity::open(path, clock).unwrap();
        assert_eq!((id.len(), id.live_count()), (1, 0));
        assert_eq!(id.password_state("alice"), PwState::Hash(H.into()));
        assert!(login_token(&id, "alice", &token, 1).0.is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn newer_file_format_is_refused() {
        let dir = scratch("newer");
        let path = dir.join("accounts.json");
        std::fs::write(&path, r#"{"format": 99, "accounts": []}"#).unwrap();
        let err = Identity::open(path, wall()).err().unwrap().to_string();
        assert!(err.contains("newer"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
