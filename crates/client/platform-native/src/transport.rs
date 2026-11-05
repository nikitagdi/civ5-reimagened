//! WebSocket on tungstenite, in its own thread, over channels. No TLS.

use std::io;
use std::net::TcpStream;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread;
use std::time::Duration;

use platform::{CloseReason, Connection, Transport, TransportEvent};
use tungstenite::protocol::CloseFrame;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

/// Also bounds outgoing-message latency.
const READ_TIMEOUT: Duration = Duration::from_millis(5);

/// Server reads nothing this long: dead.
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Default, Clone, Copy)]
pub struct WsTransport;

impl Transport for WsTransport {
    fn connect(&self, url: &str) -> Box<dyn Connection> {
        Box::new(WsConnection::start(url.to_owned(), WRITE_TIMEOUT))
    }
}

enum Command {
    Send(Vec<u8>),
    Close,
}

struct WsConnection {
    commands: Sender<Command>,
    events: Receiver<TransportEvent>,
    local_closed: bool,
    /// `Closed(Local)` is still owed.
    pending_local: bool,
    finished: bool,
}

impl WsConnection {
    fn start(url: String, write_timeout: Duration) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (ev_tx, ev_rx) = mpsc::channel();
        let spawned = thread::Builder::new()
            .name("civ5-ws".into())
            .spawn(move || run(&url, write_timeout, &cmd_rx, &ev_tx));
        if let Err(e) = spawned {
            // `poll` reports the disconnect, since the event channel closes.
            tracing::error!("failed to start WebSocket thread: {e}");
        }
        Self {
            commands: cmd_tx,
            events: ev_rx,
            local_closed: false,
            pending_local: false,
            finished: false,
        }
    }
}

impl Connection for WsConnection {
    fn send(&mut self, bytes: Vec<u8>) {
        if !self.local_closed && !self.finished {
            // Error: the thread exited; `poll` reports it.
            let _ = self.commands.send(Command::Send(bytes));
        }
    }

    fn poll(&mut self) -> Option<TransportEvent> {
        if self.finished {
            return None;
        }
        if self.local_closed {
            if self.pending_local {
                self.pending_local = false;
                self.finished = true;
                return Some(TransportEvent::Closed(CloseReason::Local));
            }
            return None;
        }
        match self.events.try_recv() {
            Ok(ev) => {
                if matches!(ev, TransportEvent::Closed(_)) {
                    self.finished = true;
                }
                Some(ev)
            }
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                self.finished = true;
                Some(TransportEvent::Closed(CloseReason::Error(
                    "connection thread terminated".into(),
                )))
            }
        }
    }

    fn close(&mut self) {
        if !self.local_closed && !self.finished {
            self.local_closed = true;
            self.pending_local = true;
            let _ = self.commands.send(Command::Close);
        }
    }
}

impl Drop for WsConnection {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Close);
    }
}

type Socket = WebSocket<MaybeTlsStream<TcpStream>>;

fn run(
    url: &str,
    write_timeout: Duration,
    commands: &Receiver<Command>,
    events: &Sender<TransportEvent>,
) {
    let mut ws = match tungstenite::connect(url) {
        Ok((ws, _response)) => ws,
        Err(e) => {
            let _ = events.send(TransportEvent::Closed(CloseReason::Error(format!(
                "failed to connect to {url}: {e}"
            ))));
            return;
        }
    };
    #[allow(irrefutable_let_patterns)] // only one variant without TLS features
    if let MaybeTlsStream::Plain(stream) = ws.get_mut() {
        let _ = stream.set_nodelay(true);
        // Write timeout: send/flush must not block forever.
        let configured = stream
            .set_read_timeout(Some(READ_TIMEOUT))
            .and_then(|()| stream.set_write_timeout(Some(write_timeout)));
        if let Err(e) = configured {
            let _ = events.send(TransportEvent::Closed(CloseReason::Error(format!(
                "failed to configure socket: {e}"
            ))));
            return;
        }
    }
    tracing::debug!("WebSocket opened: {url}");
    if events.send(TransportEvent::Opened).is_err() {
        close_quietly(&mut ws);
        return;
    }

    let mut close_frame: Option<CloseFrame> = None;
    loop {
        // Outgoing.
        loop {
            match commands.try_recv() {
                Ok(Command::Send(bytes)) => {
                    if let Err(e) = ws.send(Message::Binary(bytes.into())) {
                        let _ = events.send(closed_by_error(e, close_frame.take()));
                        return;
                    }
                }
                Ok(Command::Close) | Err(TryRecvError::Disconnected) => {
                    close_quietly(&mut ws);
                    return;
                }
                Err(TryRecvError::Empty) => break,
            }
        }
        // Incoming.
        let event = match ws.read() {
            Ok(Message::Binary(bytes)) => Some(TransportEvent::Message(bytes.to_vec())),
            Ok(Message::Close(frame)) => {
                close_frame = frame;
                None
            }
            Ok(Message::Text(_)) => {
                tracing::warn!("WebSocket: ignored text message");
                None
            }
            Ok(_) => None,
            Err(tungstenite::Error::Io(e))
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                // Read timeout: flush queued output; a flush timeout means a write timeout.
                match ws.flush() {
                    Ok(()) => None,
                    Err(e) => {
                        let _ = events.send(closed_by_error(e, close_frame.take()));
                        return;
                    }
                }
            }
            Err(e) => {
                let _ = events.send(closed_by_error(e, close_frame.take()));
                return;
            }
        };
        if let Some(ev) = event
            && events.send(ev).is_err()
        {
            close_quietly(&mut ws);
            return;
        }
    }
}

fn closed_by_error(e: tungstenite::Error, frame: Option<CloseFrame>) -> TransportEvent {
    let reason = match e {
        tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed => {
            match frame {
                Some(f) => CloseReason::Remote {
                    code: u16::from(f.code),
                    reason: f.reason.to_string(),
                },
                // 1005: no status code.
                None => CloseReason::Remote {
                    code: 1005,
                    reason: String::new(),
                },
            }
        }
        tungstenite::Error::Io(e)
            if matches!(
                e.kind(),
                io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
            ) =>
        {
            CloseReason::Error(format!(
                "write timeout: server is not reading the socket ({e})"
            ))
        }
        e => CloseReason::Error(e.to_string()),
    };
    TransportEvent::Closed(reason)
}

fn close_quietly(ws: &mut Socket) {
    let _ = ws.close(None);
    // Don't wait for the reply.
    let _ = ws.flush();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::time::Instant;

    fn poll_until(conn: &mut dyn Connection, timeout: Duration) -> Option<TransportEvent> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if let Some(ev) = conn.poll() {
                return Some(ev);
            }
            thread::sleep(Duration::from_millis(2));
        }
        None
    }

    #[test]
    fn echo_roundtrip_and_remote_close() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept");
            let mut ws = tungstenite::accept(stream).expect("handshake");
            let msg = ws.read().expect("read");
            ws.send(msg).expect("echo");
            ws.close(None).expect("close");
            while ws.read().is_ok() {}
        });

        let mut conn = WsTransport.connect(&format!("ws://{addr}/ws"));
        conn.send(vec![1, 2, 3]);
        assert_eq!(
            poll_until(conn.as_mut(), Duration::from_secs(5)),
            Some(TransportEvent::Opened)
        );
        assert_eq!(
            poll_until(conn.as_mut(), Duration::from_secs(5)),
            Some(TransportEvent::Message(vec![1, 2, 3]))
        );
        match poll_until(conn.as_mut(), Duration::from_secs(5)) {
            Some(TransportEvent::Closed(CloseReason::Remote { .. })) => {}
            other => panic!("expected Closed(Remote), got {other:?}"),
        }
        assert_eq!(conn.poll(), None);
        server.join().expect("server");
    }

    #[test]
    fn write_timeout_reports_closed() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        let (done_tx, done_rx) = mpsc::channel::<()>();
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept");
            let _ws = tungstenite::accept(stream).expect("handshake");
            // Never read, so the buffers fill up.
            let _ = done_rx.recv();
        });

        let mut conn: Box<dyn Connection> = Box::new(WsConnection::start(
            format!("ws://{addr}/ws"),
            Duration::from_millis(300),
        ));
        assert_eq!(
            poll_until(conn.as_mut(), Duration::from_secs(5)),
            Some(TransportEvent::Opened)
        );
        for _ in 0..256 {
            conn.send(vec![0u8; 1 << 20]);
        }
        match poll_until(conn.as_mut(), Duration::from_secs(20)) {
            Some(TransportEvent::Closed(CloseReason::Error(msg))) => {
                assert!(msg.contains("write timeout"), "{msg}");
            }
            other => panic!("expected Closed(Error) on write timeout, got {other:?}"),
        }
        assert_eq!(conn.poll(), None);
        let _ = done_tx.send(());
        server.join().expect("server");
    }

    #[test]
    fn connect_error_is_reported() {
        let port = TcpListener::bind("127.0.0.1:0")
            .and_then(|l| l.local_addr())
            .expect("port")
            .port();
        let mut conn = WsTransport.connect(&format!("ws://127.0.0.1:{port}/ws"));
        match poll_until(conn.as_mut(), Duration::from_secs(10)) {
            Some(TransportEvent::Closed(CloseReason::Error(_))) => {}
            other => panic!("expected Closed(Error), got {other:?}"),
        }
    }

    #[test]
    fn local_close_reports_local() {
        let mut conn = WsTransport.connect("ws://127.0.0.1:1/ws");
        conn.close();
        assert_eq!(
            conn.poll(),
            Some(TransportEvent::Closed(CloseReason::Local))
        );
        assert_eq!(conn.poll(), None);
    }
}
