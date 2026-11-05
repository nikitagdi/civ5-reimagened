//! The relay of `netproxy`: accepts the connections of the clients, opens one to the upstream for each and
//! moves the bytes through the channel model (`model.rs`). Each connection has four small tasks: a reader and a
//! writer per direction; the reader stamps every chunk with its release time, the writer lets it out then.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering::Relaxed};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, watch};
use tokio::time::{Instant, sleep_until};

use super::model::{DELAY_EDGES_MS, Profile, Rng, Shaper, delay_bucket};

/// `Up` goes from the client to the upstream, `Down` back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    Up,
    Down,
}

impl Dir {
    pub fn index(self) -> usize {
        self as usize
    }
    pub fn parse(s: &str) -> Option<Dir> {
        match s {
            "up" => Some(Dir::Up),
            "down" => Some(Dir::Down),
            _ => None,
        }
    }
}

/// Which socket of a connection is closed with a reset (RST).
pub const SIDE_CLIENT: u8 = 1;
pub const SIDE_UPSTREAM: u8 = 2;

const BUCKETS: usize = DELAY_EDGES_MS.len() + 1;

#[derive(Default)]
pub struct Stats {
    pub conns_total: AtomicU64,
    pub conns_open: AtomicU64,
    pub resets: AtomicU64,
    pub bytes: [AtomicU64; 2],
    /// Chunks waiting for their release time.
    pub queued: [AtomicUsize; 2],
    pub chunks: [AtomicU64; 2],
    pub delay_hist: [[AtomicU64; BUCKETS]; 2],
    pub delay_max_ms: [AtomicU64; 2],
}

#[derive(Default)]
struct Hold {
    blackhole_until: Option<Instant>,
    half_open_until: [Option<Instant>; 2],
}

/// Everything the control API and the relay share.
pub struct Shared {
    pub upstream: SocketAddr,
    pub seed: u64,
    pub profile: RwLock<Profile>,
    pub stats: Stats,
    hold: Mutex<Hold>,
    conns: Mutex<HashMap<u64, watch::Sender<u8>>>,
    next_id: AtomicU64,
}

impl Shared {
    pub fn new(upstream: SocketAddr, seed: u64) -> Arc<Shared> {
        Arc::new(Shared {
            upstream,
            seed,
            profile: RwLock::new(Profile::default()),
            stats: Stats::default(),
            hold: Mutex::new(Hold::default()),
            conns: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
        })
    }

    /// Nothing is delivered in both directions for `ms`; then the queue comes out in order.
    pub fn blackhole(&self, ms: u64) {
        self.hold.lock().unwrap().blackhole_until =
            Some(Instant::now() + Duration::from_millis(ms));
    }

    /// One direction is silent for `ms`.
    pub fn half_open(&self, dir: Dir, ms: u64) {
        self.hold.lock().unwrap().half_open_until[dir.index()] =
            Some(Instant::now() + Duration::from_millis(ms));
    }

    fn hold_until(&self, dir: Dir) -> Option<Instant> {
        let h = self.hold.lock().unwrap();
        match (h.blackhole_until, h.half_open_until[dir.index()]) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        }
    }

    /// RST on the connection `conn` (or on all) at the given sides (bits `SIDE_*`). Returns how many were hit.
    pub fn reset(&self, conn: Option<u64>, sides: u8) -> usize {
        let conns = self.conns.lock().unwrap();
        let mut n = 0;
        for (id, tx) in conns.iter() {
            if conn.is_none_or(|c| c == *id) {
                let _ = tx.send(sides);
                n += 1;
            }
        }
        self.stats.resets.fetch_add(n as u64, Relaxed);
        n
    }

    pub fn open_ids(&self) -> Vec<u64> {
        let mut v: Vec<u64> = self.conns.lock().unwrap().keys().copied().collect();
        v.sort_unstable();
        v
    }
}

enum Item {
    Data {
        arrived: Instant,
        release: Instant,
        data: Vec<u8>,
    },
    Fin {
        release: Instant,
    },
}

/// Accepts until the task is dropped.
pub async fn accept_loop(listener: TcpListener, shared: Arc<Shared>) {
    loop {
        let Ok((client, _)) = listener.accept().await else {
            continue;
        };
        let _ = client.set_nodelay(true);
        let id = shared.next_id.fetch_add(1, Relaxed);
        tokio::spawn(serve_conn(shared.clone(), id, client));
    }
}

async fn serve_conn(shared: Arc<Shared>, id: u64, client: TcpStream) {
    let (stop_tx, stop_rx) = watch::channel(0u8);
    shared.conns.lock().unwrap().insert(id, stop_tx);
    shared.stats.conns_total.fetch_add(1, Relaxed);
    shared.stats.conns_open.fetch_add(1, Relaxed);
    run_conn(&shared, id, client, stop_rx).await;
    shared.conns.lock().unwrap().remove(&id);
    shared.stats.conns_open.fetch_sub(1, Relaxed);
}

async fn run_conn(shared: &Arc<Shared>, id: u64, client: TcpStream, mut stop: watch::Receiver<u8>) {
    // The handshake of the model: no byte moves before this time.
    let connect_delay = shared.profile.read().unwrap().connect_delay();
    tokio::select! {
        _ = tokio::time::sleep(Duration::from_millis(u64::from(connect_delay))) => {}
        _ = stop.changed() => {
            if *stop.borrow() & SIDE_CLIENT != 0 {
                let _ = client.set_zero_linger();
            }
            return;
        }
    }
    let Ok(upstream) = TcpStream::connect(shared.upstream).await else {
        // Nothing behind the proxy: the client sees the close.
        return;
    };
    let _ = upstream.set_nodelay(true);
    let (cr, cw) = client.into_split();
    let (ur, uw) = upstream.into_split();
    let mut tasks = Vec::new();
    for (dir, r, w, rside, wside) in [
        (Dir::Up, cr, uw, SIDE_CLIENT, SIDE_UPSTREAM),
        (Dir::Down, ur, cw, SIDE_UPSTREAM, SIDE_CLIENT),
    ] {
        let (tx, rx) = mpsc::unbounded_channel();
        let seed = shared.seed ^ id.wrapping_mul(0x9E37_79B9) ^ ((dir.index() as u64 + 1) << 56);
        tasks.push(tokio::spawn(reader(
            shared.clone(),
            dir,
            r,
            tx,
            stop.clone(),
            rside,
            seed,
        )));
        tasks.push(tokio::spawn(writer(
            shared.clone(),
            dir,
            w,
            rx,
            stop.clone(),
            wside,
        )));
    }
    for t in tasks {
        let _ = t.await;
    }
}

fn apply_reset(sock: &TcpStream, mask: u8, side: u8) {
    if mask & side != 0 {
        let _ = sock.set_zero_linger();
    }
}

/// The write half on a stop: dropping it would send a FIN before the RST, so it is forgotten; the socket closes
/// with the read half.
fn stop_writer(w: OwnedWriteHalf, mask: u8, side: u8) {
    apply_reset(w.as_ref(), mask, side);
    w.forget();
}

async fn reader(
    shared: Arc<Shared>,
    dir: Dir,
    mut r: OwnedReadHalf,
    tx: mpsc::UnboundedSender<Item>,
    mut stop: watch::Receiver<u8>,
    side: u8,
    seed: u64,
) {
    let start = Instant::now();
    let mut shaper = Shaper::default();
    let mut rng = Rng::new(seed);
    let mut buf = vec![0u8; 8192];
    loop {
        let n = tokio::select! {
            n = r.read(&mut buf) => n.unwrap_or(0),
            _ = stop.changed() => {
                apply_reset(r.as_ref(), *stop.borrow(), side);
                return;
            }
        };
        let now = Instant::now();
        let now_us = now.duration_since(start).as_micros() as u64;
        let profile = shared.profile.read().unwrap().clone();
        let release =
            start + Duration::from_micros(shaper.schedule(now_us, n.max(1), &profile, &mut rng));
        if n == 0 {
            let _ = tx.send(Item::Fin { release });
            return;
        }
        shared.stats.queued[dir.index()].fetch_add(1, Relaxed);
        if tx
            .send(Item::Data {
                arrived: now,
                release,
                data: buf[..n].to_vec(),
            })
            .is_err()
        {
            return;
        }
    }
}

/// Sleeps until `t`. The timer of tokio steps by a millisecond and rounds up, which would add about a millisecond
/// to every hop of the channel; so the last millisecond is waited for by yielding.
async fn sleep_precise(t: Instant) {
    const SLACK: Duration = Duration::from_millis(1);
    let now = Instant::now();
    if t <= now {
        return;
    }
    if t - now > SLACK {
        sleep_until(t - SLACK).await;
    }
    while Instant::now() < t {
        tokio::task::yield_now().await;
    }
}

/// Waits for the release time and for the end of a blackhole or of a silence of this direction.
async fn wait_release(shared: &Shared, dir: Dir, release: Instant) {
    sleep_precise(release).await;
    while let Some(t) = shared.hold_until(dir) {
        if t <= Instant::now() {
            break;
        }
        sleep_precise(t).await;
    }
}

async fn writer(
    shared: Arc<Shared>,
    dir: Dir,
    mut w: OwnedWriteHalf,
    mut rx: mpsc::UnboundedReceiver<Item>,
    mut stop: watch::Receiver<u8>,
    side: u8,
) {
    let d = dir.index();
    loop {
        let item = tokio::select! {
            i = rx.recv() => i,
            _ = stop.changed() => {
                let mask = *stop.borrow();
                stop_writer(w, mask, side);
                return;
            }
        };
        let Some(item) = item else {
            // The reader ended: on a reset it ended because of the stop, which has to be honoured here as well
            // (a plain drop of the write half would send a FIN before the RST).
            let mask = *stop.borrow();
            if mask != 0 {
                stop_writer(w, mask, side);
            }
            return;
        };
        match item {
            Item::Data {
                arrived,
                release,
                data,
            } => {
                tokio::select! {
                    _ = wait_release(&shared, dir, release) => {}
                    _ = stop.changed() => {
                        let mask = *stop.borrow();
                stop_writer(w, mask, side);
                        return;
                    }
                }
                let added = Instant::now().duration_since(arrived).as_millis() as u64;
                let len = data.len() as u64;
                if w.write_all(&data).await.is_err() {
                    return;
                }
                let s = &shared.stats;
                s.queued[d].fetch_sub(1, Relaxed);
                s.bytes[d].fetch_add(len, Relaxed);
                s.chunks[d].fetch_add(1, Relaxed);
                s.delay_hist[d][delay_bucket(added)].fetch_add(1, Relaxed);
                s.delay_max_ms[d].fetch_max(added, Relaxed);
            }
            Item::Fin { release } => {
                tokio::select! {
                    _ = wait_release(&shared, dir, release) => {}
                    _ = stop.changed() => {
                        let mask = *stop.borrow();
                stop_writer(w, mask, side);
                        return;
                    }
                }
                let _ = w.shutdown().await;
                return;
            }
        }
    }
}
