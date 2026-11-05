//! The login and attachment state of a `ClientCore` (net-client 4.2): `Hello` + `Login` in one output,
//! the token, `resume`, `Attached`, `Detached`, `Reject`.
//!
//! Pure state: no sockets and no clock. `ClientCore` feeds it server messages and sends what it builds.
//! Password and token never reach `Debug` or the log: `Login`, `Token` and `Session` print `***`, and this
//! type has no `Debug` that could print them.

use protocol::{
    AttachMode, Attached, BasisClaim, CatalogRef, ClientMsg, CmdId, DetachReason, GameId, Login,
    PROTOCOL_VERSION, PlayerId, RejectReason, Resume, Token,
};

/// Where the session is. The connection (`ConnState`) is a separate matter: an attached game stays
/// `Attached` while the line is down and the client reconnects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Phase {
    /// Before the first connect.
    Idle,
    /// Connecting or reconnecting, no `Welcome` on the current line yet.
    Connecting,
    /// `Hello` and `Login` sent, waiting for `LoggedIn`.
    Handshake,
    LoggedIn,
    Attached,
    Rejected,
}

/// Why the player is looking at a message instead of the game.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoticeKind {
    Kicked,
    /// The same token logged in elsewhere (NC-D15).
    Replaced,
    GameStopped {
        why: String,
    },
    GameOver,
}

/// What `Attached` told and how far the stream has come.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachInfo {
    pub game: GameId,
    pub player: PlayerId,
    /// Number of the stream; changes when the gateway restarts.
    pub stream: u64,
    pub mode: AttachMode,
    pub catalog: CatalogRef,
    /// The server's last order number at attach time.
    pub last_cmd_id: CmdId,
    /// `seq` of the last batch received on this stream (0: none yet).
    pub last_seq: u64,
}

/// How an `Attached` changed things, for the core to reset what belongs to the game.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AttachOutcome {
    /// Another game than before (or the first one): everything of the game is dropped, including the map
    /// basis and the orders in flight.
    pub new_game: bool,
    /// `Fresh`: base, contexts, journal, chat, turn state and seats start over.
    pub fresh: bool,
}

pub(crate) struct SessionState {
    nick: String,
    /// Typed on this page and not yet used: the password is never kept after `LoggedIn`.
    password: Option<String>,
    token: Option<Token>,
    phase: Phase,
    rejected: Option<RejectReason>,
    attach: Option<AttachInfo>,
    /// The game of the last `Attached`; kept across a `Detached` to tell a new game from the same one.
    prev_game: Option<GameId>,
    /// The map basis the client holds (NP-D22); the stream (NC2) keeps it up to date.
    basis_claim: Option<BasisClaim>,
    /// Number of the next order (NC3 uses it); starts at `last_cmd_id + 1`.
    next_cmd_id: CmdId,
    notice: Option<NoticeKind>,
    /// `Start`, `JoinGame` or `LoadSave` sent, `Attached` not yet.
    starting: bool,
    fatal: Option<String>,
    /// The ladder gave up `resume`: the next login asks for a fresh stream.
    no_resume: bool,
}

impl SessionState {
    pub fn new(nick: &str) -> Self {
        SessionState {
            nick: nick.to_string(),
            password: None,
            token: None,
            phase: Phase::Idle,
            rejected: None,
            attach: None,
            prev_game: None,
            basis_claim: None,
            next_cmd_id: 1,
            notice: None,
            starting: false,
            fatal: None,
            no_resume: false,
        }
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    pub fn nick(&self) -> &str {
        &self.nick
    }

    pub fn token(&self) -> Option<Token> {
        self.token
    }

    pub fn rejected(&self) -> Option<&RejectReason> {
        self.rejected.as_ref()
    }

    pub fn attach(&self) -> Option<&AttachInfo> {
        self.attach.as_ref()
    }

    pub fn next_cmd_id(&self) -> CmdId {
        self.next_cmd_id
    }

    /// Hands out the number of an order and moves the counter (NC3).
    pub fn take_cmd_id(&mut self) -> CmdId {
        let id = self.next_cmd_id;
        self.next_cmd_id = id.saturating_add(1);
        id
    }

    pub fn basis_claim(&self) -> Option<BasisClaim> {
        self.basis_claim
    }

    pub fn set_basis_claim(&mut self, claim: Option<BasisClaim>) {
        self.basis_claim = claim;
    }

    pub fn notice(&self) -> Option<&NoticeKind> {
        self.notice.as_ref()
    }

    pub fn take_notice(&mut self) -> Option<NoticeKind> {
        self.notice.take()
    }

    pub fn starting(&self) -> bool {
        self.starting
    }

    pub fn set_starting(&mut self, v: bool) {
        self.starting = v;
    }

    pub fn fatal(&self) -> Option<&str> {
        self.fatal.as_deref()
    }

    pub fn set_fatal(&mut self, text: String) {
        self.fatal = Some(text);
    }

    /// The next login names no stream to resume (the second step of the ladder, 4.3).
    pub fn forget_resume(&mut self) {
        self.no_resume = true;
    }

    // ------------------------------------------------------------ credentials

    /// The player typed a nick and a password. A new login replaces an earlier refusal.
    pub fn set_credentials(&mut self, nick: &str, password: &str) {
        self.nick = nick.to_string();
        self.password = Some(password.to_string());
        self.rejected = None;
        if self.phase == Phase::Rejected {
            self.phase = Phase::Idle;
        }
    }

    /// The token of an earlier session, from the browser store.
    pub fn set_token(&mut self, token: Option<Token>) {
        self.token = token;
    }

    /// "Sign out": the session is forgotten and the login window is up (`NeedLogin`, as after a refusal): nothing
    /// reconnects until the player types a password. Before, the phase stayed `LoggedIn` or `Attached`, the
    /// socket came back by itself, the new `Welcome` was "not expected" and the screen never left the lobby
    /// (on a server open to guests the nick was let straight back in). The nick stays to prefill the window.
    pub fn sign_out(&mut self) {
        self.token = None;
        self.password = None;
        self.attach = None;
        self.prev_game = None;
        self.basis_claim = None;
        self.notice = None;
        self.starting = false;
        self.fatal = None;
        self.no_resume = false;
        self.rejected = Some(RejectReason::BadPassword);
        self.phase = Phase::Rejected;
    }

    pub fn retry(&mut self) {
        self.rejected = None;
        self.fatal = None;
        if self.phase == Phase::Rejected {
            self.phase = Phase::Idle;
        }
    }

    // ------------------------------------------------------------ building

    /// `Hello` and `Login`, one after the other with no reply in between (one round trip). The password is
    /// empty when the player did not type one on this page; the gateway takes a known token without it.
    pub fn hello_login(&self, build: &str) -> [ClientMsg; 2] {
        let resume = self
            .attach
            .as_ref()
            .filter(|_| !self.no_resume)
            .map(|a| Resume {
                stream: a.stream,
                last_seq: a.last_seq,
                basis: self.basis_claim,
            });
        [
            ClientMsg::Hello {
                protocol: PROTOCOL_VERSION,
                build: build.to_string(),
                nick: self.nick.clone(),
            },
            ClientMsg::Login(Login {
                password: self.password.clone().unwrap_or_default(),
                token: self.token,
                resume,
            }),
        ]
    }

    pub fn begin_handshake(&mut self) {
        if matches!(self.phase, Phase::Idle | Phase::Connecting) {
            self.phase = Phase::Handshake;
        }
    }

    /// The line dropped: a session that was playing stays in its phase, one still logging in starts over.
    pub fn line_down(&mut self) {
        if matches!(self.phase, Phase::Handshake | Phase::Idle) {
            self.phase = Phase::Connecting;
        }
    }

    // ------------------------------------------------------------ server messages

    /// `LoggedIn`. The password is spent.
    pub fn logged_in(&mut self, token: Option<Token>, nick: String) {
        // `None`: the web page's cookie login, the connection carries no token and none is kept (v9).
        self.token = token;
        self.nick = nick;
        self.password = None;
        self.rejected = None;
        if self.phase != Phase::Attached {
            self.phase = Phase::LoggedIn;
        }
    }

    /// `Attached`: `Fresh` starts the stream over, `Replay` continues it, another game drops everything.
    pub fn attached(&mut self, a: Attached) -> AttachOutcome {
        let new_game = self.prev_game != Some(a.game);
        let fresh = new_game || a.mode == AttachMode::Fresh;
        if new_game {
            self.basis_claim = None;
            self.next_cmd_id = a.last_cmd_id.saturating_add(1);
        } else {
            self.next_cmd_id = self.next_cmd_id.max(a.last_cmd_id.saturating_add(1));
        }
        let last_seq = match (&self.attach, fresh) {
            (Some(old), false) if old.stream == a.stream => old.last_seq,
            _ => 0,
        };
        self.prev_game = Some(a.game);
        self.attach = Some(AttachInfo {
            game: a.game,
            player: a.player,
            stream: a.stream,
            mode: if new_game { AttachMode::Fresh } else { a.mode },
            catalog: a.catalog,
            last_cmd_id: a.last_cmd_id,
            last_seq,
        });
        self.phase = Phase::Attached;
        self.starting = false;
        self.notice = None;
        self.no_resume = false;
        AttachOutcome { new_game, fresh }
    }

    /// `Detached`: back to the lobby; nothing to resume.
    pub fn detached(&mut self, reason: &DetachReason) {
        self.attach = None;
        self.starting = false;
        if self.phase == Phase::Attached {
            self.phase = Phase::LoggedIn;
        }
        self.notice = match reason {
            DetachReason::Kicked => Some(NoticeKind::Kicked),
            DetachReason::GameOver => Some(NoticeKind::GameOver),
            DetachReason::GameStopped { why } => Some(NoticeKind::GameStopped { why: why.clone() }),
            DetachReason::Replaced => Some(NoticeKind::Replaced),
            // `Left` goes straight to the lobby without a message.
            DetachReason::Left => None,
        };
    }

    /// `Reject`: nothing reconnects after it. A wrong password, nick or taken nick erases the token.
    pub fn rejected_with(&mut self, reason: RejectReason) {
        if matches!(
            reason,
            RejectReason::BadPassword | RejectReason::BadNick | RejectReason::NickTaken
        ) {
            self.token = None;
        }
        self.password = None;
        self.phase = Phase::Rejected;
        self.rejected = Some(reason);
    }

    /// The stream moved to `seq` (the last batch received).
    pub fn note_seq(&mut self, seq: u64) {
        if let Some(a) = &mut self.attach {
            a.last_seq = seq;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token(b: u8) -> Token {
        Token([b; 16])
    }

    fn attached(game: GameId, stream: u64, mode: AttachMode, last_cmd: CmdId) -> Attached {
        Attached {
            game,
            player: 1,
            stream,
            mode,
            catalog: CatalogRef { hash: 5, len: 9 },
            last_cmd_id: last_cmd,
        }
    }

    fn login(s: &SessionState) -> Login {
        match s.hello_login("b") {
            [ClientMsg::Hello { .. }, ClientMsg::Login(l)] => l,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn password_is_empty_unless_typed_and_spent_at_login() {
        let mut s = SessionState::new("alice");
        assert_eq!(login(&s).password, "");
        s.set_credentials("alice", "hunter2");
        assert_eq!(login(&s).password, "hunter2");
        s.logged_in(Some(token(1)), "alice".into());
        assert_eq!(login(&s).password, "");
        assert_eq!(login(&s).token, Some(token(1)));
    }

    #[test]
    fn a_cookie_login_leaves_no_token_and_the_next_login_sends_none() {
        let mut s = SessionState::new("alice");
        s.set_credentials("alice", "hunter2");
        s.logged_in(None, "Alice".into());
        assert_eq!((s.token(), s.nick()), (None, "Alice"));
        // The reconnect carries neither token nor password: the socket's cookie does the login.
        let l = login(&s);
        assert_eq!((l.token, l.password.as_str()), (None, ""));
        // A token login after it (a native client) is remembered again.
        s.logged_in(Some(token(3)), "Alice".into());
        assert_eq!(login(&s).token, Some(token(3)));
    }

    #[test]
    fn resume_names_the_stream_and_basis_only_while_attached() {
        let mut s = SessionState::new("alice");
        assert_eq!(login(&s).resume, None);
        s.logged_in(Some(token(1)), "alice".into());
        s.attached(attached(7, 3, AttachMode::Fresh, 0));
        s.set_basis_claim(Some(BasisClaim { game: 7, hash: 99 }));
        s.note_seq(12);
        assert_eq!(
            login(&s).resume,
            Some(Resume {
                stream: 3,
                last_seq: 12,
                basis: Some(BasisClaim { game: 7, hash: 99 })
            })
        );
        s.detached(&DetachReason::Left);
        assert_eq!(login(&s).resume, None);
    }

    #[test]
    fn replay_keeps_the_position_fresh_and_a_new_stream_start_over() {
        let mut s = SessionState::new("a");
        s.logged_in(Some(token(1)), "a".into());
        let o = s.attached(attached(7, 3, AttachMode::Fresh, 4));
        assert_eq!((o.new_game, o.fresh), (true, true));
        s.note_seq(20);
        let o = s.attached(attached(7, 3, AttachMode::Replay, 4));
        assert_eq!((o.new_game, o.fresh), (false, false));
        assert_eq!(s.attach().unwrap().last_seq, 20);
        let o = s.attached(attached(7, 3, AttachMode::Fresh, 4));
        assert_eq!((o.new_game, o.fresh), (false, true));
        assert_eq!(s.attach().unwrap().last_seq, 0);
        // The gateway restarted: another stream number, whatever the mode says.
        s.note_seq(5);
        s.attached(attached(7, 4, AttachMode::Replay, 4));
        assert_eq!(s.attach().unwrap().last_seq, 0);
    }

    #[test]
    fn command_numbers_continue_from_the_server_and_never_go_back() {
        let mut s = SessionState::new("a");
        s.logged_in(Some(token(1)), "a".into());
        s.attached(attached(7, 3, AttachMode::Fresh, 41));
        assert_eq!(s.next_cmd_id(), 42);
        // The client used more numbers than the server saw (orders lost with the page are not our case,
        // but a counter ahead of the server must not shrink).
        s.next_cmd_id = 50;
        s.attached(attached(7, 3, AttachMode::Replay, 41));
        assert_eq!(s.next_cmd_id(), 50);
        s.attached(attached(7, 3, AttachMode::Fresh, 60));
        assert_eq!(s.next_cmd_id(), 61);
        // Another game: the counter is the server's, even when lower.
        s.attached(attached(8, 1, AttachMode::Fresh, 2));
        assert_eq!(s.next_cmd_id(), 3);
    }

    #[test]
    fn another_game_drops_the_basis_claim() {
        let mut s = SessionState::new("a");
        s.logged_in(Some(token(1)), "a".into());
        s.attached(attached(7, 3, AttachMode::Fresh, 0));
        s.set_basis_claim(Some(BasisClaim { game: 7, hash: 1 }));
        s.attached(attached(7, 3, AttachMode::Fresh, 0));
        assert!(
            s.basis_claim().is_some(),
            "Fresh of the same game keeps the basis (NC-D18)"
        );
        s.detached(&DetachReason::GameOver);
        s.attached(attached(8, 1, AttachMode::Fresh, 0));
        assert!(s.basis_claim().is_none());
    }

    #[test]
    fn detach_reasons_make_notices_except_left() {
        let mut s = SessionState::new("a");
        s.logged_in(Some(token(1)), "a".into());
        for (r, n) in [
            (DetachReason::Kicked, Some(NoticeKind::Kicked)),
            (DetachReason::GameOver, Some(NoticeKind::GameOver)),
            (
                DetachReason::GameStopped { why: "x".into() },
                Some(NoticeKind::GameStopped { why: "x".into() }),
            ),
            (DetachReason::Replaced, Some(NoticeKind::Replaced)),
            (DetachReason::Left, None),
        ] {
            s.attached(attached(7, 3, AttachMode::Fresh, 0));
            s.detached(&r);
            assert_eq!(s.notice().cloned(), n);
            assert_eq!(s.phase(), Phase::LoggedIn);
            s.take_notice();
        }
    }

    #[test]
    fn rejects_erase_the_token_only_for_credential_problems() {
        for (r, erased) in [
            (RejectReason::BadPassword, true),
            (RejectReason::BadNick, true),
            (RejectReason::NickTaken, true),
            (RejectReason::VersionMismatch { server: 9 }, false),
            (RejectReason::Full, false),
            (RejectReason::Unsupported { kind: "x".into() }, false),
        ] {
            let mut s = SessionState::new("a");
            s.set_token(Some(token(1)));
            s.rejected_with(r.clone());
            assert_eq!(s.token().is_none(), erased, "{r:?}");
            assert_eq!(s.phase(), Phase::Rejected);
            assert_eq!(s.rejected(), Some(&r));
        }
    }
}
