//! Connection health, reconnecting and the outage recovery of NC-D17 (net-client 4.7, 9), with fake time.

mod common;

use client_core::{
    ClientCore, ClientEvent, ClientPhase, ConnState, DisconnectWhy, HostEvent, NetConfig,
    OrderRefused, Sock, SocketCmd,
};
use common::*;
use protocol::{AttachMode, ChatTo, ClientMsg, DetachReason, GameCtl, LobbyCmd, ServerMsg, Token};

fn plan() -> GamePlan {
    GamePlan {
        game: 7,
        player: 1,
        stream: 3,
        last_cmd_id: 0,
        last_seq: 0,
    }
}

/// Logged in, attached, quiet.
fn playing() -> (Net, Token) {
    let mut n = Net::start();
    n.srv.plan = Some(plan());
    n.connect();
    n.login();
    let token = n.core.token().expect("token");
    n.advance(100);
    assert_eq!(n.core.conn(), ConnState::Online);
    (n, token)
}

fn pings(n: &Net) -> usize {
    n.srv.sent(|m| matches!(m, ClientMsg::Ping { .. }))
}

fn first_state(n: &Net, s: ConnState, after: u64) -> Option<u64> {
    n.states
        .iter()
        .find(|(t, x)| *x == s && *t >= after)
        .map(|(t, _)| *t)
}

// ---------------------------------------------------------------- Poor and Lost

#[test]
fn poor_at_4_s_and_lost_at_10_s_of_silence_in_a_visible_tab() {
    let (mut n, _) = playing();
    // Let a ping round trip happen so the last byte is recent and exact.
    n.advance(2_500);
    let t0 = n.now;
    n.mute = true;
    // The last byte was at most one ping period ago; measure from the last Pong instead.
    let last_byte = n.now;
    n.advance(15_000);
    let poor = first_state(&n, ConnState::Poor, t0).expect("Poor");
    let lost = first_state(&n, ConnState::Lost, t0).expect("Lost");
    assert!(
        poor - last_byte >= 4_000 - 2_000 && poor - last_byte <= 4_100,
        "poor after {}",
        poor - last_byte
    );
    assert!(
        lost - poor >= 5_900 && lost - poor <= 6_100 + 2_000,
        "lost {} after poor",
        lost - poor
    );
    assert!(
        lost - last_byte <= 10_100,
        "lost after {} ms of silence",
        lost - last_byte
    );
    // The line is closed and reconnecting begins at once.
    assert!(n.closes.iter().any(|(t, _)| *t == lost));
    assert!(matches!(
        n.core.conn(),
        ConnState::Lost | ConnState::Reconnecting
    ));
}

#[test]
fn exact_thresholds_with_a_server_that_answers_until_a_known_moment() {
    let (mut n, _) = playing();
    // Silence starts exactly now: the double answered everything up to this tick.
    n.advance(10);
    let t0 = n.now;
    n.mute = true;
    // The last frame came at or before t0; the ping in flight (if any) is pending.
    let mut poor_at = None;
    let mut lost_at = None;
    while n.now < t0 + 12_000 {
        n.advance(10);
        if poor_at.is_none() && n.core.conn() == ConnState::Poor {
            poor_at = Some(n.now);
        }
        if lost_at.is_none() && matches!(n.core.conn(), ConnState::Lost | ConnState::Reconnecting) {
            lost_at = Some(n.now);
        }
    }
    let (poor, lost) = (poor_at.unwrap() - t0, lost_at.unwrap() - t0);
    assert!(
        (3_900..=4_100).contains(&poor),
        "Poor after {poor} ms of silence"
    );
    assert!(
        (9_900..=10_100).contains(&lost),
        "Lost after {lost} ms of silence"
    );
}

#[test]
fn poor_goes_back_to_online_with_the_first_byte() {
    let (mut n, _) = playing();
    n.mute = true;
    n.advance(5_000);
    assert_eq!(n.core.conn(), ConnState::Poor);
    n.mute = false;
    n.advance(2_500);
    assert_eq!(n.core.conn(), ConnState::Online);
    // A Poor line still takes lobby messages: its socket is open.
    assert!(n.core.orders_enabled());
}

#[test]
fn hidden_tab_gets_no_verdict_and_no_ping() {
    let (mut n, _) = playing();
    n.host_event(HostEvent::Hidden);
    let sent = pings(&n);
    n.mute = true;
    n.advance(60_000);
    assert_eq!(
        n.core.conn(),
        ConnState::Online,
        "no Poor and no Lost while hidden"
    );
    assert_eq!(pings(&n), sent, "no ping while hidden");
}

#[test]
fn visible_after_a_long_hiding_pings_at_once_and_loses_the_line_in_3_s() {
    let (mut n, _) = playing();
    n.host_event(HostEvent::Hidden);
    n.mute = true;
    n.advance(60_000);
    let sent = pings(&n);
    n.host_event(HostEvent::Visible);
    n.advance(10);
    assert_eq!(pings(&n), sent + 1, "a ping on return");
    let t = n.now;
    n.advance(2_900);
    assert_eq!(n.core.conn(), ConnState::Online, "3 s to answer");
    n.advance(300);
    assert!(
        matches!(n.core.conn(), ConnState::Lost | ConnState::Reconnecting),
        "{:?}",
        n.core.conn()
    );
    let lost = first_state(&n, ConnState::Lost, t).expect("Lost");
    assert!((2_900..=3_300).contains(&(lost - t)), "{}", lost - t);
}

#[test]
fn visible_after_a_long_hiding_with_a_live_server_stays_online() {
    let (mut n, _) = playing();
    n.host_event(HostEvent::Hidden);
    n.advance(60_000);
    n.host_event(HostEvent::Visible);
    n.advance(5_000);
    assert_eq!(n.core.conn(), ConnState::Online);
}

#[test]
fn a_short_hiding_pings_but_waits_for_the_ordinary_timeouts() {
    let (mut n, _) = playing();
    n.host_event(HostEvent::Hidden);
    n.advance(3_000);
    n.mute = true;
    let sent = pings(&n);
    n.host_event(HostEvent::Visible);
    n.advance(10);
    assert_eq!(pings(&n), sent + 1);
    n.advance(5_000);
    assert_eq!(n.core.conn(), ConnState::Poor);
}

// ---------------------------------------------------------------- reconnect pacing

#[test]
fn backoff_doubles_with_jitter_after_fast_refusals() {
    let (mut n, token) = playing();
    n.refuse_when_down = true;
    n.outage_start();
    // The server restarts: it closes the socket, then refuses connections.
    n.drop_sock(n.srv.sock_of(token).unwrap(), DisconnectWhy::Remote(1012));
    n.advance(120_000);
    let times: Vec<u64> = n.opens.iter().skip(1).map(|(t, _, _)| *t).collect();
    assert!(times.len() >= 8, "{times:?}");
    let gaps: Vec<u64> = times.windows(2).map(|w| w[1] - w[0]).collect();
    // The first attempt is immediate, then 500, 1000, 2000, 4000, 8000 and 10 s, each ±20 %.
    let nominal = [500u64, 1_000, 2_000, 4_000, 8_000, 10_000, 10_000];
    for (i, (g, nom)) in gaps.iter().skip(1).zip(nominal).enumerate() {
        let (lo, hi) = (nom * 80 / 100, (nom * 120 / 100).min(10_000));
        assert!(
            (lo..=hi + 20).contains(g),
            "gap {i}: {g} not in {lo}..={hi} (gaps {gaps:?})"
        );
    }
    // The status line of the grey panel counts the attempts.
    let st = n.core.reconnect_status().expect("reconnecting");
    assert!(st.attempt >= 8 && !st.probing, "{st:?}");
    assert!(st.next_in_ms <= 10_000);
}

#[test]
fn the_online_event_skips_the_pause_and_resets_the_backoff() {
    let (mut n, token) = playing();
    n.refuse_when_down = true;
    n.outage_start();
    n.drop_sock(n.srv.sock_of(token).unwrap(), DisconnectWhy::Remote(1012));
    n.advance(20_000);
    let st = n.core.reconnect_status().unwrap();
    assert!(!st.probing);
    let opens = n.opens.len();
    // The network is back: no waiting for the pause.
    n.outage_end();
    n.refuse_when_down = false;
    n.host_event(HostEvent::Online);
    n.advance(50);
    assert_eq!(n.opens.len(), opens + 1, "a socket at once");
    n.advance(2_000);
    assert_eq!(n.core.conn(), ConnState::Online);
    assert!(n.has_event(|e| matches!(e, ClientEvent::Attached { .. })));
}

#[test]
fn offline_pauses_the_attempts_and_online_resumes_them() {
    let (mut n, _) = playing();
    n.host_event(HostEvent::Offline);
    assert_eq!(n.core.conn(), ConnState::Offline);
    assert!(!n.core.orders_enabled());
    assert_eq!(
        n.core.chat(ChatTo::All, "x"),
        Err(OrderRefused::NoConnection)
    );
    assert!(n.core.reconnect_status().is_some());
    // The socket was dropped by the network meanwhile.
    let sock = n.open_socks()[0];
    n.drop_sock(sock, DisconnectWhy::Error);
    let opens = n.opens.len();
    n.advance(4_900);
    assert_eq!(
        n.opens.len(),
        opens,
        "no attempt while the browser says offline"
    );
    // `onLine` can lie: one check attempt every 5 s.
    n.advance(300);
    assert_eq!(n.opens.len(), opens + 1);
    assert_eq!(
        n.core.conn(),
        ConnState::Online,
        "the check attempt got through and logged in"
    );
}

#[test]
fn page_hide_closes_the_socket_and_nothing_reopens_until_visible() {
    let (mut n, _) = playing();
    let opens = n.opens.len();
    n.host_event(HostEvent::PageHide);
    assert!(!n.closes.is_empty());
    n.advance(60_000);
    assert_eq!(n.opens.len(), opens);
    n.host_event(HostEvent::Visible);
    n.advance(1_000);
    assert_eq!(n.opens.len(), opens + 1);
    assert_eq!(n.core.conn(), ConnState::Online);
}

#[test]
fn a_silent_server_at_the_handshake_is_a_loss_after_10_s() {
    let mut n = Net::start();
    n.mute = true;
    n.connect();
    n.advance(9_900);
    assert!(n.opens.len() == 1);
    n.advance(1_500);
    assert!(
        n.opens.len() >= 2,
        "the handshake timed out and a new attempt started"
    );
    assert_eq!(n.core.phase(), ClientPhase::Connecting);
}

#[test]
fn clock_offset_and_rtt_come_from_pongs() {
    let mut n = Net::start();
    n.one_way = 100;
    n.srv.plan = Some(plan());
    n.connect();
    n.advance(3_000);
    n.login();
    n.advance(6_000);
    assert_eq!(n.core.conn(), ConnState::Online);
    assert_eq!(n.core.rtt_ms(), Some(200));
    let now = n.now;
    assert_eq!(n.core.server_ms(now), now + 5_000_000);
}

// ---------------------------------------------------------------- outage (NC-D17)

/// Live probe sockets at time `t`: opened (asked) and not yet closed or promoted.
fn live_probes(n: &Net, t: u64, promoted: &[Sock]) -> usize {
    n.opens
        .iter()
        .filter(|(o, s, probe)| {
            *probe
                && *o <= t
                && !promoted.contains(s)
                && !n.closes.iter().any(|(c, cs)| cs == s && *c <= t)
        })
        .count()
}

#[test]
fn a_10_second_outage_is_healed_in_2_s_by_fresh_sockets() {
    let (mut n, token) = playing();
    n.one_way = 100; // RTT 200
    n.advance(500);
    let old = n.srv.sock_of(token).unwrap();
    n.srv.plan.as_mut().unwrap().last_seq = 0;
    n.take_events();
    n.opens.clear();
    n.closes.clear();
    n.states.clear();

    let t0 = n.now;
    n.outage_start();
    n.advance(10_000);
    // Probes were opened from the Poor moment on, at 500 ms steps.
    let probes: Vec<(u64, Sock)> = n
        .opens
        .iter()
        .filter(|(_, _, p)| *p)
        .map(|(t, s, _)| (*t, *s))
        .collect();
    assert!(probes.len() >= 8, "{probes:?}");
    // Silence counts from the last byte (at most one ping period before the outage), not from the outage.
    let poor = first_state(&n, ConnState::Poor, t0).expect("Poor");
    assert!(
        probes[0].0 >= poor && probes[0].0 - poor <= 60,
        "probes start with Poor ({poor}), first at {}",
        probes[0].0
    );
    assert!(
        (2_000..=4_100).contains(&(poor - t0)),
        "Poor {} ms after the outage began",
        poor - t0
    );
    for w in probes.windows(2) {
        let gap = w[1].0 - w[0].0;
        assert!((490..=640).contains(&gap), "probe gap {gap}");
    }
    // Never more than two alive; each is closed 1 s after its start, as it did not open.
    for t in (t0..t0 + 10_000).step_by(10) {
        assert!(live_probes(&n, t, &[]) <= 2, "more than two probes at {t}");
    }
    for (t, s) in &probes {
        if let Some((c, _)) = n.closes.iter().find(|(_, cs)| cs == s) {
            assert!(
                (1_000..=1_060).contains(&(c - t)),
                "probe closed after {}",
                c - t
            );
        }
    }
    // The old socket was given up at 10 s of silence.
    assert!(matches!(
        n.core.conn(),
        ConnState::Lost | ConnState::Reconnecting
    ));
    let st = n.core.reconnect_status().unwrap();
    assert!(
        st.probing && st.attempt >= 8 && st.next_in_ms <= 500,
        "{st:?}"
    );
    assert!(!n.core.orders_enabled());
    assert_eq!(
        n.core.chat(ChatTo::All, "x"),
        Err(OrderRefused::NoConnection)
    );

    // The line is back. The view is current again within 2 s.
    let back = n.now;
    n.outage_end();
    n.take_events();
    let healed = n.advance_until(2_000, |n| {
        n.has_event(|e| matches!(e, ClientEvent::Attached { .. }))
    });
    assert!(healed, "not healed within 2 s of the line coming back");
    assert!(n.now - back <= 2_000);
    assert!(n.has_event(|e| matches!(
        e,
        ClientEvent::Attached {
            mode: AttachMode::Replay,
            ..
        }
    )));
    assert_eq!(n.core.conn(), ConnState::Online);
    assert!(n.core.orders_enabled());
    // The new socket logged in with the token and the stream position, without a password.
    let (s, login) = n.srv.logins.last().unwrap();
    assert_ne!(*s, old);
    assert_eq!(login.token, Some(token));
    assert_eq!(login.password, "");
    assert_eq!(login.resume.map(|r| (r.stream, r.last_seq)), Some((3, 0)));
}

#[test]
fn a_probe_that_gets_through_replaces_the_stuck_old_socket() {
    let (mut n, token) = playing();
    n.one_way = 100;
    n.advance(500);
    let old = n.srv.sock_of(token).unwrap();
    n.outage_start();
    // The outage is shorter than the Lost threshold: the old socket is stuck but not given up yet.
    n.advance(5_000);
    assert_eq!(n.core.conn(), ConnState::Poor);
    n.outage_end();
    let ok = n.advance_until(2_500, |n| n.core.conn() == ConnState::Online);
    assert!(ok, "{:?}", n.core.conn());
    let cur = n.srv.sock_of(token).unwrap();
    assert_ne!(cur, old, "the probe is the current socket now");
    assert!(
        n.closes.iter().any(|(_, s)| *s == old),
        "the stuck socket was closed"
    );
    // The server told the old socket it was replaced; it never arrived (stuck). Even if it did, a Replaced
    // from a socket that is no longer current is ignored:
    n.core.on_bytes(
        n.now,
        old,
        &frame(&ServerMsg::Detached {
            reason: DetachReason::Replaced,
        }),
    );
    n.settle();
    assert_eq!(n.core.conn(), ConnState::Online);
    assert!(!n.has_event(|e| matches!(e, ClientEvent::Detached(DetachReason::Replaced))));
}

#[test]
fn an_old_socket_that_revives_first_closes_the_probes() {
    let (mut n, _) = playing();
    n.one_way = 100;
    n.advance(500);
    // Silence, not an outage: the server just does not talk for 5 s.
    n.mute = true;
    n.advance(5_000);
    assert_eq!(n.core.conn(), ConnState::Poor);
    assert!(n.opens.iter().any(|(_, _, p)| *p), "probes are out");
    n.mute = false;
    let ok = n.advance_until(3_000, |n| n.core.conn() == ConnState::Online);
    assert!(ok);
    // Probes that were still connecting are gone at once; those whose login was already sent get their
    // 10 s to finish (this double never answers them) and are then closed. None replaces the old socket.
    n.advance(11_000);
    let open = n.open_socks();
    assert_eq!(open.len(), 1, "{open:?}");
    assert_eq!(n.core.conn(), ConnState::Online);
}

#[test]
fn after_30_s_of_outage_the_ordinary_backoff_takes_over() {
    let (mut n, _) = playing();
    n.advance(500);
    n.opens.clear();
    let t0 = n.now;
    n.outage_start();
    n.advance(60_000);
    let early = n.opens.iter().filter(|(t, _, _)| *t < t0 + 30_000).count();
    let late = n.opens.iter().filter(|(t, _, _)| *t >= t0 + 31_000).count();
    assert!(early >= 40, "probe cadence: {early}");
    assert!(late <= 4, "ordinary backoff after the window: {late}");
    let st = n.core.reconnect_status().unwrap();
    assert!(!st.probing);
    // And the line coming back is still found by the pause-limited attempts.
    n.outage_end();
    n.host_event(HostEvent::Online);
    n.advance(3_000);
    assert_eq!(n.core.conn(), ConnState::Online);
}

#[test]
fn the_server_full_for_a_probe_does_not_end_the_reconnecting() {
    let (mut n, token) = playing();
    n.advance(500);
    n.srv.full = true;
    n.server_drops(token, 1012);
    n.advance(3_000);
    assert!(
        !matches!(n.core.phase(), ClientPhase::Fatal(_)),
        "a full server turned a probe away, the game is not over"
    );
    n.srv.full = false;
    n.advance(15_000);
    assert_eq!(n.core.conn(), ConnState::Online);
}

#[test]
fn a_replaced_notice_from_the_old_socket_waits_for_the_probe_that_caused_it() {
    // Manual drive: a probe is logging in while the old socket hears `Replaced`.
    let mut c = ClientCore::new(NetConfig::new("b", "alice"));
    let mut t = 0u64;
    c.connect(t);
    let Some(SocketCmd::Open { sock: s1, .. }) = c.poll_socket_cmd() else {
        panic!()
    };
    c.on_connected(t, s1);
    let welcome = frame(&ServerMsg::Welcome {
        session: 1,
        build: "x".into(),
    });
    let logged = |tok: u8| {
        frame(&ServerMsg::LoggedIn(protocol::Session {
            token: Some(Token([tok; 16])),
            nick: "alice".into(),
        }))
    };
    c.on_bytes(t, s1, &welcome);
    c.on_bytes(t, s1, &logged(5));
    // Silence until Poor: the first probe opens.
    let mut probe = None;
    while probe.is_none() {
        t += 10;
        c.tick(t);
        while let Some(cmd) = c.poll_socket_cmd() {
            if let SocketCmd::Open { sock, probe: true } = cmd {
                probe.get_or_insert(sock);
            }
        }
        assert!(t < 20_000);
    }
    let probe = probe.unwrap();
    assert_eq!(c.conn(), ConnState::Poor);
    c.on_connected(t, probe);
    // `Replaced` comes on the old socket first, then the probe's `LoggedIn`.
    let replaced = frame(&ServerMsg::Detached {
        reason: DetachReason::Replaced,
    });
    c.on_bytes(t, s1, &replaced);
    assert_ne!(
        c.conn(),
        ConnState::Replaced,
        "held while the probe logs in"
    );
    c.on_bytes(t + 1, probe, &welcome);
    c.on_bytes(t + 1, probe, &logged(5));
    assert_eq!(c.conn(), ConnState::Online);
    while let Some(ev) = c.poll_event() {
        assert!(
            !matches!(ev, ClientEvent::Detached(DetachReason::Replaced)),
            "{ev:?}"
        );
    }

    // The other way round: the probe fails, so it was somebody else's login. Replaced is final.
    let mut c = ClientCore::new(NetConfig::new("b", "alice"));
    let mut t = 0u64;
    c.connect(t);
    let Some(SocketCmd::Open { sock: s1, .. }) = c.poll_socket_cmd() else {
        panic!()
    };
    c.on_connected(t, s1);
    c.on_bytes(t, s1, &welcome);
    c.on_bytes(t, s1, &logged(5));
    let mut probe = None;
    while probe.is_none() {
        t += 10;
        c.tick(t);
        while let Some(cmd) = c.poll_socket_cmd() {
            if let SocketCmd::Open { sock, probe: true } = cmd {
                probe.get_or_insert(sock);
            }
        }
    }
    let probe = probe.unwrap();
    c.on_connected(t, probe);
    c.on_bytes(t, s1, &replaced);
    assert_ne!(c.conn(), ConnState::Replaced);
    c.on_disconnected(t + 5, probe, DisconnectWhy::Error);
    c.tick(t + 10);
    assert_eq!(c.conn(), ConnState::Replaced);
}

/// A long frame of the client (the city window opening in a debug build on software GL) is not a dead line: the
/// bytes the server sent meanwhile are not read yet when the next tick runs.
#[test]
fn a_stall_of_the_client_is_not_a_silent_line() {
    let (mut n, _) = playing();
    let starts = n.srv.sent(|m| matches!(m, ClientMsg::Login(..)));
    // 15 s without a tick (more than `lost_after_ms`), then the tick alone: the network has delivered nothing yet.
    n.now += 15_000;
    n.core.tick(n.now);
    assert_eq!(n.core.conn(), ConnState::Online, "the stall is ours");
    n.advance(3_000);
    assert_eq!(n.core.conn(), ConnState::Online);
    assert!(n.core.orders_enabled());
    assert_eq!(
        n.srv.sent(|m| matches!(m, ClientMsg::Login(..))),
        starts,
        "no reconnect"
    );
}

#[test]
fn nothing_but_a_lobby_message_leaves_when_the_line_is_down_and_back() {
    // The rule of NC-D5 for every kind of message, at the three down states.
    let (mut n, _) = playing();
    n.host_event(HostEvent::Offline);
    for r in [
        n.core.lobby(LobbyCmd::LeaveRoom),
        n.core.chat(ChatTo::All, "x"),
        n.core.game_ctl(GameCtl::Leave),
    ] {
        assert_eq!(r, Err(OrderRefused::NoConnection));
    }
    n.host_event(HostEvent::Online);
    n.advance(500);
    assert_eq!(n.core.conn(), ConnState::Online);
    assert_eq!(n.core.chat(ChatTo::All, "after"), Ok(()));
    n.settle();
    let chats: Vec<&ClientMsg> = n
        .srv
        .log
        .iter()
        .map(|(_, m)| m)
        .filter(|m| matches!(m, ClientMsg::Chat(_)))
        .collect();
    assert_eq!(
        chats.len(),
        1,
        "only the line sent with a connection exists"
    );
}
