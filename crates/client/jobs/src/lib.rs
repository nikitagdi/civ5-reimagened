//! A pool of worker threads for pure CPU jobs (B3 render-mt).
//!
//! The jobs read plain data (or `Sync` borrows of it) and hand back their results; they never touch
//! the GPU (`wgpu` objects stay on the thread that made them). Two forms:
//!
//! * [`for_each`] / [`map`]: a fork-join over `0..n`. The caller works on the indices too and returns
//!   when every index is done, so a call site gets its results in the same frame, in index order:
//!   the numbers are the ones of a loop on one thread.
//! * [`submit`]: a job whose result is taken later ([`Pending::try_take`], a frame after or more).
//!
//! Without workers (the native build until someone starts them, the wasm build without threads, a page
//! without `crossOriginIsolated`) both run inline on the caller, exactly as the loop they replace.
//!
//! The main thread of a browser may not block (`memory.atomic.wait` traps there): nothing here waits on
//! a futex on the caller's side. The caller spins while the last indices finish, and takes the queue's
//! lock with `try_lock` in a spin. Only an idle worker sleeps (on the queue's condition variable).

use std::collections::VecDeque;
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
use std::sync::atomic::AtomicU32;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, TryLockError};

type Task = Box<dyn FnOnce() + Send>;

struct Pool {
    queue: Mutex<VecDeque<Task>>,
    wake: Condvar,
    /// Workers that have entered [`worker_main`].
    workers: AtomicUsize,
    /// Jobs run through the pool so far (fork-joins and submits that had workers).
    pooled: AtomicUsize,
}

static POOL: Pool = Pool {
    queue: Mutex::new(VecDeque::new()),
    wake: Condvar::new(),
    workers: AtomicUsize::new(0),
    pooled: AtomicUsize::new(0),
};

/// Workers running: 0 means every job runs inline on its caller.
pub fn workers() -> usize {
    POOL.workers.load(Ordering::Acquire)
}

/// Jobs that went through the pool so far (`jobs.pooled_total`).
pub fn pooled_total() -> usize {
    POOL.pooled.load(Ordering::Relaxed)
}

/// What a worker of the pool said as it died (its panic), for the caller that finds it dead.
static WORKER_FAILURE: Mutex<Option<String>> = Mutex::new(None);

/// Keeps the first failure of a worker (its panic hook calls it before the worker aborts).
pub fn note_worker_failure(text: String) {
    if let Ok(mut f) = WORKER_FAILURE.try_lock() {
        f.get_or_insert(text);
    }
}

/// The failure a worker noted, if one did.
pub fn worker_failure() -> Option<String> {
    WORKER_FAILURE.try_lock().ok().and_then(|f| f.clone())
}

/// Whether the wasm module is terminated: a thread trapped (a panic aborts, an allocation failed, a stack
/// overflowed) and wasm-bindgen set its flag, which sits in the shared memory, so every thread sees it.
/// A worker that trapped never finishes the index or lock it held: a caller that spins on it would spin
/// forever and freeze the tab (live report 2026-10-31), so the spins look at this flag.
pub fn module_terminated() -> bool {
    #[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
    {
        unsafe extern "C" {
            /// wasm-bindgen's `__rt::__instance_terminated` (`#[no_mangle]`), set by its glue.
            static __instance_terminated: u32;
        }
        let p = (&raw const __instance_terminated).cast_mut();
        // SAFETY: a `u32` static of wasm-bindgen's runtime, aligned, alive for the whole program; the glue
        // writes it through the memory, so it is read atomically.
        unsafe { AtomicU32::from_ptr(p) }.load(Ordering::Acquire) != 0
    }
    #[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
    {
        false
    }
}

/// Stops a spin of the caller when the module is terminated: panics with what the worker said (the page's
/// panic hook shows it over the canvas) instead of spinning forever on a dead worker.
fn check_alive() {
    if module_terminated() {
        let why = worker_failure().unwrap_or_else(|| {
            "no panic message: a trap (out of memory, a stack overflow, an abort)".to_owned()
        });
        panic!("a thread of the jobs pool died, the module is terminated: {why}");
    }
}

/// The queue's lock without ever sleeping on it (allowed on a browser's main thread).
fn lock_spin() -> MutexGuard<'static, VecDeque<Task>> {
    lock_without_wait(&POOL.queue)
}

/// A lock that a job and the main thread both take: spins on `try_lock` instead of sleeping on the
/// futex (which traps on a browser's main thread). A poisoned lock is taken as it is. Keep what is
/// done under it short.
pub fn lock_without_wait<T: ?Sized>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    loop {
        match m.try_lock() {
            Ok(g) => return g,
            Err(TryLockError::Poisoned(p)) => return p.into_inner(),
            Err(TryLockError::WouldBlock) => {
                check_alive();
                std::hint::spin_loop();
            }
        }
    }
}

fn push(tasks: impl IntoIterator<Item = Task>) {
    let mut n = 0;
    {
        let mut q = lock_spin();
        for t in tasks {
            q.push_back(t);
            n += 1;
        }
    }
    // `notify` is `memory.atomic.notify`: allowed on the main thread.
    if n > 1 {
        POOL.wake.notify_all();
    } else if n == 1 {
        POOL.wake.notify_one();
    }
}

/// A task of the queue, if one waits (a caller that spins helps with it).
fn try_pop() -> Option<Task> {
    match POOL.queue.try_lock() {
        Ok(mut q) => q.pop_front(),
        Err(TryLockError::Poisoned(p)) => p.into_inner().pop_front(),
        Err(TryLockError::WouldBlock) => None,
    }
}

/// The loop of a worker thread: never returns. A web worker calls it once the module is instantiated
/// on the shared memory (`apps/web`'s `jobs_worker_main`); natively [`start_native_workers`] does.
pub fn worker_main() -> ! {
    POOL.workers.fetch_add(1, Ordering::AcqRel);
    loop {
        let task = {
            let mut q = POOL.queue.lock().unwrap_or_else(|p| p.into_inner());
            loop {
                if let Some(t) = q.pop_front() {
                    break t;
                }
                q = POOL.wake.wait(q).unwrap_or_else(|p| p.into_inner());
            }
        };
        // A panicking task (native, unwinding) does not take the worker with it.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(task));
    }
}

/// Starts `n` worker threads of the pool on a native build (tests, and a desktop that wants them).
#[cfg(not(target_family = "wasm"))]
pub fn start_native_workers(n: usize) {
    for i in 0..n {
        std::thread::Builder::new()
            .name(format!("jobs-{i}"))
            .spawn(|| worker_main())
            .expect("a jobs worker thread");
    }
    while workers() < n {
        std::thread::yield_now();
    }
}

/// One fork-join: the indices are taken one by one by whoever is free.
struct Join {
    next: AtomicUsize,
    done: AtomicUsize,
    /// An index ended in a panic.
    panicked: AtomicBool,
    n: usize,
    /// The caller's closure; valid while the caller waits in [`for_each`], which it does until `done`
    /// is `n`. Read only for an index below `n`.
    f: *const (dyn Fn(usize) + Sync),
}

// SAFETY: `f` is `Sync` and is called only while the caller of `for_each` keeps it alive (see `Join::f`).
unsafe impl Send for Join {}
unsafe impl Sync for Join {}

/// Counts an index as done when its call ends, also by a panic (then the join is marked): a caller
/// never spins on an index whose thread unwound (native tests; the wasm builds abort).
struct Finish<'a>(&'a Join);

impl Drop for Finish<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            self.0.panicked.store(true, Ordering::Relaxed);
        }
        self.0.done.fetch_add(1, Ordering::Release);
    }
}

impl Join {
    fn work(&self) {
        loop {
            let i = self.next.fetch_add(1, Ordering::Relaxed);
            if i >= self.n {
                return;
            }
            let _finish = Finish(self);
            // SAFETY: `i < n`: the caller has not seen `done == n` yet, so `f` is alive.
            unsafe { (*self.f)(i) };
        }
    }
}

/// Calls `f(i)` for every `i` in `0..n`, on the workers and the caller, and returns when all are done.
/// Without workers (or for `n < 2`) it is `for i in 0..n { f(i) }` on the caller. `f` must not panic
/// (the builds abort on a panic).
pub fn for_each(n: usize, f: &(dyn Fn(usize) + Sync)) {
    let helpers = workers().min(n.saturating_sub(1));
    if helpers == 0 {
        for i in 0..n {
            f(i);
        }
        return;
    }
    POOL.pooled.fetch_add(1, Ordering::Relaxed);
    // SAFETY: the lifetime is widened for the pool; `Join::work` calls `f` only for an index below `n`
    // and this function returns only once all `n` are done, so no call outlives the borrow.
    let f: &'static (dyn Fn(usize) + Sync) = unsafe { std::mem::transmute(f) };
    let join = Arc::new(Join {
        next: AtomicUsize::new(0),
        done: AtomicUsize::new(0),
        panicked: AtomicBool::new(false),
        n,
        f: f as *const _,
    });
    push((0..helpers).map(|_| {
        let j = join.clone();
        Box::new(move || j.work()) as Task
    }));
    {
        // Waits for the helpers also when the caller's own index unwinds: `f` must outlive them.
        struct WaitAll<'a>(&'a Join);
        impl Drop for WaitAll<'_> {
            fn drop(&mut self) {
                // Indices nobody took yet are taken here, so they cannot run after `f` is gone.
                // The indices below the old `next` were taken (and run); every later claim is past `n`.
                let started = self
                    .0
                    .next
                    .fetch_add(self.0.n, Ordering::Relaxed)
                    .min(self.0.n);
                while self.0.done.load(Ordering::Acquire) < started {
                    check_alive();
                    std::hint::spin_loop();
                }
            }
        }
        let _wait = WaitAll(&join);
        join.work();
    }
    assert!(
        !join.panicked.load(Ordering::Relaxed),
        "a job of the pool panicked"
    );
}

/// `f(i)` for `i` in `0..n`, in index order ([`for_each`]).
pub fn map<T: Send>(n: usize, f: impl Fn(usize) -> T + Sync) -> Vec<T> {
    if workers() == 0 || n < 2 {
        return (0..n).map(f).collect();
    }
    let slots: Vec<Mutex<Option<T>>> = (0..n).map(|_| Mutex::new(None)).collect();
    for_each(n, &|i| {
        let v = f(i);
        // Each slot has one writer and no reader until the join is over: the lock never waits.
        *slots[i].lock().unwrap_or_else(|p| p.into_inner()) = Some(v);
    });
    slots
        .into_iter()
        .map(|s| {
            s.into_inner()
                .unwrap_or_else(|p| p.into_inner())
                .expect("every index of a join is done")
        })
        .collect()
}

/// The ranges of `0..len` in pieces of `chunk` (the last one shorter): how a caller splits rows.
pub fn chunks(len: usize, chunk: usize) -> impl Iterator<Item = std::ops::Range<usize>> {
    let chunk = chunk.max(1);
    (0..len.div_ceil(chunk)).map(move |c| c * chunk..((c + 1) * chunk).min(len))
}

struct Slot<T> {
    done: AtomicBool,
    value: Mutex<Option<T>>,
}

/// The result of a [`submit`]ted job, taken when it is ready.
pub struct Pending<T> {
    slot: Arc<Slot<T>>,
}

impl<T> Pending<T> {
    /// Whether the job is done.
    pub fn is_done(&self) -> bool {
        self.slot.done.load(Ordering::Acquire)
    }

    /// The result once the job is done (once; `None` after).
    pub fn try_take(&mut self) -> Option<T> {
        if !self.is_done() {
            return None;
        }
        // The worker let the lock go before it set `done`.
        match self.slot.value.try_lock() {
            Ok(mut v) => v.take(),
            Err(TryLockError::Poisoned(p)) => p.into_inner().take(),
            Err(TryLockError::WouldBlock) => None,
        }
    }

    /// The result, now: the caller runs queued jobs (its own among them) while it waits, and spins.
    /// Panics when the job panicked (or its result was taken already).
    pub fn wait(mut self) -> T {
        loop {
            if self.is_done() {
                return self.try_take().expect("the job of the pool panicked");
            }
            match try_pop() {
                // The queue is shared: the task may be another caller's (a fork-join helper whose index panics, a
                // job of a `submit`). Its panic belongs to that caller (the join's flag, the empty slot), not to this
                // waiter, as on a worker (`worker_main`); natively it unwinds, the wasm builds abort anyway.
                Some(task) => {
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(task));
                }
                None => {
                    check_alive();
                    std::hint::spin_loop();
                }
            }
        }
    }
}

/// Sets `done` when the job ends, also by a panic (then the slot stays empty).
struct SlotDone<T>(Arc<Slot<T>>);

impl<T> Drop for SlotDone<T> {
    fn drop(&mut self) {
        self.0.done.store(true, Ordering::Release);
    }
}

/// Runs `f` on a worker; without workers on the caller, now (the result is ready at once).
pub fn submit<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Pending<T> {
    let slot = Arc::new(Slot {
        done: AtomicBool::new(false),
        value: Mutex::new(None),
    });
    let run = {
        let done = SlotDone(slot.clone());
        move || {
            let v = f();
            *done.0.value.lock().unwrap_or_else(|p| p.into_inner()) = Some(v);
            drop(done);
        }
    };
    if workers() == 0 {
        run();
    } else {
        POOL.pooled.fetch_add(1, Ordering::Relaxed);
        push([Box::new(run) as Task]);
    }
    Pending { slot }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Once;
    use std::time::Duration;

    fn pool() {
        static START: Once = Once::new();
        START.call_once(|| start_native_workers(3));
    }

    /// Runs `body` on a thread and fails the test if it takes over 60 s (a hung pool spins a core).
    fn watchdog(body: impl FnOnce() + Send + 'static) {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(body));
            let _ = tx.send(r);
        });
        match rx.recv_timeout(Duration::from_secs(60)) {
            Ok(Ok(())) => {}
            Ok(Err(p)) => std::panic::resume_unwind(p),
            Err(_) => {
                eprintln!("jobs test hung for 60 s: aborting");
                std::process::abort();
            }
        }
    }

    fn work(i: usize) -> u64 {
        (0..(i as u64 % 97) * 50).fold(i as u64, |a, x| a.wrapping_mul(31).wrapping_add(x))
    }

    fn wsum(v: impl IntoIterator<Item = u64>) -> u64 {
        v.into_iter().fold(0, u64::wrapping_add)
    }

    #[test]
    fn chunks_cover_the_range() {
        let c: Vec<_> = chunks(10, 4).collect();
        assert_eq!(c, [0..4, 4..8, 8..10]);
        assert_eq!(chunks(0, 4).count(), 0);
        assert_eq!(chunks(3, 0).collect::<Vec<_>>(), [0..1, 1..2, 2..3]);
    }

    #[test]
    fn a_map_on_the_pool_is_the_loop_on_one_thread() {
        watchdog(|| {
            let inline: Vec<u64> = (0..500).map(work).collect();
            pool();
            assert!(workers() >= 3);
            for _ in 0..20 {
                assert_eq!(map(500, work), inline);
            }
            assert!(pooled_total() > 0);
        });
    }

    #[test]
    fn every_index_is_done_once() {
        watchdog(|| {
            pool();
            let hits: Vec<AtomicUsize> = (0..1000).map(|_| AtomicUsize::new(0)).collect();
            for_each(1000, &|i| {
                hits[i].fetch_add(1, Ordering::Relaxed);
            });
            assert!(hits.iter().all(|h| h.load(Ordering::Relaxed) == 1));
        });
    }

    #[test]
    fn nested_joins_finish() {
        watchdog(|| {
            pool();
            let out = map(8, |i| wsum(map(50, move |j| work(i * 50 + j))));
            let want: Vec<u64> = (0..8)
                .map(|i| wsum((0..50).map(|j| work(i * 50 + j))))
                .collect();
            assert_eq!(out, want);
        });
    }

    #[test]
    fn a_panicking_job_fails_its_caller_and_does_not_hang() {
        watchdog(|| {
            pool();
            let r = std::panic::catch_unwind(|| {
                for_each(200, &|i| assert!(i != 137, "index 137"));
            });
            assert!(r.is_err());
            let r = std::panic::catch_unwind(|| submit(|| -> u32 { panic!("job") }).wait());
            assert!(r.is_err());
            // The workers are still there.
            assert_eq!(map(100, work), (0..100).map(work).collect::<Vec<_>>());
        });
    }

    /// A waiter that pops a task of another caller runs it under `catch_unwind`: the panic of that task is not its
    /// own (this made `submitted_jobs_hand_back_their_results` fail one run in eight, when it ran beside the
    /// panicking test).
    #[test]
    fn a_waiter_that_runs_a_foreign_panicking_task_does_not_fail() {
        watchdog(|| {
            pool();
            for k in 0..100u64 {
                fn boom() {
                    panic!("a task of another caller");
                }
                push([Box::new(boom) as Task]);
                assert_eq!(submit(move || work(k as usize)).wait(), work(k as usize));
            }
        });
    }

    #[test]
    fn submitted_jobs_hand_back_their_results() {
        watchdog(|| {
            pool();
            let jobs: Vec<_> = (0..64).map(|i| submit(move || work(i))).collect();
            let got: Vec<u64> = jobs.into_iter().map(Pending::wait).collect();
            assert_eq!(got, (0..64).map(work).collect::<Vec<_>>());
            let mut p = submit(|| 7);
            while !p.is_done() {
                std::hint::spin_loop();
            }
            assert_eq!(p.try_take(), Some(7));
            assert_eq!(p.try_take(), None);
        });
    }
}
