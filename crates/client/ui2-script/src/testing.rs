//! A worker host for tests: it records what was posted and answers with what the test queues. Clones share
//! the state, so a test keeps a handle after giving one to the supervisor.

use std::cell::RefCell;
use std::rc::Rc;

use platform::{ScriptHost, ScriptHostError, ScriptHostEvent, WorkerId};

#[derive(Debug, Default)]
struct State {
    next: u32,
    live: Vec<WorkerId>,
    terminated: Vec<WorkerId>,
    posted: Vec<(WorkerId, String)>,
    inbox: Vec<(WorkerId, ScriptHostEvent)>,
    refuse_spawn: bool,
}

/// A host without workers: posted texts are kept, replies are queued by the test.
#[derive(Clone, Debug, Default)]
pub struct MockScriptHost {
    state: Rc<RefCell<State>>,
}

impl MockScriptHost {
    #[must_use]
    pub fn new() -> MockScriptHost {
        MockScriptHost::default()
    }

    /// Make every following `spawn` fail (as on a page without workers).
    pub fn refuse_spawn(&self, refuse: bool) {
        self.state.borrow_mut().refuse_spawn = refuse;
    }

    /// The workers started and not terminated, oldest first.
    #[must_use]
    pub fn live(&self) -> Vec<WorkerId> {
        self.state.borrow().live.clone()
    }

    /// The workers terminated, in order.
    #[must_use]
    pub fn terminated(&self) -> Vec<WorkerId> {
        self.state.borrow().terminated.clone()
    }

    /// Everything posted so far, and forget it.
    pub fn take_posted(&self) -> Vec<(WorkerId, String)> {
        std::mem::take(&mut self.state.borrow_mut().posted)
    }

    /// Queue a message from a worker for the next drain.
    pub fn reply(&self, worker: WorkerId, text: &str) {
        self.state
            .borrow_mut()
            .inbox
            .push((worker, ScriptHostEvent::Message(text.to_string())));
    }

    /// Queue an error of a worker for the next drain.
    pub fn error(&self, worker: WorkerId, message: &str) {
        self.state
            .borrow_mut()
            .inbox
            .push((worker, ScriptHostEvent::Error(message.to_string())));
    }
}

impl ScriptHost for MockScriptHost {
    fn spawn(&mut self) -> Result<WorkerId, ScriptHostError> {
        let mut s = self.state.borrow_mut();
        if s.refuse_spawn {
            return Err(ScriptHostError::Unsupported);
        }
        let id = WorkerId(s.next);
        s.next += 1;
        s.live.push(id);
        Ok(id)
    }

    fn post(&mut self, worker: WorkerId, text: &str) -> Result<(), ScriptHostError> {
        let mut s = self.state.borrow_mut();
        if !s.live.contains(&worker) {
            return Err(ScriptHostError::NoWorker(worker));
        }
        s.posted.push((worker, text.to_string()));
        Ok(())
    }

    fn drain(&mut self, out: &mut Vec<(WorkerId, ScriptHostEvent)>) {
        let mut s = self.state.borrow_mut();
        let live = s.live.clone();
        out.extend(s.inbox.drain(..).filter(|(w, _)| live.contains(w)));
    }

    fn terminate(&mut self, worker: WorkerId) {
        let mut s = self.state.borrow_mut();
        if let Some(i) = s.live.iter().position(|w| *w == worker) {
            s.live.remove(i);
            s.terminated.push(worker);
            s.inbox.retain(|(w, _)| *w != worker);
        }
    }
}
