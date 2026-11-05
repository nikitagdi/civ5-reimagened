//! Background tasks (D8, architecture §7).

/// Task for a [`TaskPool`]. `Send` so it can run on another thread natively.
pub type Job = Box<dyn FnOnce() + Send + 'static>;

/// Pool for asset decoding and similar work: a thread pool natively, drained per frame on web.
pub trait TaskPool {
    fn spawn(&self, job: Job);

    /// Runs pending tasks until `budget_ms` is spent. Checked between tasks, never mid-task.
    fn run_pending(&self, budget_ms: u32);
}
