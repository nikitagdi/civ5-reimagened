//! `GameHost` seam (§5.8). TODO: the thread should own `Game` once gamecore has it.

use std::sync::mpsc::{self, Sender};
use std::thread::JoinHandle;

#[derive(Debug)]
pub enum HostMsg {
    Shutdown,
}

pub struct GameHost {
    tx: Sender<HostMsg>,
    thread: Option<JoinHandle<()>>,
}

impl GameHost {
    pub fn spawn() -> Self {
        let (tx, rx) = mpsc::channel::<HostMsg>();
        let thread = std::thread::Builder::new()
            .name("game-host".into())
            .spawn(move || {
                while let Ok(msg) = rx.recv() {
                    if matches!(msg, HostMsg::Shutdown) {
                        break;
                    }
                }
            })
            .ok();
        if thread.is_none() {
            tracing::error!("failed to start the game-host thread");
        }
        Self { tx, thread }
    }

    /// `false` if the thread has already exited.
    pub fn send(&self, msg: HostMsg) -> bool {
        self.tx.send(msg).is_ok()
    }
}

impl Drop for GameHost {
    fn drop(&mut self) {
        let _ = self.tx.send(HostMsg::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_and_drop_joins() {
        let host = GameHost::spawn();
        assert!(host.send(HostMsg::Shutdown));
        drop(host);
    }
}
