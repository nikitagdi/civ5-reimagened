//! `TaskPool` on std threads.

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use platform::{Job, TaskPool};

/// Shared queue. Drop waits for queued jobs.
pub struct ThreadPool {
    sender: Option<Sender<Job>>,
    workers: Vec<JoinHandle<()>>,
}

impl ThreadPool {
    /// At least one worker.
    pub fn new(threads: usize) -> Self {
        let (sender, receiver) = mpsc::channel::<Job>();
        let receiver = Arc::new(Mutex::new(receiver));
        let workers = (0..threads.max(1))
            .filter_map(|i| {
                let rx = Arc::clone(&receiver);
                thread::Builder::new()
                    .name(format!("civ5-task-{i}"))
                    .spawn(move || worker(&rx))
                    .map_err(|e| tracing::error!("failed to start pool thread: {e}"))
                    .ok()
            })
            .collect();
        Self {
            sender: Some(sender),
            workers,
        }
    }

    /// Cores minus one, clamped to 1..=8.
    pub fn with_default_size() -> Self {
        let n = thread::available_parallelism().map_or(2, |n| n.get());
        Self::new(n.saturating_sub(1).clamp(1, 8))
    }
}

fn worker(rx: &Mutex<Receiver<Job>>) {
    loop {
        // Hold the lock only while receiving, so jobs run unlocked.
        let job = match rx.lock() {
            Ok(guard) => guard.recv(),
            Err(_) => return,
        };
        match job {
            Ok(job) => job(),
            Err(_) => return,
        }
    }
}

impl TaskPool for ThreadPool {
    fn spawn(&self, job: Job) {
        let sent = self.sender.as_ref().is_some_and(|s| s.send(job).is_ok());
        if !sent {
            tracing::warn!("thread pool closed, task dropped");
        }
    }

    fn run_pending(&self, _budget_ms: u32) {}
}

impl Drop for ThreadPool {
    fn drop(&mut self) {
        self.sender = None;
        for w in self.workers.drain(..) {
            let _ = w.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_jobs_on_workers() {
        let pool = ThreadPool::new(3);
        let (tx, rx) = mpsc::channel();
        for i in 0..10 {
            let tx = tx.clone();
            pool.spawn(Box::new(move || {
                tx.send(i).ok();
            }));
        }
        drop(tx);
        drop(pool);
        let mut got: Vec<i32> = rx.iter().collect();
        got.sort();
        assert_eq!(got, (0..10).collect::<Vec<_>>());
    }
}
