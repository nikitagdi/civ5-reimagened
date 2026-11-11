//! Where orders leave the actions module: the [`OrderSink`] the host implements over the network client, and a
//! recording double for tests.

use protocol::{ChatTo, CmdId, Command};
use ui2_model::generated::{RefuseWhy, ScreenId};

/// Why the sink did not take an order. Mirrors the refusals of the network client.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Refused {
    #[error("no connection to the server")]
    NoConnection,
    #[error("no game is running")]
    NoGame,
    #[error("too many orders are waiting for an answer")]
    TooMany,
    #[error("the turn changed")]
    TurnChanged,
    #[error("the order is too big")]
    TooBig,
}

impl Refused {
    /// The reason the skin shows.
    #[must_use]
    pub fn why(self) -> RefuseWhy {
        match self {
            Refused::NoConnection => RefuseWhy::NoConnection,
            Refused::NoGame => RefuseWhy::NoGame,
            Refused::TooMany => RefuseWhy::TooMany,
            Refused::TurnChanged => RefuseWhy::TurnChanged,
            Refused::TooBig => RefuseWhy::TooBig,
        }
    }
}

/// A request to move the camera. The position comes from the model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CameraReq {
    /// Center on a plot.
    Plot(i16, i16),
}

/// What the actions module needs from the host: orders to the server, chat, the camera and the screens that
/// live outside the skin. The orders are typed [`Command`]s; the module never builds a GCN1 message and never
/// touches the replica.
pub trait OrderSink {
    /// Send an order, stamped with the turn of the model it was given in. The sink answers at once only with
    /// a local refusal; the server's answer comes later as an event ([`ActionEvent`](crate::ActionEvent)).
    ///
    /// # Errors
    /// The order was not sent.
    fn command(&mut self, cmd: Command, turn: i32) -> Result<CmdId, Refused>;

    /// Send a chat line.
    ///
    /// # Errors
    /// The line was not sent.
    fn chat(&mut self, to: ChatTo, text: &str) -> Result<(), Refused>;

    /// Move the camera.
    fn camera(&mut self, req: CameraReq);

    /// Open a screen that lives outside the skin; `false` if the host has no way to.
    fn open_screen(&mut self, s: ScreenId) -> bool;
}

/// One thing a [`RecordingSink`] was asked to do.
#[derive(Clone, Debug, PartialEq)]
pub enum Sent {
    Command { id: CmdId, cmd: Command, turn: i32 },
    Chat { to: ChatTo, text: String },
    Camera(CameraReq),
    Screen(ScreenId),
}

/// A sink for tests: keeps everything it is given, numbers the orders and can refuse on demand.
#[derive(Clone, Debug)]
pub struct RecordingSink {
    /// Everything accepted, in order.
    pub sent: Vec<Sent>,
    /// The id the next order gets.
    pub next_id: CmdId,
    /// Refuse every order and chat line with this until it is cleared.
    pub refuse: Option<Refused>,
    /// Whether [`OrderSink::open_screen`] finds a way to open screens.
    pub has_screens: bool,
}

impl Default for RecordingSink {
    fn default() -> RecordingSink {
        RecordingSink {
            sent: Vec::new(),
            next_id: 1,
            refuse: None,
            has_screens: true,
        }
    }
}

impl RecordingSink {
    /// An empty sink that accepts everything.
    #[must_use]
    pub fn new() -> RecordingSink {
        RecordingSink::default()
    }

    /// The orders sent, without the other kinds.
    #[must_use]
    pub fn commands(&self) -> Vec<Command> {
        self.sent
            .iter()
            .filter_map(|s| match s {
                Sent::Command { cmd, .. } => Some(*cmd),
                _ => None,
            })
            .collect()
    }
}

impl OrderSink for RecordingSink {
    fn command(&mut self, cmd: Command, turn: i32) -> Result<CmdId, Refused> {
        if let Some(r) = self.refuse {
            return Err(r);
        }
        let id = self.next_id;
        self.next_id += 1;
        self.sent.push(Sent::Command { id, cmd, turn });
        Ok(id)
    }

    fn chat(&mut self, to: ChatTo, text: &str) -> Result<(), Refused> {
        if let Some(r) = self.refuse {
            return Err(r);
        }
        self.sent.push(Sent::Chat {
            to,
            text: text.to_string(),
        });
        Ok(())
    }

    fn camera(&mut self, req: CameraReq) {
        self.sent.push(Sent::Camera(req));
    }

    fn open_screen(&mut self, s: ScreenId) -> bool {
        if self.has_screens {
            self.sent.push(Sent::Screen(s));
        }
        self.has_screens
    }
}
