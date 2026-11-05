//! Login, resume, refusals, detach, lobby model, host change (net-client 9) on the scripted server.

mod common;

use client_core::{
    ClientCore, ClientEvent, ClientPhase, ConnState, DisconnectWhy, NetConfig, NoticeKind,
    OrderRefused, Sock, SocketCmd,
};
use common::*;
use protocol::{
    AttachMode, ChatTo, ClientMsg, DetachReason, GameCtl, GameCtlReason, GameMsg, LobbyCmd,
    RejectReason, ServerMsg, Token,
};

fn plan(game: u32, stream: u64) -> GamePlan {
    GamePlan {
        game,
        player: 1,
        stream,
        last_cmd_id: 4,
        last_seq: 0,
    }
}

/// Logged in with the password and attached to a game.
fn playing() -> (Net, Token) {
    let mut n = Net::start();
    n.srv.plan = Some(plan(7, 3));
    n.connect();
    n.login();
    let token = n.core.token().expect("token");
    (n, token)
}

fn seats_batch(seq: u64, host: &str) -> ServerMsg {
    batch(
        seq,
        vec![GameMsg::Seats(vec![
            seat(0, "alice", host == "alice"),
            seat(1, "bob", host == "bob"),
        ])],
    )
}

// ---------------------------------------------------------------- login

#[test]
fn hello_and_login_leave_in_one_output_before_any_reply() {
    let mut c = ClientCore::new(NetConfig::new("build-1", "alice"));
    assert!(c.poll_outgoing().is_none() && c.poll_socket_cmd().is_none());
    c.connect(0);
    let Some(SocketCmd::Open { sock, probe: false }) = c.poll_socket_cmd() else {
        panic!("connect opens the first socket");
    };
    assert!(
        c.poll_outgoing().is_none(),
        "nothing is sent before the socket is open"
    );
    c.on_connected(5, sock);
    let (s1, f1) = c.poll_outgoing().expect("Hello");
    let (s2, f2) = c.poll_outgoing().expect("Login");
    assert!(c.poll_outgoing().is_none());
    assert_eq!((s1, s2), (sock, sock));
    // Plain frames: the client never packs.
    assert_eq!((f1[0], f2[0]), (0, 0));
    assert_eq!(
        client_msg(&f1),
        ClientMsg::Hello {
            protocol: protocol::PROTOCOL_VERSION,
            build: "build-1".into(),
            nick: "alice".into()
        }
    );
    match client_msg(&f2) {
        ClientMsg::Login(l) => {
            assert_eq!((l.password.as_str(), l.token, l.resume), ("", None, None));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_wrong_password_stops_the_reconnecting_and_a_new_login_starts_it_again() {
    let mut n = Net::start();
    n.connect();
    assert!(n.has_event(|e| matches!(e, ClientEvent::Rejected(RejectReason::BadPassword))));
    assert_eq!(n.core.phase(), ClientPhase::NeedLogin);
    assert_eq!(n.core.conn(), ConnState::Closed);
    let opens = n.opens.len();
    n.advance(120_000);
    assert_eq!(n.opens.len(), opens, "no socket after a Reject");
    n.login();
    assert_eq!(n.core.phase(), ClientPhase::Lobby);
    assert_eq!(n.core.conn(), ConnState::Online);
    assert!(n.core.rejected().is_none());
}

#[test]
fn the_token_is_issued_once_and_logs_in_without_a_password_next_time() {
    let (n, token) = playing();
    let issued: Vec<Token> = n
        .events
        .iter()
        .filter_map(|e| match e {
            ClientEvent::TokenIssued(t) => Some(*t),
            _ => None,
        })
        .collect();
    assert_eq!(issued, [token]);

    // A new page: the shell reads the token from the store; there is no password.
    let mut m = Net::start();
    m.srv.tokens = n.srv.tokens.clone();
    m.core.set_token(Some(token));
    m.connect();
    assert_eq!(m.core.phase(), ClientPhase::Lobby);
    let (_, login) = m.srv.logins.last().expect("a login");
    assert_eq!((login.password.as_str(), login.token), ("", Some(token)));
    assert!(login.resume.is_none(), "a fresh page has nothing to resume");
}

#[test]
fn an_unknown_token_logs_in_as_from_scratch() {
    let mut n = Net::start();
    n.core.set_token(Some(Token([200; 16])));
    n.core.login(NICK, PASSWORD);
    n.connect();
    let issued: Vec<Token> = n
        .events
        .iter()
        .filter_map(|e| match e {
            ClientEvent::TokenIssued(t) => Some(*t),
            _ => None,
        })
        .collect();
    assert_eq!(issued.len(), 1);
    assert_ne!(issued[0], Token([200; 16]));
    assert_eq!(n.core.token(), Some(issued[0]));
    assert_eq!(n.core.phase(), ClientPhase::Lobby);
}

#[test]
fn every_reject_reason_ends_the_connection_and_maps_to_a_phase() {
    for (reason, erases_token, expect) in [
        (RejectReason::BadPassword, true, ClientPhase::NeedLogin),
        (RejectReason::BadNick, true, ClientPhase::NeedLogin),
        (RejectReason::NickTaken, true, ClientPhase::NeedLogin),
        (
            RejectReason::VersionMismatch { server: 9 },
            false,
            ClientPhase::Fatal("The server has been updated, reload the page".into()),
        ),
        (
            RejectReason::Full,
            false,
            ClientPhase::Fatal("The server is full".into()),
        ),
        (
            RejectReason::Unsupported { kind: "x".into() },
            false,
            ClientPhase::Fatal("The client and the server are not compatible".into()),
        ),
    ] {
        let mut c = ClientCore::new(NetConfig::new("b", "alice"));
        c.set_token(Some(Token([7; 16])));
        c.connect(0);
        let Some(SocketCmd::Open { sock, .. }) = c.poll_socket_cmd() else {
            panic!()
        };
        c.on_connected(1, sock);
        c.on_bytes(2, sock, &frame(&ServerMsg::Reject(reason.clone())));
        assert_eq!(c.phase(), expect, "{reason:?}");
        assert_eq!(c.conn(), ConnState::Closed);
        assert_eq!(c.token().is_none(), erases_token, "{reason:?}");
        assert_eq!(c.rejected(), Some(&reason));
        // The shell closes the socket; whatever happens, nothing reopens.
        c.on_disconnected(3, sock, DisconnectWhy::Remote(1000));
        for t in (0..120).map(|s| 1_000 * s) {
            c.tick(t);
        }
        // Only closes are allowed after a refusal.
        while let Some(cmd) = c.poll_socket_cmd() {
            assert!(
                !matches!(cmd, SocketCmd::Open { .. }),
                "{reason:?}: {cmd:?}"
            );
        }
        assert!(c.orders_enabled().eq(&false));
        assert_eq!(c.lobby(LobbyCmd::LeaveRoom), Err(OrderRefused::NoGame));
    }
}

#[test]
fn password_and_token_never_reach_debug_output() {
    let (n, token) = playing();
    let mut text = String::new();
    for e in &n.events {
        text += &format!("{e:?}\n");
    }
    for (_, l) in &n.srv.logins {
        text += &format!("{l:?}\n");
    }
    text += &format!("{:?}\n{:?}\n", token, n.core.config());
    text += &format!("{:?}\n", n.core.attach_info());
    assert!(!text.contains(PASSWORD), "{text}");
    let hex: String = token.0.iter().map(|b| format!("{b:02x}")).collect();
    assert!(
        !text.contains(&hex) && !text.contains("[1, 1, 1, 1"),
        "{text}"
    );
    assert!(text.contains("***"), "the redaction is visible");
    // The server did get the password, once, in the one login that needed it.
    assert!(n.srv.logins.iter().any(|(_, l)| l.password == PASSWORD));
}

#[test]
fn nothing_panics_on_garbage_in_any_state() {
    let mut rng = 0x1234_5678_9ABC_DEF1_u64;
    let mut next = || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        rng
    };
    let mut n = Net::start();
    n.srv.plan = Some(plan(7, 3));
    n.connect();
    n.login();
    let sock = n.open_socks()[0];
    for round in 0..2000 {
        let len = (next() % 40) as usize;
        let mut bytes: Vec<u8> = (0..len).map(|_| next() as u8).collect();
        // Half the time a frame that starts like a real one.
        if round % 2 == 0 && !bytes.is_empty() {
            bytes[0] = (next() % 4) as u8;
        }
        n.core.on_bytes(n.now, sock, &bytes);
        n.core.on_bytes(n.now, Sock(9999), &bytes);
    }
    n.settle();
    assert!(n.core.stats().decode_errors > 0);
}

// ---------------------------------------------------------------- attach, resume

#[test]
fn attached_fresh_then_replay_after_a_drop_keeps_the_state() {
    let (mut n, token) = playing();
    assert!(n.has_event(|e| matches!(
        e,
        ClientEvent::Attached {
            game: 7,
            mode: AttachMode::Fresh
        }
    )));
    assert_eq!(
        n.core.phase(),
        ClientPhase::Loading,
        "no keyframe, no catalog yet"
    );
    n.push(token, seats_batch(1, "alice"));
    n.push(token, seats_batch(2, "alice"));
    n.settle();
    assert_eq!(n.core.attach_info().unwrap().last_seq, 2);
    assert_eq!(n.core.seats().len(), 2);
    assert_eq!(n.core.next_cmd_id(), 5);

    n.srv.plan.as_mut().unwrap().last_seq = 2;
    n.take_events();
    n.server_drops(token, 1006);
    n.advance(2_000);

    let (_, login) = n.srv.logins.last().unwrap();
    let r = login
        .resume
        .expect("the stream position goes with the login");
    assert_eq!((r.stream, r.last_seq), (3, 2));
    assert_eq!(login.password, "", "the password was spent");
    assert!(n.has_event(|e| matches!(
        e,
        ClientEvent::Attached {
            game: 7,
            mode: AttachMode::Replay
        }
    )));
    assert_eq!(n.core.seats().len(), 2, "Replay resets nothing");
    assert_eq!(n.core.attach_info().unwrap().last_seq, 2);
    assert_eq!(n.core.conn(), ConnState::Online);
}

#[test]
fn fresh_after_a_gateway_restart_resets_the_stream_state() {
    let (mut n, token) = playing();
    n.push(token, seats_batch(1, "alice"));
    n.settle();
    assert_eq!(n.core.seats().len(), 2);
    // The gateway restarted: another stream number, nothing to replay.
    n.srv.plan = Some(plan(7, 4));
    n.server_drops(token, 1012);
    n.advance(3_000);
    assert!(n.has_event(|e| matches!(
        e,
        ClientEvent::Attached {
            game: 7,
            mode: AttachMode::Fresh
        }
    )));
    assert!(n.core.seats().is_empty());
    let a = n.core.attach_info().unwrap();
    assert_eq!((a.stream, a.last_seq), (4, 0));
}

#[test]
fn another_game_drops_everything_of_the_old_one() {
    let (mut n, token) = playing();
    n.push(token, seats_batch(1, "alice"));
    n.settle();
    assert_eq!(n.core.next_cmd_id(), 5);
    // Back to the lobby, then into game 8 (its order counter is lower).
    n.push(
        token,
        ServerMsg::Detached {
            reason: DetachReason::Left,
        },
    );
    n.settle();
    assert!(n.core.attach_info().is_none());
    assert_eq!(
        n.core.phase(),
        ClientPhase::Lobby,
        "Left goes to the lobby without a note"
    );
    n.srv.plan = Some(GamePlan {
        game: 8,
        player: 0,
        stream: 1,
        last_cmd_id: 1,
        last_seq: 0,
    });
    n.push(
        token,
        ServerMsg::Attached(protocol::Attached {
            game: 8,
            player: 0,
            stream: 1,
            mode: AttachMode::Replay,
            catalog: catalog_ref(),
            last_cmd_id: 1,
        }),
    );
    n.settle();
    let a = n.core.attach_info().unwrap();
    assert_eq!((a.game, a.stream, a.last_seq), (8, 1, 0));
    assert!(n.core.seats().is_empty());
    assert_eq!(n.core.next_cmd_id(), 2);
}

#[test]
fn the_catalog_is_requested_once_per_hash() {
    let (mut n, token) = playing();
    assert_eq!(
        n.count_events(|e| matches!(e, ClientEvent::CatalogNeeded(c) if *c == catalog_ref())),
        1
    );
    n.core.set_catalog(protocol::Catalog::default());
    n.take_events();
    n.server_drops(token, 1006);
    n.advance(2_000);
    assert!(n.has_event(|e| matches!(e, ClientEvent::Attached { .. })));
    assert!(
        !n.has_event(|e| matches!(e, ClientEvent::CatalogNeeded(_))),
        "the catalog of this hash is already loaded"
    );
}

// ---------------------------------------------------------------- detach

#[test]
fn detach_reasons_show_a_note_until_it_is_read() {
    for (reason, note) in [
        (DetachReason::Kicked, Some(NoticeKind::Kicked)),
        (DetachReason::GameOver, Some(NoticeKind::GameOver)),
        (
            DetachReason::GameStopped {
                why: "crash".into(),
            },
            Some(NoticeKind::GameStopped {
                why: "crash".into(),
            }),
        ),
        (DetachReason::Left, None),
    ] {
        let (mut n, token) = playing();
        n.push(
            token,
            ServerMsg::Detached {
                reason: reason.clone(),
            },
        );
        n.push(token, ServerMsg::LobbyState(n.srv.lobby_state()));
        n.settle();
        assert!(n.has_event(|e| *e == ClientEvent::Detached(reason.clone())));
        match note {
            Some(note) => {
                assert_eq!(n.core.phase(), ClientPhase::Notice(note), "{reason:?}");
                // The LobbyState that follows the detach does not hide the note.
                assert!(n.core.lobby_state().is_some());
                n.core.dismiss_notice();
                assert_eq!(n.core.phase(), ClientPhase::Lobby);
            }
            None => assert_eq!(n.core.phase(), ClientPhase::Lobby),
        }
        assert_eq!(n.core.conn(), ConnState::Online, "a detach keeps the line");
    }
}

#[test]
fn replaced_is_final_until_the_player_uses_this_tab() {
    let (mut n, token) = playing();
    let opens = n.opens.len();
    n.push(
        token,
        ServerMsg::Detached {
            reason: DetachReason::Replaced,
        },
    );
    n.settle();
    assert_eq!(n.core.conn(), ConnState::Replaced);
    assert_eq!(n.core.phase(), ClientPhase::Notice(NoticeKind::Replaced));
    n.advance(120_000);
    assert_eq!(n.opens.len(), opens, "no reconnecting after Replaced");
    assert_eq!(n.core.chat(ChatTo::All, "hi"), Err(OrderRefused::NoGame));
    n.core.use_this_tab();
    n.settle();
    n.advance(2_000);
    assert_eq!(n.opens.len(), opens + 1);
    assert_eq!(n.core.conn(), ConnState::Online);
    assert_eq!(n.core.phase(), ClientPhase::Loading);
    assert!(n.core.attach_info().is_some());
}

#[test]
fn the_server_replacing_a_tab_reaches_it_before_the_socket_closes() {
    // Two clients with one token: the second login replaces the first (the double does what the gateway does).
    let (mut a, token) = playing();
    let mut b = Net::start();
    b.srv.tokens = a.srv.tokens.clone();
    b.core.set_token(Some(token));
    b.connect();
    // Hand the second login to the first server: re-create the situation on `a`'s server.
    let sock_b = Sock(77);
    a.srv.recv(
        sock_b,
        ClientMsg::Hello {
            protocol: protocol::PROTOCOL_VERSION,
            build: "x".into(),
            nick: NICK.into(),
        },
    );
    let replies = a.srv.recv(
        sock_b,
        ClientMsg::Login(protocol::Login {
            password: String::new(),
            token: Some(token),
            resume: None,
        }),
    );
    let old = replies
        .iter()
        .find(|(_, m)| matches!(m, ServerMsg::Detached { .. }))
        .expect("the old socket is told");
    a.push_to(old.0, old.1.clone());
    a.settle();
    assert_eq!(a.core.conn(), ConnState::Replaced);
}

#[test]
fn game_ctl_rejection_is_an_event_and_changes_no_state() {
    let (mut n, token) = playing();
    let phase = n.core.phase();
    n.push(
        token,
        ServerMsg::GameCtlRejected {
            ctl: GameCtl::Pause(true),
            reason: GameCtlReason::NotHost,
        },
    );
    n.settle();
    assert!(n.has_event(|e| matches!(
        e,
        ClientEvent::GameCtlRejected {
            ctl: GameCtl::Pause(true),
            reason: GameCtlReason::NotHost
        }
    )));
    assert_eq!(n.core.phase(), phase);
    assert_eq!(n.core.conn(), ConnState::Online);
}

// ---------------------------------------------------------------- lobby model, refusals

#[test]
fn lobby_state_is_replaced_whole_and_commands_go_out() {
    let mut n = Net::start();
    n.connect();
    n.login();
    assert_eq!(n.core.phase(), ClientPhase::Lobby);
    assert!(n.core.choices().is_some());
    let token = n.core.token().unwrap();
    n.srv.room_host = Some("alice".into());
    n.push(token, ServerMsg::LobbyState(n.srv.lobby_state()));
    n.settle();
    assert_eq!(n.core.phase(), ClientPhase::Room);
    n.srv.room_host = None;
    n.push(token, ServerMsg::LobbyState(n.srv.lobby_state()));
    n.settle();
    assert_eq!(n.core.phase(), ClientPhase::Lobby);
    assert!(n.core.lobby_state().unwrap().room.is_none());

    assert_eq!(n.core.lobby(LobbyCmd::Start), Ok(()));
    assert_eq!(n.core.phase(), ClientPhase::Starting);
    n.settle();
    assert_eq!(
        n.srv
            .sent(|m| matches!(m, ClientMsg::Lobby(LobbyCmd::Start))),
        1
    );
    // A refusal ends the wait.
    n.push(token, ServerMsg::LobbyError { text: "no".into() });
    n.settle();
    assert!(n.has_event(|e| matches!(e, ClientEvent::LobbyError(t) if t == "no")));
    assert_eq!(n.core.phase(), ClientPhase::Lobby);
}

#[test]
fn chat_and_game_ctl_need_a_game_and_a_line() {
    let mut n = Net::start();
    n.connect();
    n.login();
    assert_eq!(
        n.core.chat(ChatTo::All, "x"),
        Err(OrderRefused::NoGame),
        "in the lobby"
    );
    assert_eq!(n.core.game_ctl(GameCtl::Leave), Err(OrderRefused::NoGame));

    let (mut n, _) = playing();
    assert!(n.core.orders_enabled());
    let long = "é".repeat(600);
    assert_eq!(n.core.chat(ChatTo::All, &long), Ok(()));
    assert_eq!(n.core.game_ctl(GameCtl::Pause(true)), Ok(()));
    n.settle();
    let chat: Vec<usize> = n
        .srv
        .log
        .iter()
        .filter_map(|(_, m)| match m {
            ClientMsg::Chat(c) => Some(c.text.chars().count()),
            _ => None,
        })
        .collect();
    assert_eq!(chat, [500], "a line is cut at 500 characters");
    assert_eq!(
        n.srv
            .sent(|m| matches!(m, ClientMsg::GameCtl(GameCtl::Pause(true)))),
        1
    );
}

#[test]
fn without_a_connection_nothing_is_accepted_and_nothing_waits() {
    let (mut n, token) = playing();
    n.srv.plan.as_mut().unwrap().last_seq = 0;
    n.outage_start();
    n.advance(11_000);
    assert!(matches!(
        n.core.conn(),
        ConnState::Lost | ConnState::Reconnecting
    ));
    assert!(!n.core.orders_enabled());
    let before = n.core.stats().refused_offline;
    assert_eq!(
        n.core.lobby(LobbyCmd::LeaveRoom),
        Err(OrderRefused::NoConnection)
    );
    assert_eq!(
        n.core.chat(ChatTo::All, "x"),
        Err(OrderRefused::NoConnection)
    );
    assert_eq!(
        n.core.game_ctl(GameCtl::Leave),
        Err(OrderRefused::NoConnection)
    );
    assert_eq!(n.core.stats().refused_offline, before + 3);
    // Nothing was kept for later: after the line comes back, none of them is sent.
    n.outage_end();
    n.advance(5_000);
    assert_eq!(n.core.conn(), ConnState::Online);
    assert_eq!(
        n.srv
            .sent(|m| matches!(m, ClientMsg::Chat(_) | ClientMsg::GameCtl(_))),
        0
    );
    assert_eq!(
        n.srv
            .sent(|m| matches!(m, ClientMsg::Lobby(LobbyCmd::LeaveRoom))),
        0
    );
    let _ = token;
    // On a Poor line the socket is open, so messages go out.
}

// ---------------------------------------------------------------- host change (NP-D25)

#[test]
fn host_change_in_a_game_is_seen_between_neighbouring_seat_lists() {
    let (mut n, token) = playing();
    n.push(token, seats_batch(1, "alice"));
    n.settle();
    assert!(
        !n.has_event(|e| matches!(e, ClientEvent::HostChanged { .. })),
        "the first snapshot after Fresh only sets the baseline"
    );
    n.push(token, seats_batch(2, "alice"));
    n.settle();
    assert_eq!(n.core.stats().host_changes, 0);
    // Alice drops, Bob is host now (60 s later); Alice comes back as a plain player.
    n.push(token, seats_batch(3, "bob"));
    n.settle();
    let ev: Vec<&ClientEvent> = n
        .events
        .iter()
        .filter(|e| matches!(e, ClientEvent::HostChanged { .. }))
        .collect();
    assert_eq!(
        ev,
        [&ClientEvent::HostChanged {
            from: Some("alice".into()),
            to: "bob".into(),
            me: false
        }]
    );
    n.push(token, seats_batch(4, "bob"));
    n.settle();
    assert_eq!(
        n.core.stats().host_changes,
        1,
        "the returning host is a plain player"
    );
    // I become host.
    n.push(token, seats_batch(5, "alice"));
    n.settle();
    assert!(n.has_event(|e| matches!(
        e,
        ClientEvent::HostChanged { to, me: true, .. } if to == "alice"
    )));
    assert_eq!(n.core.stats().host_changes, 2);
}

#[test]
fn host_change_in_a_room_and_a_fresh_attach_resets_the_baseline() {
    let mut n = Net::start();
    n.connect();
    n.login();
    let token = n.core.token().unwrap();
    n.srv.room_host = Some("bob".into());
    n.push(token, ServerMsg::LobbyState(n.srv.lobby_state()));
    n.settle();
    assert!(!n.has_event(|e| matches!(e, ClientEvent::HostChanged { .. })));
    n.srv.room_host = Some("alice".into());
    n.push(token, ServerMsg::LobbyState(n.srv.lobby_state()));
    n.settle();
    assert!(n.has_event(|e| matches!(
        e,
        ClientEvent::HostChanged { from: Some(f), to, me: true } if f == "bob" && to == "alice"
    )));

    // In a game: the first snapshot after a Fresh attach is a baseline whoever the host is.
    n.srv.plan = Some(plan(7, 3));
    n.push(
        token,
        ServerMsg::Attached(protocol::Attached {
            game: 7,
            player: 0,
            stream: 3,
            mode: AttachMode::Fresh,
            catalog: catalog_ref(),
            last_cmd_id: 0,
        }),
    );
    n.push(token, seats_batch(1, "bob"));
    n.settle();
    assert_eq!(n.core.stats().host_changes, 1);
    n.push(
        token,
        ServerMsg::Attached(protocol::Attached {
            game: 7,
            player: 0,
            stream: 3,
            mode: AttachMode::Fresh,
            catalog: catalog_ref(),
            last_cmd_id: 0,
        }),
    );
    n.push(token, seats_batch(1, "alice"));
    n.settle();
    assert_eq!(
        n.core.stats().host_changes,
        1,
        "Fresh starts a new baseline: bob to alice across a Fresh is not a change"
    );
}

#[test]
fn duplicate_batches_are_ignored_and_the_position_only_grows() {
    let (mut n, token) = playing();
    n.push(token, seats_batch(1, "alice"));
    n.push(token, seats_batch(1, "bob"));
    n.push(token, seats_batch(2, "alice"));
    n.settle();
    assert_eq!(n.core.attach_info().unwrap().last_seq, 2);
    assert_eq!(
        n.core.stats().host_changes,
        0,
        "the duplicate carried a different host and was dropped"
    );
}

// ---------------------------------------------------------------- sign out

#[test]
fn sign_out_shows_the_login_window_even_on_an_open_server_and_a_new_login_works() {
    let (mut n, _token) = playing();
    assert!(matches!(
        n.core.phase(),
        ClientPhase::Playing | ClientPhase::Loading
    ));
    // A server open to guests would let the nick straight back in: the empty password is the right one.
    n.srv.password = String::new();
    // The shell: the core forgets, every socket is dropped as `NetClient::sign_out` does.
    n.core.sign_out();
    for sock in n.open_socks() {
        n.drop_sock(sock, DisconnectWhy::Local);
    }
    let opens = n.opens.len();
    n.advance(120_000);
    assert_eq!(n.opens.len(), opens, "nothing reconnects by itself");
    assert_eq!(n.core.phase(), ClientPhase::NeedLogin);
    assert!(n.core.token().is_none());
    // The player types the nick and a password: the connection starts from the beginning.
    n.core.login(NICK, "");
    n.settle();
    assert!(matches!(
        n.core.phase(),
        ClientPhase::Lobby | ClientPhase::Playing | ClientPhase::Loading
    ));
    assert!(n.core.token().is_some());
}
