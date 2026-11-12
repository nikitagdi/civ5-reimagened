//! Where the orders of UI v2 leave: [`NetSink`], the actions module's [`OrderSink`] over the network client's
//! [`ClientCore`], and [`HostNet`], what one frame of [`V2Ui`](crate::V2Ui) needs of the network.
//!
//! A `NetSink` lives for one frame: the application makes it inside the borrow of its network handle and drops it
//! before the borrow ends, so whatever the core queued leaves at once. That borrow is never taken inside a call
//! into page script or from one: the page is single threaded, and the frame, the network timer and the page
//! events never run inside one another. Nothing here calls back into script either.
//!
//! The camera is not the network's: the sink only collects the requests of the frame, in order, and the
//! application carries them out the way it carries out the classic UI's `UI.LookAt` (see [`NetSink::look_at`]).

use client_core::{ClientCore, OrderRefused};
use protocol::{ChatTo, CmdId, Command, PlayerId, SeatView, TurnState};
use ui2_actions::{CameraReq, OrderSink, Refused};
use ui2_model::r#gen::ScreenId;
use ui2_source::SessionRead;

/// What one frame of UI v2 needs of the network: a sink for orders and the session to read. The source reads
/// the session after the actions module used the sink, never both at once.
pub trait HostNet {
    /// Where orders, chat lines and camera requests go.
    fn sink(&mut self) -> &mut dyn OrderSink;
    /// The client session the source reads (connection, seat, turn state, chat).
    fn session(&self) -> &dyn SessionRead;
}

/// The refusal of the network client as the actions module names it.
#[must_use]
pub fn refused(r: OrderRefused) -> Refused {
    match r {
        OrderRefused::NoConnection => Refused::NoConnection,
        OrderRefused::NoGame => Refused::NoGame,
        OrderRefused::TooMany => Refused::TooMany,
        OrderRefused::TurnChanged => Refused::TurnChanged,
        OrderRefused::TooBig => Refused::TooBig,
    }
}

/// The actions module's sink over the network client, for one frame.
pub struct NetSink<'a> {
    core: &'a mut ClientCore,
    camera: Vec<CameraReq>,
}

impl<'a> NetSink<'a> {
    /// A sink over the core the application lends for this frame.
    pub fn new(core: &'a mut ClientCore) -> NetSink<'a> {
        NetSink {
            core,
            camera: Vec::new(),
        }
    }

    /// The camera requests of the frame, oldest first, taken once.
    pub fn take_camera(&mut self) -> Vec<CameraReq> {
        std::mem::take(&mut self.camera)
    }

    /// The plot the camera should go to after this frame, in the form the classic UI's `UI.LookAt` gives the
    /// application (a plot the camera eases to): the last request of the frame wins.
    #[must_use]
    pub fn look_at(&self) -> Option<(i32, i32)> {
        self.camera.last().map(|r| match *r {
            CameraReq::Plot(x, y) => (i32::from(x), i32::from(y)),
        })
    }
}

impl OrderSink for NetSink<'_> {
    /// `ClientCore::order_at` stamped with the turn of the model: an order given on a turn the server has left
    /// is refused there instead of landing in the next one.
    fn command(&mut self, cmd: Command, turn: i32) -> Result<CmdId, Refused> {
        self.core.order_at(cmd, Some(turn)).map_err(refused)
    }

    fn chat(&mut self, to: ChatTo, text: &str) -> Result<(), Refused> {
        self.core.chat(to, text).map_err(refused)
    }

    fn camera(&mut self, req: CameraReq) {
        self.camera.push(req);
    }

    /// No screen outside the skin can be opened from UI v2 yet: the caller tells the player so.
    // SIMPLIFIED(SIMP-266): a screen the skin lacks is refused with a notice; the hand-off to the classic UI for the time of that screen is not built.
    fn open_screen(&mut self, _s: ScreenId) -> bool {
        false
    }
}

impl HostNet for NetSink<'_> {
    fn sink(&mut self) -> &mut dyn OrderSink {
        self
    }

    fn session(&self) -> &dyn SessionRead {
        &*self.core
    }
}

/// No network (no game, no core): every order and chat line is refused for want of a connection, the session
/// is empty, camera requests are dropped (there is no map to look at).
#[derive(Debug, Default)]
pub struct Offline;

impl OrderSink for Offline {
    fn command(&mut self, _cmd: Command, _turn: i32) -> Result<CmdId, Refused> {
        Err(Refused::NoConnection)
    }

    fn chat(&mut self, _to: ChatTo, _text: &str) -> Result<(), Refused> {
        Err(Refused::NoConnection)
    }

    fn camera(&mut self, _req: CameraReq) {}

    fn open_screen(&mut self, _s: ScreenId) -> bool {
        false
    }
}

impl SessionRead for Offline {
    fn connected(&self) -> bool {
        false
    }

    fn me(&self) -> Option<PlayerId> {
        None
    }

    fn turn_state(&self) -> Option<&TurnState> {
        None
    }

    fn my_finished(&self) -> Option<bool> {
        None
    }

    fn timer_left_ms(&self, _now_ms: u64) -> Option<u64> {
        None
    }

    fn seats(&self) -> &[SeatView] {
        &[]
    }

    fn chat_total(&self) -> u64 {
        0
    }

    fn chat_lines(&self) -> &[client_core::ChatEntry] {
        &[]
    }
}

impl HostNet for Offline {
    fn sink(&mut self) -> &mut dyn OrderSink {
        self
    }

    fn session(&self) -> &dyn SessionRead {
        self
    }
}

#[cfg(test)]
mod tests {
    use client_core::NetConfig;

    use super::*;

    fn core() -> ClientCore {
        ClientCore::new(NetConfig::new("t", "alice"))
    }

    #[test]
    fn every_refusal_of_the_core_has_its_name() {
        for (r, want) in [
            (OrderRefused::NoConnection, Refused::NoConnection),
            (OrderRefused::NoGame, Refused::NoGame),
            (OrderRefused::TooMany, Refused::TooMany),
            (OrderRefused::TurnChanged, Refused::TurnChanged),
            (OrderRefused::TooBig, Refused::TooBig),
        ] {
            assert_eq!(refused(r), want, "{r:?}");
        }
    }

    #[test]
    fn a_core_without_a_game_refuses_orders_and_chat() {
        let mut core = core();
        let mut sink = NetSink::new(&mut core);
        let r = sink.command(Command::EndTurn { force: false }, 42);
        assert!(
            matches!(r, Err(Refused::NoConnection | Refused::NoGame)),
            "{r:?}"
        );
        let r = sink.chat(ChatTo::All, "hello");
        assert!(
            matches!(r, Err(Refused::NoConnection | Refused::NoGame)),
            "{r:?}"
        );
        assert!(!sink.session().connected());
        assert_eq!(sink.session().me(), None);
        drop(sink);
        assert_eq!(core.stats().orders_sent, 0, "nothing went out");
    }

    #[test]
    fn camera_requests_are_kept_for_the_application_in_order() {
        let mut core = core();
        let mut sink = NetSink::new(&mut core);
        assert_eq!(sink.look_at(), None);
        sink.camera(CameraReq::Plot(3, 4));
        sink.camera(CameraReq::Plot(-1, 7));
        assert_eq!(sink.look_at(), Some((-1, 7)), "the last request wins");
        assert_eq!(
            sink.take_camera(),
            vec![CameraReq::Plot(3, 4), CameraReq::Plot(-1, 7)]
        );
        assert!(sink.take_camera().is_empty() && sink.look_at().is_none());
    }

    #[test]
    fn no_screen_opens_outside_the_skin_yet() {
        let mut core = core();
        let mut sink = NetSink::new(&mut core);
        assert!(!sink.open_screen(ScreenId::Policies));
        assert!(!Offline.open_screen(ScreenId::Policies));
    }

    #[test]
    fn offline_refuses_everything_and_reads_empty() {
        let mut off = Offline;
        assert_eq!(
            off.sink().command(Command::EndTurn { force: false }, 1),
            Err(Refused::NoConnection)
        );
        assert_eq!(
            off.sink().chat(ChatTo::All, "x"),
            Err(Refused::NoConnection)
        );
        let s = off.session();
        assert!(!s.connected() && s.turn_state().is_none() && s.seats().is_empty());
        assert_eq!((s.chat_total(), s.timer_left_ms(5)), (0, None));
    }
}
