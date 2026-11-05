//! Message transport: WebSocket (binary) now; WebTransport later behind the same traits (D6).

pub trait Transport {
    fn connect(&self, url: &str) -> Box<dyn Connection>;
}

pub trait Connection {
    /// Sends sent before `Opened` are delivered after it opens; sends after close are dropped.
    fn send(&mut self, bytes: Vec<u8>);

    fn poll(&mut self) -> Option<TransportEvent>;

    /// Afterwards `poll` yields `Closed(Local)`.
    fn close(&mut self);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportEvent {
    Opened,
    Message(Vec<u8>),
    Closed(CloseReason),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloseReason {
    Local,
    Remote {
        code: u16,
        reason: String,
    },
    /// Network or protocol error, including a failed connect.
    Error(String),
}
