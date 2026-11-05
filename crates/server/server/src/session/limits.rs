//! Rate and size limits. Pure state machines over `Clock` milliseconds:
//! no timers, no sockets, so tests drive them with `TestClock`.
//!
//! In use now: the password-failure guard, the unauthenticated-connection cap and the per-session
//! lobby bucket. [`SeatLimits`] is for the seat pipeline (N2.6/N2.7): `Cmd`, `Focus`/`Query`, `Chat`.

use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

/// Configurable limits; `Default` is the table of spec 4.4.
#[derive(Debug, Clone)]
pub struct LimitsConfig {
    /// Connections from one IP that have not logged in yet.
    pub pending_per_ip: u32,
    /// First delay of a `BadPassword` answer; it doubles after `password_soft_fails` (1, 2, 4 ...).
    pub password_delay: Duration,
    /// The delay never grows past this.
    pub password_delay_max: Duration,
    /// Failures in `password_soft_window` after which the delay starts to grow.
    pub password_soft_fails: u32,
    pub password_soft_window: Duration,
    /// Failures in `password_block_window` that block the IP for `password_block_for`.
    pub password_block_fails: u32,
    pub password_block_window: Duration,
    pub password_block_for: Duration,
    /// Failures of one account (from any address) in `account_window` after which its answers are held back.
    pub account_fails: u32,
    pub account_window: Duration,
    /// Registrations per address and in all per `register_window` (spec accounts 4).
    pub register_per_ip: u32,
    pub register_total: u32,
    pub register_window: Duration,
    /// Messages in the send queue of a session; more closes it with 1013.
    pub outbox_msgs: usize,
    /// Bytes in the send queue of a session.
    pub outbox_bytes: usize,
    /// `Lobby` commands per second per session (burst = the same number).
    pub lobby_per_s: u32,
    /// `Cmd` per second per seat, and its burst.
    pub cmd_per_s: u32,
    pub cmd_burst: u32,
    /// `Focus` and `Query` per second per seat.
    pub focus_query_per_s: u32,
    /// `Query` in flight per seat.
    pub queries_in_flight: u32,
    /// `Chat` lines per `chat_window` per seat.
    pub chat_lines: u32,
    pub chat_window: Duration,
}

impl Default for LimitsConfig {
    fn default() -> Self {
        Self {
            pending_per_ip: 4,
            password_delay: Duration::from_secs(1),
            password_delay_max: Duration::from_secs(30),
            password_soft_fails: 5,
            password_soft_window: Duration::from_secs(60),
            password_block_fails: 10,
            password_block_window: Duration::from_secs(10 * 60),
            password_block_for: Duration::from_secs(10 * 60),
            account_fails: 10,
            account_window: Duration::from_secs(10 * 60),
            register_per_ip: 5,
            register_total: 50,
            register_window: Duration::from_secs(60 * 60),
            outbox_msgs: 256,
            outbox_bytes: 4 << 20,
            lobby_per_s: 10,
            cmd_per_s: 100,
            cmd_burst: 200,
            focus_query_per_s: 20,
            queries_in_flight: 8,
            chat_lines: 5,
            chat_window: Duration::from_secs(10),
        }
    }
}

fn ms(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

/// Token bucket: `rate` tokens per second up to `burst`; starts full.
#[derive(Debug, Clone)]
pub struct TokenBucket {
    rate_per_s: u64,
    /// Capacity in milli-tokens.
    cap: u64,
    /// Milli-tokens now.
    tokens: u64,
    last_ms: u64,
}

impl TokenBucket {
    pub fn new(rate_per_s: u32, burst: u32, now_ms: u64) -> Self {
        let cap = u64::from(burst) * 1000;
        Self {
            rate_per_s: u64::from(rate_per_s),
            cap,
            tokens: cap,
            last_ms: now_ms,
        }
    }

    /// Takes one token; `false` if the bucket is empty.
    pub fn try_take(&mut self, now_ms: u64) -> bool {
        let elapsed = now_ms.saturating_sub(self.last_ms);
        self.last_ms = self.last_ms.max(now_ms);
        // 1 token per (1000 / rate) ms is `rate` milli-tokens per ms.
        self.tokens = self.cap.min(
            self.tokens
                .saturating_add(elapsed.saturating_mul(self.rate_per_s)),
        );
        if self.tokens >= 1000 {
            self.tokens -= 1000;
            true
        } else {
            false
        }
    }
}

/// At most `max` events in any window of `window_ms`.
#[derive(Debug, Clone)]
pub struct WindowCounter {
    max: usize,
    window_ms: u64,
    hits: VecDeque<u64>,
}

impl WindowCounter {
    pub fn new(max: u32, window: Duration) -> Self {
        Self {
            max: max as usize,
            window_ms: ms(window),
            hits: VecDeque::new(),
        }
    }

    /// Counts an event; `false` (and no count) if the window is full.
    pub fn try_hit(&mut self, now_ms: u64) -> bool {
        while self
            .hits
            .front()
            .is_some_and(|t| now_ms.saturating_sub(*t) >= self.window_ms)
        {
            self.hits.pop_front();
        }
        if self.hits.len() >= self.max {
            return false;
        }
        self.hits.push_back(now_ms);
        true
    }

    /// Milliseconds until the oldest event leaves the window (0 if there is room now).
    pub fn retry_after_ms(&self, now_ms: u64) -> u64 {
        if self.hits.len() < self.max {
            return 0;
        }
        self.hits
            .front()
            .map_or(0, |t| (t + self.window_ms).saturating_sub(now_ms))
    }
}

/// Limits of one session outside a game: the lobby command bucket.
#[derive(Debug, Clone)]
pub struct SessionLimits {
    lobby: TokenBucket,
}

impl SessionLimits {
    pub fn new(cfg: &LimitsConfig, now_ms: u64) -> Self {
        Self {
            lobby: TokenBucket::new(cfg.lobby_per_s, cfg.lobby_per_s, now_ms),
        }
    }

    /// A `Lobby` command arrives; `false` means it is over the limit.
    pub fn lobby_ok(&mut self, now_ms: u64) -> bool {
        self.lobby.try_take(now_ms)
    }
}

/// Limits of one seat. Used by the seat pipeline.
#[derive(Debug, Clone)]
pub struct SeatLimits {
    cmd: TokenBucket,
    focus_query: TokenBucket,
    chat: WindowCounter,
    max_queries: u32,
    queries: u32,
}

impl SeatLimits {
    pub fn new(cfg: &LimitsConfig, now_ms: u64) -> Self {
        Self {
            cmd: TokenBucket::new(cfg.cmd_per_s, cfg.cmd_burst, now_ms),
            focus_query: TokenBucket::new(cfg.focus_query_per_s, cfg.focus_query_per_s, now_ms),
            chat: WindowCounter::new(cfg.chat_lines, cfg.chat_window),
            max_queries: cfg.queries_in_flight,
            queries: 0,
        }
    }

    /// `false`: answer `Rejected { TooMany }`.
    pub fn cmd_ok(&mut self, now_ms: u64) -> bool {
        self.cmd.try_take(now_ms)
    }

    /// `Focus`; `false`: drop it.
    pub fn focus_ok(&mut self, now_ms: u64) -> bool {
        self.focus_query.try_take(now_ms)
    }

    /// `Query`: rate and the number in flight. On `true` the caller owes a [`Self::query_done`].
    pub fn query_ok(&mut self, now_ms: u64) -> bool {
        if self.queries >= self.max_queries || !self.focus_query.try_take(now_ms) {
            return false;
        }
        self.queries += 1;
        true
    }

    pub fn query_done(&mut self) {
        self.queries = self.queries.saturating_sub(1);
    }

    /// `Chat` line rate.
    pub fn chat_ok(&mut self, now_ms: u64) -> bool {
        self.chat.try_hit(now_ms)
    }
}

/// Text of a chat line: at most 500 characters, no control characters.
pub fn chat_text_ok(text: &str) -> bool {
    text.chars().count() <= 500 && !text.chars().any(char::is_control)
}

#[derive(Default)]
struct IpState {
    /// Times of password failures, oldest first, within the block window.
    fails: VecDeque<u64>,
    blocked_until: u64,
    /// Connections that have not logged in yet.
    pending: u32,
}

// SIMPLIFIED(SIMP-exe-844): wrong passwords only slow the answers down (per address: delay, then a 10 minute block; per
// account: a growing delay from any address); an account is never locked, so a stranger cannot lock its owner out.

/// Per-IP state of the gateway: password failures (delay, block) and unauthenticated connections.
pub struct Gate {
    cfg: LimitsConfig,
    ips: Mutex<HashMap<IpAddr, IpState>>,
    /// Times of password failures per account key (any address), within `account_window`.
    accounts: Mutex<HashMap<String, VecDeque<u64>>>,
    /// Registrations per address, and in all.
    registrations: Mutex<Registrations>,
}

struct Registrations {
    per_ip: HashMap<IpAddr, WindowCounter>,
    total: WindowCounter,
}

/// A connection counted in [`Gate`] until it logs in or ends. Releases on drop.
pub struct PendingGuard {
    gate: Arc<Gate>,
    ip: IpAddr,
    held: bool,
}

impl PendingGuard {
    /// The connection logged in: it no longer counts as pending.
    pub fn release(&mut self) {
        if std::mem::take(&mut self.held) {
            self.gate.leave_pending(self.ip);
        }
    }
}

impl Drop for PendingGuard {
    fn drop(&mut self) {
        self.release();
    }
}

impl Gate {
    pub fn new(cfg: LimitsConfig) -> Arc<Self> {
        let total = WindowCounter::new(cfg.register_total, cfg.register_window);
        Arc::new(Self {
            cfg,
            ips: Mutex::new(HashMap::new()),
            accounts: Mutex::new(HashMap::new()),
            registrations: Mutex::new(Registrations {
                per_ip: HashMap::new(),
                total,
            }),
        })
    }

    pub fn config(&self) -> &LimitsConfig {
        &self.cfg
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<IpAddr, IpState>> {
        self.ips.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Counts a new connection of `ip`; `None` if it already has `pending_per_ip` unfinished ones.
    pub fn enter_pending(self: &Arc<Self>, ip: IpAddr) -> Option<PendingGuard> {
        let mut ips = self.lock();
        let st = ips.entry(ip).or_default();
        if st.pending >= self.cfg.pending_per_ip {
            return None;
        }
        st.pending += 1;
        Some(PendingGuard {
            gate: Arc::clone(self),
            ip,
            held: true,
        })
    }

    fn leave_pending(&self, ip: IpAddr) {
        let mut ips = self.lock();
        if let Some(st) = ips.get_mut(&ip) {
            st.pending = st.pending.saturating_sub(1);
            if st.is_idle() {
                ips.remove(&ip);
            }
        }
    }

    /// Is `ip` blocked from password logins now?
    pub fn is_blocked(&self, ip: IpAddr, now_ms: u64) -> bool {
        self.lock()
            .get(&ip)
            .is_some_and(|st| st.blocked_until > now_ms)
    }

    /// How long `ip` stays blocked from password checks, if it is.
    pub fn blocked_for(&self, ip: IpAddr, now_ms: u64) -> Option<Duration> {
        self.lock()
            .get(&ip)
            .filter(|st| st.blocked_until > now_ms)
            .map(|st| Duration::from_millis(st.blocked_until - now_ms))
    }

    /// How long an attempt on the account `key` is held back now: nothing below `account_fails` failures in
    /// `account_window`, then the base delay, doubling with every further failure up to the cap. No lockout.
    pub fn account_delay(&self, key: &str, now_ms: u64) -> Duration {
        let c = &self.cfg;
        let mut map = self.accounts.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(q) = map.get_mut(key) else {
            return Duration::ZERO;
        };
        let window = ms(c.account_window);
        while q
            .front()
            .is_some_and(|t| now_ms.saturating_sub(*t) >= window)
        {
            q.pop_front();
        }
        let n = q.len();
        if q.is_empty() {
            map.remove(key);
        }
        let over = n.checked_sub(c.account_fails as usize);
        match over {
            None => Duration::ZERO,
            Some(over) => {
                let shift = u32::try_from(over).unwrap_or(u32::MAX).min(16);
                c.password_delay
                    .saturating_mul(1u32 << shift)
                    .min(c.password_delay_max)
            }
        }
    }

    /// Records a wrong password for the account `key`, whoever sent it (also for a nick without an account, so that the
    /// answer does not tell).
    pub fn record_account_failure(&self, key: &str, now_ms: u64) {
        let window = ms(self.cfg.account_window);
        let mut map = self.accounts.lock().unwrap_or_else(PoisonError::into_inner);
        if map.len() > 4096 {
            map.retain(|_, q| q.back().is_some_and(|t| now_ms.saturating_sub(*t) < window));
        }
        let q = map.entry(key.to_owned()).or_default();
        while q
            .front()
            .is_some_and(|t| now_ms.saturating_sub(*t) >= window)
        {
            q.pop_front();
        }
        q.push_back(now_ms);
    }

    /// A registration from `ip` is about to be made: `Err(retry after)` if the address has made `register_per_ip` in
    /// `register_window`, or the server `register_total`. Counts on `Ok`.
    pub fn register_ok(&self, ip: IpAddr, now_ms: u64) -> Result<(), Duration> {
        let c = &self.cfg;
        let mut r = self
            .registrations
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if r.per_ip.len() > 4096 {
            r.per_ip.retain(|_, w| w.retry_after_ms(now_ms) > 0);
        }
        let ip_counter = r
            .per_ip
            .entry(ip)
            .or_insert_with(|| WindowCounter::new(c.register_per_ip, c.register_window));
        let ip_wait = ip_counter.retry_after_ms(now_ms);
        let total_wait = r.total.retry_after_ms(now_ms);
        // `retry_after_ms` is 0 while there is room; a full window of events still in it is not 0.
        if ip_wait > 0 || total_wait > 0 {
            return Err(Duration::from_millis(ip_wait.max(total_wait).max(1)));
        }
        let ok_ip = r.per_ip.get_mut(&ip).is_some_and(|w| w.try_hit(now_ms));
        let ok_total = r.total.try_hit(now_ms);
        if ok_ip && ok_total {
            Ok(())
        } else {
            Err(Duration::from_secs(1))
        }
    }

    /// Records a wrong password from `ip` and returns how long to hold the answer back.
    /// Reaching `password_block_fails` in the block window blocks the IP.
    pub fn record_password_failure(&self, ip: IpAddr, now_ms: u64) -> Duration {
        let c = &self.cfg;
        let mut ips = self.lock();
        // Keep the table small: forget addresses with nothing to remember.
        if ips.len() > 4096 {
            ips.retain(|_, st| {
                st.prune(now_ms, ms(c.password_block_window));
                !st.is_idle_at(now_ms)
            });
        }
        let st = ips.entry(ip).or_default();
        st.prune(now_ms, ms(c.password_block_window));
        st.fails.push_back(now_ms);
        let in_block_window = st.fails.len();
        if in_block_window >= c.password_block_fails as usize {
            st.blocked_until = now_ms.saturating_add(ms(c.password_block_for));
        }
        let soft = ms(c.password_soft_window);
        let recent = st
            .fails
            .iter()
            .filter(|t| now_ms.saturating_sub(**t) < soft)
            .count();
        let over = recent.saturating_sub(c.password_soft_fails as usize);
        let shift = u32::try_from(over).unwrap_or(u32::MAX).min(16);
        c.password_delay
            .saturating_mul(1u32 << shift)
            .min(c.password_delay_max)
    }
}

impl IpState {
    fn prune(&mut self, now_ms: u64, window_ms: u64) {
        while self
            .fails
            .front()
            .is_some_and(|t| now_ms.saturating_sub(*t) >= window_ms)
        {
            self.fails.pop_front();
        }
    }

    fn is_idle(&self) -> bool {
        self.pending == 0 && self.fails.is_empty() && self.blocked_until == 0
    }

    fn is_idle_at(&self, now_ms: u64) -> bool {
        self.pending == 0 && self.fails.is_empty() && self.blocked_until <= now_ms
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(n: u8) -> IpAddr {
        IpAddr::from([10, 0, 0, n])
    }

    #[test]
    fn bucket_allows_a_burst_then_refills() {
        let mut b = TokenBucket::new(10, 10, 0);
        for _ in 0..10 {
            assert!(b.try_take(0));
        }
        assert!(!b.try_take(0));
        // 100 ms = one token at 10 per second.
        assert!(!b.try_take(99));
        assert!(b.try_take(100));
        assert!(!b.try_take(100));
        // A long pause fills only to the burst.
        for _ in 0..10 {
            assert!(b.try_take(60_000));
        }
        assert!(!b.try_take(60_000));
    }

    #[test]
    fn bucket_ignores_a_clock_that_steps_back() {
        let mut b = TokenBucket::new(1, 1, 5_000);
        assert!(b.try_take(5_000));
        assert!(!b.try_take(1_000));
        assert!(b.try_take(6_000));
    }

    #[test]
    fn window_counts_lines_per_window() {
        let mut w = WindowCounter::new(5, Duration::from_secs(10));
        for t in 0..5 {
            assert!(w.try_hit(t * 100));
        }
        assert!(!w.try_hit(900));
        assert!(!w.try_hit(9_999));
        // The first line leaves the window at 10 000 ms.
        assert!(w.try_hit(10_000));
        assert!(!w.try_hit(10_001));
    }

    #[test]
    fn seat_limits_follow_the_table() {
        let cfg = LimitsConfig::default();
        let mut s = SeatLimits::new(&cfg, 0);
        for _ in 0..200 {
            assert!(s.cmd_ok(0));
        }
        assert!(!s.cmd_ok(0));
        // 100 per second refill: 10 ms is one command.
        assert!(s.cmd_ok(10));

        let mut s = SeatLimits::new(&cfg, 0);
        for _ in 0..8 {
            assert!(s.query_ok(0));
        }
        assert!(!s.query_ok(1_000), "eight queries in flight is the cap");
        s.query_done();
        assert!(s.query_ok(1_000));

        let mut s = SeatLimits::new(&cfg, 0);
        for _ in 0..20 {
            assert!(s.focus_ok(0));
        }
        assert!(!s.focus_ok(0));
    }

    #[test]
    fn chat_text_rules() {
        assert!(chat_text_ok("hello"));
        assert!(chat_text_ok(&"я".repeat(500)));
        assert!(!chat_text_ok(&"я".repeat(501)));
        assert!(!chat_text_ok("a\nb"));
        assert!(!chat_text_ok("a\u{7}"));
    }

    #[test]
    fn password_delay_grows_after_five_failures() {
        let gate = Gate::new(LimitsConfig::default());
        let secs = |n: u64| Duration::from_secs(n);
        let got: Vec<Duration> = (0..9)
            .map(|i| gate.record_password_failure(ip(1), i * 1_000))
            .collect();
        // Five at the base delay, then 2, 4, 8, 16.
        assert_eq!(
            got,
            [
                secs(1),
                secs(1),
                secs(1),
                secs(1),
                secs(1),
                secs(2),
                secs(4),
                secs(8),
                secs(16)
            ]
        );
        // Another address is untouched.
        assert_eq!(gate.record_password_failure(ip(2), 9_000), secs(1));
    }

    #[test]
    fn password_delay_is_capped_and_failures_age_out() {
        let gate = Gate::new(LimitsConfig {
            password_block_fails: 1000,
            ..LimitsConfig::default()
        });
        let mut last = Duration::ZERO;
        for i in 0..12 {
            last = gate.record_password_failure(ip(1), i);
        }
        assert_eq!(last, Duration::from_secs(30));
        // A minute later the short window is empty again.
        assert_eq!(
            gate.record_password_failure(ip(1), 61_000),
            Duration::from_secs(1)
        );
    }

    #[test]
    fn ten_failures_in_ten_minutes_block_the_ip_for_ten_minutes() {
        let gate = Gate::new(LimitsConfig::default());
        // One failure a minute: never five in 60 s, but ten in ten minutes.
        for i in 0..9 {
            gate.record_password_failure(ip(1), i * 60_000);
            assert!(!gate.is_blocked(ip(1), i * 60_000));
        }
        let t = 9 * 60_000 - 1;
        gate.record_password_failure(ip(1), t);
        assert!(gate.is_blocked(ip(1), t));
        assert!(gate.is_blocked(ip(1), t + 600_000 - 1));
        assert!(!gate.is_blocked(ip(1), t + 600_000));
        assert!(!gate.is_blocked(ip(2), t));
    }

    #[test]
    fn pending_connections_are_capped_per_ip() {
        let gate = Gate::new(LimitsConfig::default());
        let mut held: Vec<PendingGuard> =
            (0..4).map(|_| gate.enter_pending(ip(1)).unwrap()).collect();
        assert!(gate.enter_pending(ip(1)).is_none());
        // Another address has its own count.
        assert!(gate.enter_pending(ip(2)).is_some());
        // A login frees a slot; so does a drop.
        held[0].release();
        held[0].release();
        let again = gate.enter_pending(ip(1)).unwrap();
        assert!(gate.enter_pending(ip(1)).is_none());
        drop(again);
        held.pop();
        assert!(gate.enter_pending(ip(1)).is_some());
    }

    #[test]
    fn a_blocked_address_knows_how_long_it_waits() {
        let gate = Gate::new(LimitsConfig::default());
        for i in 0..10 {
            gate.record_password_failure(ip(1), i);
        }
        assert_eq!(
            gate.blocked_for(ip(1), 9),
            Some(Duration::from_millis(600_000))
        );
        assert_eq!(gate.blocked_for(ip(1), 600_009), None);
        assert_eq!(gate.blocked_for(ip(2), 9), None);
    }

    #[test]
    fn an_account_under_attack_answers_slowly_for_everybody_but_is_not_locked() {
        let gate = Gate::new(LimitsConfig::default());
        let secs = Duration::from_secs;
        for i in 0..9 {
            gate.record_account_failure("alice", i * 1000);
        }
        // Nine failures: no delay yet.
        assert_eq!(gate.account_delay("alice", 9_000), Duration::ZERO);
        gate.record_account_failure("alice", 9_000);
        assert_eq!(gate.account_delay("alice", 9_500), secs(1));
        gate.record_account_failure("alice", 9_600);
        gate.record_account_failure("alice", 9_700);
        assert_eq!(gate.account_delay("alice", 9_800), secs(4));
        // Capped, and another account is untouched.
        for i in 0..40 {
            gate.record_account_failure("alice", 10_000 + i);
        }
        assert_eq!(gate.account_delay("alice", 10_100), secs(30));
        assert_eq!(gate.account_delay("bob", 10_100), Duration::ZERO);
        // Ten minutes later the failures have aged out.
        assert_eq!(
            gate.account_delay("alice", 10_000 + 600_000 + 100),
            Duration::ZERO
        );
    }

    #[test]
    fn registrations_are_limited_per_address_and_in_all() {
        let gate = Gate::new(LimitsConfig::default());
        for i in 0..5 {
            gate.register_ok(ip(1), i * 1000).unwrap();
        }
        // The sixth in the hour: wait until the first leaves the window.
        let wait = gate.register_ok(ip(1), 5_000).unwrap_err();
        assert_eq!(wait, Duration::from_millis(3_600_000 - 5_000));
        // Another address is fine, the first is again an hour on.
        gate.register_ok(ip(2), 5_000).unwrap();
        gate.register_ok(ip(1), 3_600_000).unwrap();

        // 50 in all per hour.
        let gate = Gate::new(LimitsConfig::default());
        for n in 0..50u32 {
            let a = IpAddr::from([10, 1, (n / 250) as u8, (n % 250) as u8]);
            gate.register_ok(a, 1_000).unwrap();
        }
        assert!(gate.register_ok(ip(9), 2_000).is_err());
        assert!(gate.register_ok(ip(9), 3_601_000).is_ok());
    }

    #[test]
    fn window_counter_tells_when_there_is_room_again() {
        let mut w = WindowCounter::new(2, Duration::from_secs(10));
        assert_eq!(w.retry_after_ms(0), 0);
        assert!(w.try_hit(1_000));
        assert!(w.try_hit(2_000));
        assert_eq!(w.retry_after_ms(3_000), 8_000);
        assert_eq!(w.retry_after_ms(11_000), 0);
    }
}
