//! Minimal `block_on`: parks the thread until woken.

use std::future::Future;
use std::pin::pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};

struct ThreadWaker(Thread);

impl Wake for ThreadWaker {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.unpark();
    }
}

/// Runs the future to completion on the current thread.
pub fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    let waker = Waker::from(Arc::new(ThreadWaker(thread::current())));
    let mut cx = Context::from_waker(&waker);
    loop {
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(v) => return v,
            Poll::Pending => thread::park(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn ready_future() {
        assert_eq!(block_on(async { 42 }), 42);
    }

    #[test]
    fn wakes_from_other_thread() {
        struct Flag(mpsc::Receiver<u32>, Option<Waker>);
        impl Future for Flag {
            type Output = u32;
            fn poll(mut self: std::pin::Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<u32> {
                match self.0.try_recv() {
                    Ok(v) => Poll::Ready(v),
                    Err(_) => {
                        self.1 = Some(cx.waker().clone());
                        let w = cx.waker().clone();
                        thread::spawn(move || w.wake());
                        Poll::Pending
                    }
                }
            }
        }
        let (tx, rx) = mpsc::channel();
        let h = thread::spawn(move || tx.send(7).ok());
        h.join().ok();
        assert_eq!(block_on(Flag(rx, None)), 7);
    }
}
