//! Connection health and reconnecting (net-client 4.7, 4.8; NC-D8, NC-D15, NC-D17), as pure logic.
//!
//! `Link` owns the sockets (by number, never by handle), the liveness timers and the reconnect schedule.
//! The shell executes what `Link` asks (`LinkAct`) and reports what happened (`on_open`, `on_bytes`,
//! `on_close`). Timeouts count only in a visible tab. Every socket has a number; frames of a socket that is
//! neither the current one nor a live attempt are ignored, which is how a replaced old socket (NC-D15) and a
//! late probe stay harmless.

/// Number of a socket, assigned by `ClientCore` (generation, NC-D15).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Sock(pub u32);

/// What the shell does with sockets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocketCmd {
    /// Open a new WebSocket. `probe`: it is an attempt that replaces the current socket once it has
    /// logged in (NC-D17); an attempt without a current socket is the same.
    Open { sock: Sock, probe: bool },
    /// Close it (code 1000). The shell then reports `on_disconnected(.., Local)`.
    Close { sock: Sock },
}

/// Why a socket closed, as the shell saw it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisconnectWhy {
    /// We asked for it.
    Local,
    /// The server closed it with a code (1012 restarting, 1001 going away, ...).
    Remote(u16),
    /// Network error, including a failed connect.
    Error,
}

/// What the page tells about itself (`NetHandle::host_event`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostEvent {
    Online,
    Offline,
    Visible,
    Hidden,
    PageHide,
}

/// What the player sees of the connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConnState {
    Online,
    Poor,
    Lost,
    Reconnecting,
    Offline,
    Replaced,
    Closed,
}

/// For the grey order panel: "Reconnecting… attempt N, next in S s".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReconnectStatus {
    pub attempt: u32,
    pub next_in_ms: u64,
    pub probing: bool,
}

/// What `Link` asks of the core, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LinkAct {
    Socket(SocketCmd),
    Ping {
        sock: Sock,
        seq: u32,
        client_ms: u64,
    },
    State(ConnState),
}

/// Who a report is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Who {
    Current,
    Attempt,
    Stale,
}

/// xorshift64*; the same generator drives every random choice of the client.
#[derive(Debug, Clone)]
pub(crate) struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(if seed == 0 {
            0x9E37_79B9_7F4A_7C15
        } else {
            seed
        })
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// `0..n` (n > 0).
    pub fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n.max(1)
    }
}

/// Server clock from `Pong`s: the estimate with the smallest RTT of the last 8 wins.
#[derive(Debug, Clone, Default)]
pub(crate) struct ClockSync {
    samples: Vec<(u64, i64)>,
}

impl ClockSync {
    const KEEP: usize = 8;

    pub fn add(&mut self, now_ms: u64, client_ms: u64, server_ms: u64) {
        let rtt = now_ms.saturating_sub(client_ms);
        let offset = server_ms as i64 + (rtt / 2) as i64 - now_ms as i64;
        if self.samples.len() == Self::KEEP {
            self.samples.remove(0);
        }
        self.samples.push((rtt, offset));
    }

    pub fn rtt(&self) -> Option<u64> {
        self.samples.last().map(|s| s.0)
    }

    pub fn offset(&self) -> i64 {
        self.samples.iter().min_by_key(|s| s.0).map_or(0, |s| s.1)
    }

    pub fn server_ms(&self, now_ms: u64) -> u64 {
        (now_ms as i64 + self.offset()).max(0) as u64
    }
}

#[derive(Debug, Clone)]
struct Cur {
    sock: Sock,
    opened: bool,
    welcomed: bool,
    /// Open and login must be done by then.
    deadline: u64,
}

#[derive(Debug, Clone)]
struct Attempt {
    sock: Sock,
    started: u64,
    opened: bool,
    /// Probe in the outage cadence (short open timeout) or an ordinary backoff attempt.
    probe: bool,
    /// Login must be done by then (set when it opens).
    deadline: u64,
}

pub(crate) struct Link {
    cfg: crate::NetConfig,
    state: ConnState,
    acts: Vec<LinkAct>,
    visible: bool,
    hidden_since: Option<u64>,
    net_online: bool,
    now: u64,
    /// The time of the previous `tick` (a stall of the client shows as a long gap).
    last_tick: Option<u64>,
    /// Silence is counted from `max(last byte, return to the tab)`.
    silence_origin: u64,
    last_byte: u64,
    current: Option<Cur>,
    attempts: Vec<Attempt>,
    next_sock: u32,
    attempt_no: u32,
    next_attempt: Option<u64>,
    probing: bool,
    backoff_step: u32,
    fast_fail: u32,
    stable_from: Option<u64>,
    ping_seq: u32,
    next_ping: u64,
    pending_ping: Option<u64>,
    ping_deadline: Option<u64>,
    offline_next_check: u64,
    started: bool,
    terminal: bool,
    /// After `PageHide`: nothing is opened until the tab is shown again.
    paused: bool,
    /// Set by the core: a token exists and a login succeeded once, so probes can resume.
    can_probe: bool,
    rng: Rng,
    clock: ClockSync,
}

impl Link {
    pub fn new(cfg: crate::NetConfig) -> Self {
        let rng = Rng::new(cfg.jitter_seed);
        Link {
            cfg,
            state: ConnState::Closed,
            acts: Vec::new(),
            visible: true,
            hidden_since: None,
            net_online: true,
            now: 0,
            last_tick: None,
            silence_origin: 0,
            last_byte: 0,
            current: None,
            attempts: Vec::new(),
            next_sock: 1,
            attempt_no: 0,
            next_attempt: None,
            probing: false,
            backoff_step: 0,
            fast_fail: 0,
            stable_from: None,
            ping_seq: 0,
            next_ping: 0,
            pending_ping: None,
            ping_deadline: None,
            offline_next_check: 0,
            started: false,
            terminal: false,
            paused: false,
            can_probe: false,
            rng,
            clock: ClockSync::default(),
        }
    }

    // ------------------------------------------------------------ reading

    pub fn state(&self) -> ConnState {
        self.state
    }

    pub fn current(&self) -> Option<Sock> {
        self.current.as_ref().map(|c| c.sock)
    }

    /// The current socket is open: frames can be sent now.
    pub fn current_open(&self) -> bool {
        self.current.as_ref().is_some_and(|c| c.opened)
    }

    /// `Welcome` has come on the current socket.
    pub fn current_welcomed(&self) -> bool {
        self.current.as_ref().is_some_and(|c| c.welcomed)
    }

    pub fn clock(&self) -> &ClockSync {
        &self.clock
    }

    /// An attempt has opened, so its login is on the wire.
    pub fn attempt_logging_in(&self) -> bool {
        self.attempts.iter().any(|a| a.opened)
    }

    pub fn is_terminal(&self) -> bool {
        self.terminal
    }

    pub fn reconnect_status(&self) -> Option<ReconnectStatus> {
        match self.state {
            ConnState::Lost | ConnState::Reconnecting => Some(ReconnectStatus {
                attempt: self.attempt_no,
                next_in_ms: self.next_attempt.map_or(0, |t| t.saturating_sub(self.now)),
                probing: self.probing,
            }),
            ConnState::Offline => Some(ReconnectStatus {
                attempt: self.attempt_no,
                next_in_ms: self.offline_next_check.saturating_sub(self.now),
                probing: false,
            }),
            _ => None,
        }
    }

    pub fn drain(&mut self) -> Vec<LinkAct> {
        std::mem::take(&mut self.acts)
    }

    pub fn set_can_probe(&mut self, v: bool) {
        self.can_probe = v;
    }

    // ------------------------------------------------------------ commands from the core

    /// Start: open the first socket.
    pub fn connect(&mut self, now: u64) {
        self.now = now;
        if self.started {
            return;
        }
        self.started = true;
        self.last_byte = now;
        self.silence_origin = now;
        self.attempt_no = 1;
        self.open_current(now);
        self.set_state(ConnState::Reconnecting);
    }

    /// Start over after a terminal state (`Replaced`: "use this tab"; `Closed`: retry).
    pub fn restart(&mut self, now: u64) {
        self.now = now;
        self.close_all();
        self.terminal = false;
        self.paused = false;
        self.started = true;
        self.backoff_step = 0;
        self.fast_fail = 0;
        self.attempt_no = 1;
        self.last_byte = now;
        self.silence_origin = now;
        self.open_current(now);
        self.set_state(ConnState::Reconnecting);
    }

    /// The stream is broken but the line may be fine: close the socket and reconnect at once. The ladder
    /// above decides whether `resume` is offered.
    pub fn break_current(&mut self, now: u64) {
        self.now = now;
        if !self.started || self.terminal || self.paused {
            return;
        }
        self.lose_current(now);
    }

    /// Replaced / Closed: nothing is reopened by itself (NC-D15, "never reconnect after Replaced/Reject").
    pub fn finish(&mut self, state: ConnState) {
        self.close_all();
        self.terminal = true;
        self.next_attempt = None;
        self.set_state(state);
    }

    pub fn welcomed(&mut self, now: u64) {
        self.now = now;
        if let Some(c) = &mut self.current {
            c.welcomed = true;
        }
        self.next_ping = now;
    }

    /// `LoggedIn` arrived on the current socket: the handshake is over.
    pub fn logged_in(&mut self) {
        if let Some(c) = &mut self.current {
            c.deadline = u64::MAX;
        }
    }

    /// The game stream runs: after `stable_after_ms` of it the backoff starts over.
    pub fn attached(&mut self, now: u64) {
        self.stable_from = Some(now);
    }

    pub fn pong(&mut self, now: u64, client_ms: u64, server_ms: u64) {
        self.pending_ping = None;
        self.ping_deadline = None;
        self.clock.add(now, client_ms, server_ms);
    }

    // ------------------------------------------------------------ reports from the shell

    pub fn on_open(&mut self, now: u64, sock: Sock) -> Who {
        self.now = now;
        if let Some(c) = &mut self.current
            && c.sock == sock
        {
            c.opened = true;
            c.deadline = now + self.cfg.handshake_timeout_ms;
            self.silence_origin = self.silence_origin.max(now);
            return Who::Current;
        }
        if let Some(a) = self.attempts.iter_mut().find(|a| a.sock == sock) {
            a.opened = true;
            a.deadline = now + self.cfg.handshake_timeout_ms;
            return Who::Attempt;
        }
        Who::Stale
    }

    /// Bytes arrived. From the current socket they prove the line is alive.
    pub fn on_bytes(&mut self, now: u64, sock: Sock) -> Who {
        self.now = now;
        if self.current() == Some(sock) {
            self.last_byte = now;
            self.silence_origin = self.silence_origin.max(now);
            if matches!(
                self.state,
                ConnState::Poor | ConnState::Offline | ConnState::Reconnecting
            ) {
                self.net_online = true;
                // The old socket lives: probes still connecting are not needed. Those whose login is on
                // the wire finish: if one gets through, it takes the session over and the old one closes.
                self.close_unopened_attempts();
                self.set_state(ConnState::Online);
            }
            return Who::Current;
        }
        if self.attempts.iter().any(|a| a.sock == sock) {
            Who::Attempt
        } else {
            Who::Stale
        }
    }

    /// A socket is gone.
    pub fn on_close(&mut self, now: u64, sock: Sock, why: DisconnectWhy) -> Who {
        self.now = now;
        if self.current() == Some(sock) {
            let was_open = self.current.take().is_some_and(|c| c.opened);
            self.pending_ping = None;
            self.ping_deadline = None;
            if self.terminal || self.paused || !self.started {
                return Who::Current;
            }
            // A current socket that never got to open is a refusal: back off.
            if !was_open && why != DisconnectWhy::Local {
                self.fast_fail += 1;
            }
            self.lost(now);
            return Who::Current;
        }
        if let Some(i) = self.attempts.iter().position(|a| a.sock == sock) {
            let a = self.attempts.remove(i);
            if !self.terminal && !self.paused {
                let failed = why != DisconnectWhy::Local;
                self.attempt_gone(now, failed, a.opened);
            }
            return Who::Attempt;
        }
        Who::Stale
    }

    /// An attempt has logged in: it becomes the current socket, the old one closes (NC-D17).
    pub fn promote(&mut self, now: u64, sock: Sock) {
        self.now = now;
        let Some(i) = self.attempts.iter().position(|a| a.sock == sock) else {
            return;
        };
        self.attempts.remove(i);
        if let Some(old) = self.current.take() {
            self.acts
                .push(LinkAct::Socket(SocketCmd::Close { sock: old.sock }));
        }
        self.close_attempts();
        self.current = Some(Cur {
            sock,
            opened: true,
            welcomed: true,
            deadline: u64::MAX,
        });
        self.attempt_no = 0;
        self.fast_fail = 0;
        self.next_attempt = None;
        self.probing = false;
        self.last_byte = now;
        self.silence_origin = now;
        self.pending_ping = None;
        self.ping_deadline = None;
        self.next_ping = now;
        self.stable_from = None;
        self.set_state(ConnState::Online);
    }

    // ------------------------------------------------------------ page events

    pub fn host_event(&mut self, now: u64, ev: HostEvent) {
        self.now = now;
        match ev {
            HostEvent::Hidden => {
                self.visible = false;
                self.hidden_since = Some(now);
                self.pending_ping = None;
                self.ping_deadline = None;
            }
            HostEvent::Visible => {
                let long = self
                    .hidden_since
                    .take()
                    .is_some_and(|h| now.saturating_sub(h) > self.cfg.probe_after_hidden_ms);
                self.visible = true;
                self.silence_origin = now;
                if self.paused {
                    self.paused = false;
                    self.reconnect_now(now);
                } else if self.started && !self.terminal && self.current.is_none() {
                    self.reconnect_now(now);
                } else {
                    self.ping_now(now, long);
                }
            }
            HostEvent::Online => {
                self.net_online = true;
                if !self.started || self.terminal || self.paused {
                    return;
                }
                if self.current_open() {
                    if self.state == ConnState::Offline {
                        self.set_state(ConnState::Online);
                    }
                    self.silence_origin = now;
                    self.ping_now(now, true);
                } else {
                    self.reconnect_now(now);
                }
            }
            HostEvent::Offline => {
                self.net_online = false;
                if !self.started || self.terminal {
                    return;
                }
                self.close_attempts();
                self.next_attempt = None;
                self.offline_next_check = now + self.cfg.offline_check_ms;
                self.set_state(ConnState::Offline);
            }
            HostEvent::PageHide => {
                if !self.started || self.terminal {
                    return;
                }
                self.close_all();
                self.paused = true;
                self.next_attempt = None;
                self.set_state(ConnState::Lost);
            }
        }
    }

    // ------------------------------------------------------------ time

    pub fn tick(&mut self, now: u64) {
        self.now = now;
        let gap = self.last_tick.map_or(0, |t| now.saturating_sub(t));
        self.last_tick = Some(now);
        if !self.started || self.terminal || self.paused {
            return;
        }
        // Our own thread did not run for a while (a heavy frame, a debug build on software GL: seconds). What the
        // server sent meanwhile is queued in the browser and not yet read, so silence counted over the gap says
        // nothing about the line: a city window that opened with a long frame was taken for a dead socket and the
        // connection dropped. Count from now, and give an answer to the probe ping the same time again.
        if gap >= self.stall_ms() {
            tracing::debug!(
                gap_ms = gap,
                "the client stalled, silence is counted from now"
            );
            self.silence_origin = self.silence_origin.max(now);
            if let Some(d) = &mut self.ping_deadline {
                *d = (*d).max(now + self.cfg.probe_wait_ms);
            }
        }
        if let Some(from) = self.stable_from
            && now.saturating_sub(from) >= self.cfg.stable_after_ms
        {
            self.backoff_step = 0;
            self.fast_fail = 0;
            self.stable_from = None;
        }
        self.expire_attempts(now);
        if let Some(c) = &self.current
            && now >= c.deadline
        {
            tracing::warn!("handshake timeout, connection considered lost");
            self.lose_current(now);
        }
        if self.visible && self.current_open() && self.net_online_for_liveness() {
            self.liveness(now);
        }
        if self.state == ConnState::Poor
            && self.can_probe
            && now.saturating_sub(self.silence_origin) >= self.cfg.probe_start_ms
        {
            self.fire_attempt(now);
        }
        if self.current.is_none() {
            match self.state {
                ConnState::Offline if now >= self.offline_next_check => {
                    self.offline_next_check = now + self.cfg.offline_check_ms;
                    if self.attempts.is_empty() {
                        self.probing = false;
                        self.spawn_attempt(now, false);
                    }
                }
                ConnState::Lost | ConnState::Reconnecting
                    if self.next_attempt.is_some_and(|t| now >= t) =>
                {
                    self.fire_attempt(now);
                }
                _ => {}
            }
        }
    }

    /// A gap between two ticks this long is a stall of the client, not of the line: two ping periods (a visible
    /// tab ticks every frame).
    fn stall_ms(&self) -> u64 {
        2 * self.cfg.ping_interval_ms
    }

    fn net_online_for_liveness(&self) -> bool {
        // Offline keeps the socket, but the browser has told us there is no network: no verdicts.
        self.net_online
    }

    fn liveness(&mut self, now: u64) {
        let welcomed = self.current.as_ref().is_some_and(|c| c.welcomed);
        let sock = self.current();
        if welcomed
            && now >= self.next_ping
            && let Some(sock) = sock
        {
            self.send_ping(now, sock);
        }
        if self.ping_deadline.is_some_and(|d| now >= d) {
            tracing::warn!("no answer to the probe ping, connection considered lost");
            self.lose_current(now);
            return;
        }
        let silence = now.saturating_sub(self.silence_origin);
        if silence >= self.cfg.lost_after_ms {
            tracing::warn!(silent_ms = silence, "no bytes, connection considered lost");
            self.lose_current(now);
        } else if silence >= self.cfg.poor_after_ms
            && self.pending_ping.is_some()
            && self.state == ConnState::Online
        {
            self.set_state(ConnState::Poor);
        }
    }

    fn send_ping(&mut self, now: u64, sock: Sock) {
        let seq = self.ping_seq;
        self.ping_seq = self.ping_seq.wrapping_add(1);
        self.next_ping = now + self.cfg.ping_interval_ms;
        self.pending_ping.get_or_insert(now);
        self.acts.push(LinkAct::Ping {
            sock,
            seq,
            client_ms: now,
        });
    }

    /// `Ping` now; `deadline`: the answer is awaited `probe_wait_ms` (return after hiding, `Online`).
    fn ping_now(&mut self, now: u64, deadline: bool) {
        let welcomed = self.current.as_ref().is_some_and(|c| c.welcomed);
        if let (Some(sock), true) = (self.current(), welcomed && self.current_open()) {
            self.send_ping(now, sock);
            if deadline {
                self.ping_deadline = Some(now + self.cfg.probe_wait_ms);
            }
        }
    }

    // ------------------------------------------------------------ loss and reconnecting

    fn lose_current(&mut self, now: u64) {
        if let Some(c) = self.current.take() {
            self.acts
                .push(LinkAct::Socket(SocketCmd::Close { sock: c.sock }));
        }
        self.pending_ping = None;
        self.ping_deadline = None;
        self.lost(now);
    }

    /// No current socket and the game wants one: schedule the first attempt.
    fn lost(&mut self, now: u64) {
        self.stable_from = None;
        // Attempts started while the old socket was fading (Poor) are still useful: keep them.
        if !self.net_online {
            self.set_state(ConnState::Offline);
            self.offline_next_check = now + self.cfg.offline_check_ms;
            return;
        }
        self.set_state(ConnState::Lost);
        if !self.attempts.is_empty() {
            self.set_state(ConnState::Reconnecting);
        }
        let probing = self.probe_mode(now);
        self.next_attempt = Some(if probing { now } else { now + self.backoff() });
    }

    fn probe_mode(&self, now: u64) -> bool {
        self.can_probe
            && self.fast_fail == 0
            && now.saturating_sub(self.last_byte) < self.cfg.outage_window_ms
    }

    /// Immediate reconnect without a pause; the backoff starts over.
    fn reconnect_now(&mut self, now: u64) {
        self.backoff_step = 0;
        self.fast_fail = 0;
        // Attempts that have not opened were started before the network came back: they may hang for
        // their whole timeout. Fresh sockets do not.
        self.close_unopened_attempts();
        if self.state == ConnState::Offline || self.state == ConnState::Online {
            self.set_state(ConnState::Lost);
        }
        self.fire_attempt(now);
    }

    /// An attempt is due: open one in the cadence that fits (probe or backoff).
    fn fire_attempt(&mut self, now: u64) {
        if !self.net_online {
            return;
        }
        self.probing = self.probe_mode(now);
        if self.probing {
            if self.attempts.len() < self.cfg.probe_max_live
                && self
                    .last_spawn()
                    .is_none_or(|t| now.saturating_sub(t) >= self.cfg.probe_every_ms)
            {
                self.spawn_attempt(now, true);
            }
            self.next_attempt = Some(now + self.cfg.probe_every_ms);
        } else {
            if self.attempts.is_empty() {
                self.spawn_attempt(now, false);
            }
            self.next_attempt = None;
        }
    }

    fn last_spawn(&self) -> Option<u64> {
        self.attempts.iter().map(|a| a.started).max()
    }

    fn spawn_attempt(&mut self, now: u64, probe: bool) {
        let sock = self.new_sock();
        self.attempts.push(Attempt {
            sock,
            started: now,
            opened: false,
            probe,
            deadline: u64::MAX,
        });
        self.attempt_no += 1;
        self.acts
            .push(LinkAct::Socket(SocketCmd::Open { sock, probe: true }));
        if self.state == ConnState::Lost {
            self.set_state(ConnState::Reconnecting);
        }
    }

    fn expire_attempts(&mut self, now: u64) {
        let open_timeout_probe = self.cfg.probe_attempt_ms;
        let open_timeout_plain = self.cfg.handshake_timeout_ms;
        let mut gone = Vec::new();
        self.attempts.retain(|a| {
            let timeout = if a.probe {
                open_timeout_probe
            } else {
                open_timeout_plain
            };
            let dead = if a.opened {
                now >= a.deadline
            } else {
                now.saturating_sub(a.started) >= timeout
            };
            if dead {
                gone.push((a.sock, a.opened));
            }
            !dead
        });
        for (sock, opened) in gone {
            self.acts.push(LinkAct::Socket(SocketCmd::Close { sock }));
            // Our own timeout is no refusal: the cadence goes on.
            self.attempt_gone(now, false, opened);
        }
    }

    /// An attempt ended without logging in. In the probe cadence nothing changes unless it was refused;
    /// the ordinary backoff schedules the next one.
    fn attempt_gone(&mut self, now: u64, failed: bool, _opened: bool) {
        if self.current.is_some() {
            return;
        }
        if failed {
            self.fast_fail += 1;
            self.next_attempt = Some(now + self.backoff());
        } else if !self.probing {
            self.next_attempt = Some(now + self.backoff());
        }
        if self.attempts.is_empty() && self.state == ConnState::Reconnecting {
            self.set_state(ConnState::Lost);
        }
    }

    fn backoff(&mut self) -> u64 {
        let step = self.backoff_step.min(20);
        self.backoff_step = self.backoff_step.saturating_add(1);
        let base = (self.cfg.backoff_first_ms << step).min(self.cfg.backoff_max_ms);
        let span = base * self.cfg.backoff_jitter_pct / 100;
        let jittered = base - span + self.rng.below(2 * span + 1);
        jittered.min(self.cfg.backoff_max_ms)
    }

    fn open_current(&mut self, now: u64) {
        let sock = self.new_sock();
        self.current = Some(Cur {
            sock,
            opened: false,
            welcomed: false,
            deadline: now + self.cfg.handshake_timeout_ms,
        });
        self.acts
            .push(LinkAct::Socket(SocketCmd::Open { sock, probe: false }));
    }

    fn new_sock(&mut self) -> Sock {
        let s = Sock(self.next_sock);
        self.next_sock += 1;
        s
    }

    fn close_unopened_attempts(&mut self) {
        let acts = &mut self.acts;
        self.attempts.retain(|a| {
            if !a.opened {
                acts.push(LinkAct::Socket(SocketCmd::Close { sock: a.sock }));
            }
            a.opened
        });
    }

    fn close_attempts(&mut self) {
        for a in std::mem::take(&mut self.attempts) {
            self.acts
                .push(LinkAct::Socket(SocketCmd::Close { sock: a.sock }));
        }
    }

    fn close_all(&mut self) {
        if let Some(c) = self.current.take() {
            self.acts
                .push(LinkAct::Socket(SocketCmd::Close { sock: c.sock }));
        }
        self.close_attempts();
        self.pending_ping = None;
        self.ping_deadline = None;
    }

    fn set_state(&mut self, s: ConnState) {
        if self.state != s {
            self.state = s;
            self.acts.push(LinkAct::State(s));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NetConfig;

    fn cfg() -> NetConfig {
        NetConfig::new("t", "n")
    }

    #[test]
    fn clock_takes_the_estimate_with_the_smallest_rtt() {
        let mut c = ClockSync::default();
        assert_eq!(c.server_ms(1_000), 1_000);
        // rtt 400, server 5_000 at the midpoint of 1_000..1_400 => offset 4_000 - ... computed below.
        c.add(1_400, 1_000, 5_200);
        assert_eq!(c.offset(), 5_200 + 200 - 1_400);
        // A better sample (rtt 40) replaces it.
        c.add(2_040, 2_000, 9_000);
        assert_eq!(c.offset(), 9_000 + 20 - 2_040);
        // A worse one does not.
        c.add(3_500, 3_000, 1_234);
        assert_eq!(c.offset(), 9_000 + 20 - 2_040);
        assert_eq!(c.rtt(), Some(500));
        assert_eq!(c.server_ms(10_000), (10_000 + 9_000 + 20 - 2_040) as u64);
        // Only the last 8 count: the good sample ages out.
        for i in 0..8 {
            c.add(10_000 + i * 10 + 300, 10_000 + i * 10, 20_000);
        }
        assert_eq!(c.samples.len(), 8);
        assert_eq!(c.rtt(), Some(300));
    }

    #[test]
    fn backoff_doubles_to_the_cap_with_jitter() {
        let mut l = Link::new(cfg());
        let mut seen = Vec::new();
        for _ in 0..9 {
            seen.push(l.backoff());
        }
        let nominal = [
            500u64, 1000, 2000, 4000, 8000, 10_000, 10_000, 10_000, 10_000,
        ];
        for (got, nom) in seen.iter().zip(nominal) {
            assert!(
                *got >= nom * 80 / 100 && *got <= (nom * 120 / 100).min(10_000),
                "{got} vs {nom}"
            );
        }
        // Not all equal to nominal: the jitter is real, and it is deterministic for a seed.
        assert!(seen.iter().zip(nominal).any(|(g, n)| *g != n));
        let mut l2 = Link::new(cfg());
        let again: Vec<u64> = (0..9).map(|_| l2.backoff()).collect();
        assert_eq!(seen, again);
        let mut c3 = cfg();
        c3.jitter_seed = 7;
        let mut l3 = Link::new(c3);
        let other: Vec<u64> = (0..9).map(|_| l3.backoff()).collect();
        assert_ne!(seen, other);
    }

    #[test]
    fn ten_seconds_of_stable_play_reset_the_backoff() {
        let mut l = Link::new(cfg());
        l.connect(0);
        let sock = l.current().unwrap();
        l.on_open(10, sock);
        l.logged_in();
        l.backoff_step = 3;
        l.fast_fail = 2;
        l.attached(1_000);
        l.on_bytes(10_990, sock); // the line is alive: no verdict on silence
        l.tick(10_999);
        assert_eq!((l.backoff_step, l.fast_fail), (3, 2));
        l.tick(11_000);
        assert_eq!((l.backoff_step, l.fast_fail), (0, 0));
    }

    #[test]
    fn a_probe_cadence_needs_a_token_and_a_recent_byte() {
        let mut l = Link::new(cfg());
        l.last_byte = 100_000;
        assert!(!l.probe_mode(100_001), "no login yet: ordinary backoff");
        l.set_can_probe(true);
        assert!(l.probe_mode(100_001));
        assert!(l.probe_mode(129_999));
        assert!(!l.probe_mode(130_000), "the outage window is over");
        l.fast_fail = 1;
        assert!(!l.probe_mode(100_001), "a refusal means back off");
    }

    #[test]
    fn rng_is_never_stuck_at_zero() {
        let mut r = Rng::new(0);
        assert_ne!(r.next_u64(), r.next_u64());
        assert_eq!(r.below(1), 0);
    }
}
