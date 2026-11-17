//! Built-in screen registry and worker-local Lua execution.
pub(crate) mod localization;
mod runtime;
pub(crate) use runtime::Screens;

/// Names and sources compiled into the server. Add one entry per screen.
pub const REGISTRY: &[(&str, &str)] = &[
    ("techs", include_str!("techs.lua")),
    ("hud", include_str!("hud.lua")),
];

use crate::core::Refusal;
use crate::{Core, Worker};
use protocol::{Command, PlayerId};

impl<C: Core> Worker<C> {
    pub(crate) fn screen_runtime(&mut self) -> Result<&Screens, String> {
        if self.screens.is_none() {
            let state = Screens::new().map_err(|e| e.to_string())?;
            self.core
                .attach_screen_lua(state.lua())
                .map_err(|e| e.to_string())?;
            state.load().map_err(|e| e.to_string())?;
            self.screens = Some(state);
        }
        self.screens
            .as_ref()
            .ok_or_else(|| "screen interpreter is unavailable".into())
    }

    /// Inject a full popup through the same mapping and queue path as native events.
    /// Intended for embeddings and fixtures, never exposed as a client command.
    pub fn inject_popup(
        &mut self,
        player: PlayerId,
        popup: crate::popups::Popup,
    ) -> Result<Option<i32>, String> {
        if !self.seats.contains_key(&player) {
            return Err("popup seat is unavailable".into());
        }
        let id = self.popups.next_id()?;
        let fallback;
        let mapping = match self.screens.as_ref() {
            Some(runtime) => runtime,
            None => {
                fallback = Screens::new().map_err(|e| e.to_string())?;
                fallback.load().map_err(|e| e.to_string())?;
                &fallback
            }
        };
        let Some(head) = mapping.map_popup(&popup, id).map_err(|e| e.to_string())? else {
            self.popups.unknown(&popup.kind);
            return Ok(None);
        };
        self.popups.push(player, popup, head).map(Some)
    }

    pub(crate) fn screen_data(
        &mut self,
        player: PlayerId,
        screen: &str,
        args: &[(String, i64)],
    ) -> Result<String, String> {
        let popup = self
            .popups
            .head(player)
            .filter(|e| e.head.screen == screen)
            .cloned();
        let result = self.screen_runtime().and_then(|runtime| {
            runtime.context(popup.as_ref()).map_err(|e| e.to_string())?;
            runtime
                .data(player, screen, args)
                .map_err(|e| e.to_string())
        });
        let result = result.and_then(|json| {
            if screen != "hud" {
                return Ok(json);
            }
            let mut value: serde_json::Value =
                serde_json::from_str(&json).map_err(|e| e.to_string())?;
            value["popup"] = serde_json::to_value(self.popups.head(player).map(|e| &e.head))
                .map_err(|e| e.to_string())?;
            value["popup_count"] = self.popups.len(player).into();
            let json = serde_json::to_string(&value).map_err(|e| e.to_string())?;
            if json.len() > 512 * 1024 {
                return Err("screen response exceeds 512 KiB".into());
            }
            Ok(json)
        });
        if let Err(reason) = &result {
            tracing::warn!(screen, %reason, "screen query failed");
        }
        result
    }

    pub(crate) fn screen_or_core_command(
        &mut self,
        player: PlayerId,
        cmd: &Command,
    ) -> Result<(), Refusal> {
        self.screen_effect = None;
        let Command::ScreenAction {
            screen,
            action,
            args,
            text,
        } = cmd
        else {
            return self.core.command(player, cmd);
        };
        let result = (|| {
            if screen == "hud" && action == "popup_done" {
                let [(name, id)] = args.as_slice() else {
                    return Err("popup dismissal requires one id".into());
                };
                if name != "a" || text.is_some() {
                    return Err("invalid popup dismissal arguments".into());
                }
                let id = i32::try_from(*id).map_err(|_| "invalid popup id")?;
                return self.popups.done(player, id, None);
            }
            let popup = self
                .popups
                .head(player)
                .filter(|e| e.head.screen == *screen)
                .cloned();
            if let Some((_, id)) = args.iter().find(|(key, _)| key == "popup_id")
                && popup.as_ref().is_none_or(|p| i64::from(p.head.id) != *id)
            {
                return Err("stale popup action".into());
            }
            let runtime = self.screen_runtime()?;
            runtime.context(popup.as_ref()).map_err(|e| e.to_string())?;
            let output = runtime
                .action(player, screen, action, args, text.as_deref())
                .map_err(|e| e.to_string())?;
            if let Some(id) = output.done {
                self.popups.validate_done(player, id, Some(screen))?;
            }
            if !output.messages.is_empty() {
                self.core
                    .replica()
                    .ok_or("screen actions are unavailable for this core")?
                    .net_submit(player, &gamecore::replica::encode_net(&output.messages))?;
            }
            if let Some(id) = output.done {
                self.popups.done(player, id, Some(screen))?;
            }
            if output.open.is_some() || output.close || output.done.is_some() {
                self.screen_effect = Some(protocol::CommandOutcome::ScreenApplied {
                    screen: screen.clone(),
                    open: output.open,
                    popup_done: output.done,
                    close: output.close || output.done.is_some(),
                });
            }
            Ok(())
        })();
        if let Err(reason) = &result {
            tracing::warn!(screen, action, %reason, "screen action failed");
        }
        result.map_err(Refusal::other)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::FixtureCore;
    use crate::popups::Popup;

    fn init() -> protocol::WorkerInit {
        use protocol::*;
        WorkerInit {
            ipc: IPC_VERSION,
            game: 1,
            core: CoreKind::Fixture {
                huge: false,
                wrap_x: false,
            },
            save_dir: String::new(),
            seed: 1,
            cfg: WorkerConfig::default(),
            start: Start::New(GameSetup {
                settings: GameSettings {
                    name: "popup fixture".into(),
                    world_size: "WORLDSIZE_DUEL".into(),
                    map_script: "fixture.lua".into(),
                    map_options: vec![],
                    speed: "GAMESPEED_STANDARD".into(),
                    start_era: "ERA_ANCIENT".into(),
                    handicap: "HANDICAP_PRINCE".into(),
                    minor_civs: 0,
                    turn_mode: TurnMode::Simultaneous,
                    timer: TimerMode::Off,
                    game_options: vec![],
                    seed: Some(1),
                    map_seed: None,
                    auto_choices: AutoChoices::default(),
                    diplo_auto_decline: true,
                    mods: vec![],
                },
                slots: (0..2)
                    .map(|player| SlotSetup {
                        player,
                        civ: CivPick::Random,
                        control: SlotControl::Human {
                            nick: format!("fixture-{player}"),
                        },
                    })
                    .collect(),
            }),
        }
    }

    #[test]
    fn worker_injection_hud_and_completion_are_seat_scoped_and_transactional() {
        let (mut worker, _) = Worker::new(FixtureCore::new(false, false), init(), 0);
        for i in 1..10 {
            if worker.phase != protocol::TurnPhase::Starting {
                break;
            }
            worker.step(i);
        }
        let runtime = Screens::new().unwrap();
        runtime.load().unwrap();
        runtime.lua().exec(r#"
            __screens.hud.data = function() return {} end
            __screens.techs.data = function(player, args) return {id = args.popup_id, flag = args.popup.flag} end
            __screens.techs.actions.done = function(player, args, net) net.popup_done(args.popup_id) end
            __screens.techs.actions.fail = function(player, args, net) net.popup_done(args.popup_id); error('failed') end
            __screens.techs.actions.refused = function(player, args, net) net.popup_done(args.popup_id); net.send('sendResearch', 1) end
        "#, "=popup-worker").unwrap();
        worker.screens = Some(runtime);
        let id = worker
            .inject_popup(
                0,
                Popup {
                    kind: "BUTTONPOPUP_CHOOSETECH".into(),
                    flag: 6,
                    ..Default::default()
                },
            )
            .unwrap()
            .unwrap();
        assert!(
            worker
                .inject_popup(
                    1,
                    Popup {
                        kind: "UNKNOWN".into(),
                        ..Default::default()
                    }
                )
                .unwrap()
                .is_none()
        );
        let hud = |worker: &mut Worker<FixtureCore>, player| -> serde_json::Value {
            serde_json::from_str(&worker.screen_data(player, "hud", &[]).unwrap()).unwrap()
        };
        assert_eq!(hud(&mut worker, 0)["popup"]["id"], id);
        assert_eq!(hud(&mut worker, 1)["popup_count"], 0);
        assert_eq!(
            hud(&mut worker, 0)["popup"]["id"],
            id,
            "fresh subscription preserves the head"
        );
        let cmd = |screen: &str, action: &str, id| Command::ScreenAction {
            screen: screen.into(),
            action: action.into(),
            args: vec![
                ("a".into(), i64::from(id)),
                ("popup_id".into(), i64::from(id)),
            ],
            text: None,
        };
        assert!(
            worker
                .screen_or_core_command(0, &cmd("hud", "popup_done", id))
                .is_err()
        );
        assert!(
            worker
                .screen_or_core_command(1, &cmd("techs", "done", id))
                .is_err()
        );
        assert!(
            worker
                .screen_or_core_command(0, &cmd("techs", "done", id + 1))
                .is_err()
        );
        assert!(
            worker
                .screen_or_core_command(0, &cmd("techs", "fail", id))
                .is_err()
        );
        assert!(
            worker
                .screen_or_core_command(0, &cmd("techs", "refused", id))
                .is_err()
        );
        assert_eq!(hud(&mut worker, 0)["popup_count"], 1);
        worker
            .screen_or_core_command(0, &cmd("techs", "done", id))
            .unwrap();
        assert_eq!(hud(&mut worker, 0)["popup_count"], 0);
        assert!(
            matches!(worker.screen_effect, Some(protocol::CommandOutcome::ScreenApplied { popup_done: Some(done), close: true, .. }) if done == id)
        );
        let info = worker
            .inject_popup(
                0,
                Popup {
                    kind: "BUTTONPOPUP_TECH_TREE".into(),
                    ..Default::default()
                },
            )
            .unwrap()
            .unwrap();
        let dismiss = Command::ScreenAction {
            screen: "hud".into(),
            action: "popup_done".into(),
            args: vec![("a".into(), i64::from(info))],
            text: None,
        };
        worker.screen_or_core_command(0, &dismiss).unwrap();
        assert_eq!(hud(&mut worker, 0)["popup_count"], 0);
        assert!(worker.screen_or_core_command(0, &dismiss).is_err());
    }
}
