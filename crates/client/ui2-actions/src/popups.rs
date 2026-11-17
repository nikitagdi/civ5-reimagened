//! Popup windows follow the authoritative HUD queue head.
use protocol::{Command, CommandOutcome};
use ui2_model::{ModelStore, generated::HudPopup};

use crate::{ActionEvent, OrderSink, UiActions};

pub(crate) struct PopupSession {
    pub id: i32,
    screen: String,
    dismiss: Option<u32>,
    pub finished: bool,
}

impl UiActions {
    pub(crate) fn clear_popup(&mut self) {
        if let Some(old) = self.popup.take()
            && let Some(w) = self.state.edit().windows.remote_mut(&old.screen)
            && w.popup_id == Some(old.id)
        {
            w.open = false;
            w.popup_id = None;
            w.popup_decision = false;
        }
    }

    /// Apply a schema-validated queue head, or clear popup state on disconnect.
    pub fn sync_popup(
        &mut self,
        head: Option<&HudPopup>,
        store: &mut ModelStore,
        sink: &mut dyn OrderSink,
        now_ms: u64,
    ) {
        if self.popup.as_ref().map(|p| p.id) != head.map(|p| p.id) {
            self.clear_popup();
            if let Some(head) = head
                && let Some(w) = self.state.edit().windows.remote_mut(&head.screen)
            {
                w.open = true;
                w.a = head.a;
                w.b = head.b;
                w.popup_id = Some(head.id);
                w.popup_decision = head.decision;
                self.popup = Some(PopupSession {
                    id: head.id,
                    screen: head.screen.to_string(),
                    dismiss: None,
                    finished: false,
                });
            }
        }
        if let (Some(head), Some(popup)) = (head, &mut self.popup)
            && !head.decision
            && !popup.finished
            && popup.dismiss.is_none()
            && self
                .state
                .ui()
                .windows
                .remote(&popup.screen)
                .is_some_and(|w| !w.open)
            && let Some(turn) = crate::validate::game_turn(store)
        {
            match sink.command(
                Command::ScreenAction {
                    screen: "hud".into(),
                    action: "popup_done".into(),
                    args: vec![("a".into(), i64::from(head.id))],
                    text: None,
                },
                turn,
            ) {
                Ok(id) => {
                    popup.dismiss = Some(id);
                    self.state
                        .track(id, "screen.act", "hud".into(), None, now_ms);
                }
                Err(error) => {
                    if let Some(w) = self.state.edit().windows.remote_mut(&popup.screen) {
                        w.open = true;
                    }
                    self.state.toast(
                        ui2_model::generated::ToastKind::Refused,
                        error.why(),
                        None,
                        now_ms,
                    );
                }
            }
        }
        self.publish(store);
    }

    pub(crate) fn popup_outcome(&mut self, event: &ActionEvent) {
        let Some(popup) = self.popup.as_mut() else {
            return;
        };
        let (id, success) = match event {
            ActionEvent::Answered {
                id,
                outcome: CommandOutcome::Applied | CommandOutcome::ScreenApplied { .. },
            } => (*id, true),
            ActionEvent::Answered {
                id,
                outcome: CommandOutcome::Rejected { .. } | CommandOutcome::Lost,
            }
            | ActionEvent::RolledBack { id, .. } => (*id, false),
            _ => return,
        };
        if popup.dismiss == Some(id) {
            popup.finished = success;
            popup.dismiss = None;
            if !success && let Some(w) = self.state.edit().windows.remote_mut(&popup.screen) {
                w.open = true;
            }
        }
    }
}
