//! What the HTTP auth routes (`http/auth.rs`) and the WebSocket login (`session/login.rs`) share: the argon2 pool, the
//! registration gate, and the password check with its limits (spec accounts-login-lobby 4).

use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Semaphore;

use crate::Shared;
use crate::identity::{self, PwState, nick_key};

/// Argon2 runs at most this many at once (each takes 19 MiB and a core for a moment), so that a login flood cannot
/// eat the AI workers' CPUs. The rest wait in a queue.
pub const HASH_PARALLEL: usize = 2;

/// Hashes and verifies passwords on the blocking pool, `HASH_PARALLEL` at a time.
pub struct Hasher {
    sem: Arc<Semaphore>,
}

impl Default for Hasher {
    fn default() -> Self {
        Self::new()
    }
}

impl Hasher {
    pub fn new() -> Self {
        Self {
            sem: Arc::new(Semaphore::new(HASH_PARALLEL)),
        }
    }

    async fn run<T: Send + 'static>(&self, f: impl FnOnce() -> T + Send + 'static) -> Option<T> {
        let permit = Arc::clone(&self.sem).acquire_owned().await.ok()?;
        let out = tokio::task::spawn_blocking(move || {
            let r = f();
            drop(permit);
            r
        })
        .await;
        out.ok()
    }

    /// The PHC hash of `password`; `None` if the hasher failed.
    pub async fn hash(&self, password: String) -> Option<String> {
        self.run(move || identity::hash_password(&password))
            .await?
            .map_err(|e| tracing::error!(error = %e, "cannot hash a password"))
            .ok()
    }

    /// Whether `password` opens `hash`. `None` (no account, or a legacy one) verifies against the dummy hash, so the
    /// answer takes as long as for a real account, and is always `false`.
    pub async fn verify(&self, hash: Option<String>, password: String) -> bool {
        let real = hash.is_some();
        let hash = hash.unwrap_or_else(|| identity::dummy_hash().to_owned());
        let ok = self
            .run(move || identity::verify_password(&hash, &password))
            .await
            .unwrap_or(false);
        real && ok
    }
}

/// Why a password was not accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PwFail {
    /// The address is blocked; nothing was checked. The time is what is left of the block.
    Blocked(Duration),
    /// Wrong nick or password (the answer was held back as the limits say).
    Wrong,
}

// SIMPLIFIED(SIMP-exe-858): a public server (deploy/compose.public.yaml) does not run with `open_access` any more: registration needs the
// invitation code, the shared server password (`CIV5_PASSWORD`), which is no login password.
/// May `candidate` register accounts? Open access: anybody. Otherwise the shared server password is the code.
pub fn code_ok(shared: &Shared, candidate: &str) -> bool {
    shared.config.open_access
        || shared
            .config
            .password
            .as_ref()
            .is_some_and(|p| p.matches(candidate))
}

/// `"open"` or `"code"` (for `GET /api/auth/info`).
pub fn registration_mode(shared: &Shared) -> &'static str {
    if shared.config.open_access {
        "open"
    } else {
        "code"
    }
}

/// Checks `password` for `nick`: the per-IP block, the per-account delay, the argon2 verification, and on a failure the
/// counts and the delay of the answer. `Ok` carries the hash that matched, for `Identity::login` / `login_http`.
pub async fn check_password(
    shared: &Shared,
    ip: IpAddr,
    nick: &str,
    password: &str,
) -> Result<String, PwFail> {
    let now = shared.config.clock.now_ms();
    if let Some(left) = shared.gate.blocked_for(ip, now) {
        return Err(PwFail::Blocked(left));
    }
    let key = nick_key(nick);
    // An account under attack answers slowly for everybody (no lockout).
    let slow = shared.gate.account_delay(&key, now);
    if !slow.is_zero() {
        tokio::time::sleep(slow).await;
    }
    let hash = match shared.identity.password_state(nick) {
        PwState::Hash(h) => Some(h),
        PwState::Legacy | PwState::Unknown => None,
    };
    if shared
        .hasher
        .verify(hash.clone(), password.to_owned())
        .await
        && let Some(hash) = hash
    {
        return Ok(hash);
    }
    let now = shared.config.clock.now_ms();
    shared.gate.record_account_failure(&key, now);
    let delay = shared.gate.record_password_failure(ip, now);
    tokio::time::sleep(delay).await;
    Err(PwFail::Wrong)
}
