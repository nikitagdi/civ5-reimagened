//! `netproxy`: a TCP relay with a channel in the middle: delay, jitter, bandwidth, a
//! model of loss, a blackhole, resets. One proxy per server; bots and browsers connect to one port. The
//! control API is HTTP JSON on its own port.

pub mod control;
pub mod model;
pub mod relay;

use std::net::SocketAddr;
use std::sync::Arc;

use tokio::net::TcpListener;
use tokio::task::JoinHandle;

pub use model::Profile;
pub use relay::Shared;

/// The timers of Windows step by about 15 ms; a finer step makes the delays closer to the profile.
#[cfg(windows)]
fn fine_timer() {
    #[link(name = "winmm")]
    unsafe extern "system" {
        fn timeBeginPeriod(period: u32) -> u32;
    }
    // SAFETY: a plain system call with a number; the matching end is the end of the process.
    unsafe {
        timeBeginPeriod(1);
    }
}

#[cfg(not(windows))]
fn fine_timer() {}

/// A running proxy. Dropping it does not stop it; `stop()` does.
pub struct Proxy {
    pub listen: SocketAddr,
    pub control: SocketAddr,
    pub shared: Arc<Shared>,
    tasks: Vec<JoinHandle<()>>,
}

impl Proxy {
    pub fn stop(self) {
        for t in &self.tasks {
            t.abort();
        }
    }
}

/// Binds both ports (port 0 picks a free one) and starts the relay and the control API.
pub async fn start(
    listen: SocketAddr,
    upstream: SocketAddr,
    control: SocketAddr,
    seed: u64,
) -> std::io::Result<Proxy> {
    fine_timer();
    let l = TcpListener::bind(listen).await?;
    let c = TcpListener::bind(control).await?;
    let shared = Shared::new(upstream, seed);
    let (listen, control) = (l.local_addr()?, c.local_addr()?);
    let app = control::router(shared.clone());
    let t1 = tokio::spawn(relay::accept_loop(l, shared.clone()));
    let t2 = tokio::spawn(async move {
        let _ = axum::serve(c, app).await;
    });
    Ok(Proxy {
        listen,
        control,
        shared,
        tasks: vec![t1, t2],
    })
}
