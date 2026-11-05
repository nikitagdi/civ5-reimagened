//! The queue of the notification sounds (re-b15 ITEM 5 P5.1): the exe never lets two of them overlap. A new
//! notification's sound joins a queue unless it equals the last one queued (`PushNotificationSound`); once a whole second
//! (`cvAudioSimMainGame::Update` polls `OnNotificationUpdate` when the integer second changes) the next one plays, if the
//! one before has finished.
//!
//! SIMPLIFIED(SIMP-exe-1204): the exe drops (does not delay) a notification popped while a leader screen is open and plays
//! none once the game is over; the port's queue knows neither state and plays them.

use std::collections::VecDeque;

#[derive(Debug, Default)]
pub struct NotificationQueue {
    queue: VecDeque<String>,
    /// The script of the last sound played from the queue.
    current: Option<String>,
    /// The whole second of the last poll.
    last_second: Option<u64>,
}

impl NotificationQueue {
    /// A notification of type `id` arrived (`NOTIFICATION_GOODY`); a type equal to the tail of the queue is dropped.
    pub fn push(&mut self, id: &str) {
        if self.queue.back().map(String::as_str) != Some(id) {
            self.queue.push_back(id.to_owned());
        }
    }

    pub fn len(&self) -> usize {
        self.queue.len()
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// The next notification type whose sound plays now: only on the first call of a whole second of `now_ms`, and only
    /// when the script given to [`Self::playing`] no longer sounds (`sounding`).
    pub fn poll(&mut self, now_ms: u64, sounding: impl Fn(&str) -> bool) -> Option<String> {
        let second = now_ms / 1000;
        if self.last_second == Some(second) {
            return None;
        }
        self.last_second = Some(second);
        if let Some(s) = &self.current {
            if sounding(s) {
                return None;
            }
            self.current = None;
        }
        self.queue.pop_front()
    }

    /// The script that started for the type [`Self::poll`] gave (`None`: it made no sound).
    pub fn playing(&mut self, script: Option<String>) {
        self.current = script;
    }

    /// Everything queued is dropped (the game is left).
    pub fn clear(&mut self) {
        self.queue.clear();
        self.current = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notifications_sound_one_after_another_on_whole_seconds() {
        let mut q = NotificationQueue::default();
        q.push("N_TECH");
        q.push("N_POLICY");
        q.push("N_POLICY");
        q.push("N_TECH");
        assert_eq!(q.len(), 3, "only a repeat of the tail is dropped");
        let mut played = Vec::new();
        let mut sounding_until = 0u64;
        // Every frame (16 ms) for six seconds; each sound lasts 1.5 s.
        for now in (0..6000u64).step_by(16) {
            if let Some(n) = q.poll(now, |_| now < sounding_until) {
                played.push((now, n));
                q.playing(Some("AS2D_X".into()));
                sounding_until = now + 1500;
            }
        }
        let at: Vec<u64> = played.iter().map(|p| p.0 / 1000).collect();
        let ids: Vec<&str> = played.iter().map(|p| p.1.as_str()).collect();
        assert_eq!(ids, ["N_TECH", "N_POLICY", "N_TECH"]);
        // The second waits for the first to end (1.5 s): it starts on the tick of second 2, the third on second 4.
        assert_eq!(at, [0, 2, 4]);
    }

    #[test]
    fn a_silent_notification_does_not_hold_the_next_one() {
        let mut q = NotificationQueue::default();
        q.push("N_A");
        q.push("N_B");
        assert_eq!(q.poll(0, |_| true).as_deref(), Some("N_A"));
        q.playing(None);
        assert_eq!(q.poll(500, |_| true), None, "the same second");
        assert_eq!(q.poll(1000, |_| true).as_deref(), Some("N_B"));
        q.push("N_C");
        q.clear();
        assert_eq!(q.poll(5000, |_| false), None);
    }
}
