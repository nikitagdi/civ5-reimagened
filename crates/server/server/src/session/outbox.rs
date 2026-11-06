//! Send queue of a session: other tasks (lobby, game, seat pipeline) push framed
//! messages, the session task is the only writer to the socket. The queue holds at most
//! `max_msgs` messages or `max_bytes` bytes; a push beyond that marks the queue overflowed, and
//! the session closes the socket with 1013 (the reader is too slow) instead of buffering more.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use tokio::sync::{Notify, mpsc};

/// Why a push failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushError {
    /// The queue is over its limit; the session is going to close with 1013.
    Overflow,
    /// The session has ended.
    Closed,
}

struct State {
    msgs: AtomicUsize,
    bytes: AtomicUsize,
    overflow: AtomicBool,
    wake: Notify,
    max_msgs: usize,
    max_bytes: usize,
}

/// The producer half. Cheap to clone; any task may push.
#[derive(Clone)]
pub struct Outbox {
    tx: mpsc::UnboundedSender<Vec<u8>>,
    state: Arc<State>,
}

/// The consumer half, owned by the session task.
pub struct OutboxRx {
    rx: mpsc::UnboundedReceiver<Vec<u8>>,
    state: Arc<State>,
}

/// A queue with the given limits.
pub fn channel(max_msgs: usize, max_bytes: usize) -> (Outbox, OutboxRx) {
    let (tx, rx) = mpsc::unbounded_channel();
    let state = Arc::new(State {
        msgs: AtomicUsize::new(0),
        bytes: AtomicUsize::new(0),
        overflow: AtomicBool::new(false),
        wake: Notify::new(),
        max_msgs,
        max_bytes,
    });
    (
        Outbox {
            tx,
            state: Arc::clone(&state),
        },
        OutboxRx { rx, state },
    )
}

impl std::fmt::Debug for Outbox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Outbox")
    }
}

impl Outbox {
    /// Queues one framed message (WS binary payload). Never waits.
    pub fn push(&self, frame: Vec<u8>) -> Result<(), PushError> {
        let st = &self.state;
        if st.overflow.load(Ordering::Acquire) {
            return Err(PushError::Overflow);
        }
        let len = frame.len();
        let msgs = st.msgs.fetch_add(1, Ordering::AcqRel) + 1;
        let bytes = st.bytes.fetch_add(len, Ordering::AcqRel) + len;
        if msgs > st.max_msgs || bytes > st.max_bytes {
            st.overflow.store(true, Ordering::Release);
            // `notify_one` keeps a permit if the session is not waiting right now.
            st.wake.notify_one();
            return Err(PushError::Overflow);
        }
        self.tx.send(frame).map_err(|_| PushError::Closed)
    }

    /// Ends the session after everything queued so far has been written (a refusal such as `Reject` is the last thing
    /// the client reads). The marker is an empty frame, which no message is.
    pub fn close_after_flush(&self) -> Result<(), PushError> {
        self.push(Vec::new())
    }

    /// Closes the queue as if it had overflowed: the session ends with 1013 (test hook `close_socket`).
    pub fn close(&self) {
        self.state.overflow.store(true, Ordering::Release);
        self.state.wake.notify_one();
    }

    /// True once a push went over the limit.
    pub fn is_overflowed(&self) -> bool {
        self.state.overflow.load(Ordering::Acquire)
    }
}

impl OutboxRx {
    /// Next queued message. `None` when every producer is gone and the queue is empty.
    pub async fn recv(&mut self) -> Option<Vec<u8>> {
        let frame = self.rx.recv().await?;
        self.state.msgs.fetch_sub(1, Ordering::AcqRel);
        self.state.bytes.fetch_sub(frame.len(), Ordering::AcqRel);
        Some(frame)
    }

    /// A handle to wait for overflow while `recv` borrows the receiver.
    pub fn watch(&self) -> OverflowWatch {
        OverflowWatch(Arc::clone(&self.state))
    }
}

/// Waits for the queue to overflow; see [`OutboxRx::watch`].
#[derive(Clone)]
pub struct OverflowWatch(Arc<State>);

impl OverflowWatch {
    /// Resolves when a push went over the limit.
    pub async fn wait(&self) {
        loop {
            if self.0.overflow.load(Ordering::Acquire) {
                return;
            }
            self.0.wake.notified().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn messages_come_out_in_order() {
        let (tx, mut rx) = channel(4, 100);
        tx.push(vec![1]).unwrap();
        tx.push(vec![2, 2]).unwrap();
        assert_eq!(rx.recv().await, Some(vec![1]));
        assert_eq!(rx.recv().await, Some(vec![2, 2]));
        assert!(!tx.is_overflowed());
    }

    #[tokio::test]
    async fn the_message_count_is_limited() {
        let (tx, rx) = channel(256, usize::MAX);
        for _ in 0..256 {
            tx.push(vec![0]).unwrap();
        }
        assert_eq!(tx.push(vec![0]), Err(PushError::Overflow));
        assert!(tx.is_overflowed());
        // It stays overflowed; the reader is told.
        assert_eq!(tx.push(vec![0]), Err(PushError::Overflow));
        rx.watch().wait().await;
    }

    #[tokio::test]
    async fn the_byte_size_is_limited() {
        let (tx, _rx) = channel(usize::MAX, 4 << 20);
        for _ in 0..4 {
            tx.push(vec![0; 1 << 20]).unwrap();
        }
        assert_eq!(tx.push(vec![0]), Err(PushError::Overflow));
    }

    #[tokio::test]
    async fn the_close_marker_is_an_empty_frame_behind_the_queued_ones() {
        let (tx, mut rx) = channel(4, 100);
        tx.push(vec![1]).unwrap();
        tx.close_after_flush().unwrap();
        assert_eq!(rx.recv().await, Some(vec![1]));
        assert_eq!(rx.recv().await, Some(Vec::new()));
        assert!(!tx.is_overflowed());
    }

    #[tokio::test]
    async fn draining_makes_room() {
        let (tx, mut rx) = channel(2, 100);
        tx.push(vec![1]).unwrap();
        tx.push(vec![2]).unwrap();
        assert_eq!(rx.recv().await, Some(vec![1]));
        tx.push(vec![3]).unwrap();
        assert_eq!(rx.recv().await, Some(vec![2]));
        assert_eq!(rx.recv().await, Some(vec![3]));
    }

    #[tokio::test]
    async fn overflow_wakes_a_waiting_session() {
        let (tx, rx) = channel(1, 100);
        let waiter = tokio::spawn(async move { rx.watch().wait().await });
        tokio::task::yield_now().await;
        tx.push(vec![1]).unwrap();
        let _ = tx.push(vec![2]);
        tokio::time::timeout(std::time::Duration::from_secs(5), waiter)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn a_push_after_the_session_ended_is_closed() {
        let (tx, rx) = channel(4, 100);
        drop(rx);
        assert_eq!(tx.push(vec![1]), Err(PushError::Closed));
    }
}
