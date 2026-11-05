//! The WebSocket of one bot: `tungstenite` on a plain TCP stream, no TLS, a read timeout
//! of 1 ms so that one thread can read, send and keep time. Time is `Instant` only.

use std::io;
use std::net::TcpStream;
use std::time::{Duration, Instant};

use protocol::{ClientMsg, Codec, Postcard, ServerMsg};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};
use wire::{Encoder, Limits};

use super::BotError;

const READ_TIMEOUT: Duration = Duration::from_millis(1);
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);

/// One server message with what the bot measures about it.
#[derive(Debug)]
pub struct Frame {
    pub msg: ServerMsg,
    /// Length of the WebSocket message body (compressed, as it came).
    pub wire_bytes: usize,
    /// When it was read.
    pub at: Instant,
}

/// What a read gave.
#[derive(Debug)]
pub enum Poll {
    Frame(Box<Frame>),
    Idle,
    /// The peer closed (with a close frame or without).
    Closed {
        code: Option<u16>,
        reason: String,
    },
}

pub struct Conn {
    ws: WebSocket<MaybeTlsStream<TcpStream>>,
    /// A close frame came; the next read reports it.
    close: Option<(Option<u16>, String)>,
}

impl Conn {
    /// Connects and finishes the WebSocket handshake. Only `ws://` (the gateway has no TLS, NT-D1).
    pub fn open(url: &str) -> Result<Conn, BotError> {
        let (mut ws, _) = tungstenite::connect(url).map_err(|e| BotError::Ws(e.to_string()))?;
        match ws.get_mut() {
            MaybeTlsStream::Plain(s) => {
                s.set_nodelay(true)?;
                s.set_read_timeout(Some(READ_TIMEOUT))?;
                s.set_write_timeout(Some(WRITE_TIMEOUT))?;
            }
            _ => return Err(BotError::Ws("only plain ws:// is supported".into())),
        }
        Ok(Conn { ws, close: None })
    }

    /// Sends one message; returns the body length and the moment it was handed to the socket.
    pub fn send(&mut self, msg: &ClientMsg) -> Result<(usize, Instant), BotError> {
        let body = Postcard
            .encode(msg)
            .map_err(|e| BotError::Codec(e.to_string()))?;
        let frame = Encoder::plain(&body);
        let len = frame.len();
        let at = Instant::now();
        self.ws
            .send(Message::binary(frame))
            .map_err(|e| BotError::Ws(e.to_string()))?;
        Ok((len, at))
    }

    /// Sends a message the way the caller built it (`bytes` is a whole WebSocket body). For the raw peer:
    /// a corrupted or oversized frame.
    pub fn send_raw(&mut self, bytes: Vec<u8>) -> Result<(), BotError> {
        self.ws
            .send(Message::binary(bytes))
            .map_err(|e| BotError::Ws(e.to_string()))
    }

    /// Reads at most one message (waits at most the read timeout).
    pub fn poll(&mut self) -> Result<Poll, BotError> {
        if let Some((code, reason)) = self.close.take() {
            return Ok(Poll::Closed { code, reason });
        }
        match self.ws.read() {
            Ok(Message::Binary(b)) => {
                let at = Instant::now();
                let msg = wire::decode::<ServerMsg>(&b, &Limits::CLIENT)
                    .map_err(|e| BotError::Codec(e.to_string()))?;
                Ok(Poll::Frame(Box::new(Frame {
                    msg,
                    wire_bytes: b.len(),
                    at,
                })))
            }
            Ok(Message::Close(frame)) => {
                let (code, reason) = frame
                    .map(|f| (Some(u16::from(f.code)), f.reason.to_string()))
                    .unwrap_or((None, String::new()));
                Ok(Poll::Closed { code, reason })
            }
            Ok(Message::Text(_)) => {
                Err(BotError::Protocol("a text message from the server".into()))
            }
            Ok(_) => Ok(Poll::Idle),
            Err(tungstenite::Error::Io(e))
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                // Flushes what the library queued (the answer to a WebSocket ping).
                match self.ws.flush() {
                    Ok(()) => Ok(Poll::Idle),
                    Err(tungstenite::Error::Io(e))
                        if matches!(
                            e.kind(),
                            io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                        ) =>
                    {
                        Ok(Poll::Idle)
                    }
                    Err(e) => Err(BotError::Ws(e.to_string())),
                }
            }
            Err(tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed) => {
                Ok(Poll::Closed {
                    code: None,
                    reason: "connection closed".into(),
                })
            }
            Err(tungstenite::Error::Io(e)) => Ok(Poll::Closed {
                code: None,
                reason: format!("io: {e}"),
            }),
            Err(e) => Err(BotError::Ws(e.to_string())),
        }
    }

    /// A polite close (close frame, then the socket goes).
    pub fn close(mut self) {
        let _ = self.ws.close(None);
        let _ = self.ws.flush();
    }

    /// The socket goes without a close frame (a dropped link: the server sees the end of the stream).
    pub fn drop_hard(self) {
        drop(self);
    }
}
