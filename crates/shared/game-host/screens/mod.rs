//! Built-in screen registry and worker-local Lua execution.
pub(crate) mod localization;
mod runtime;
pub(crate) use runtime::Screens;

/// Names and sources compiled into the server. Add one entry per screen.
pub const REGISTRY: &[(&str, &str)] = &[
    ("economy", include_str!("economy.lua")),
    ("military", include_str!("military.lua")),
    ("trade_routes", include_str!("trade_routes.lua")),
    ("espionage", include_str!("espionage.lua")),
    ("victory", include_str!("victory.lua")),
    ("demographics", include_str!("demographics.lua")),
    ("techs", include_str!("techs.lua")),
    ("hud", include_str!("hud.lua")),
];

use crate::core::Refusal;
use crate::{Core, Worker};
use protocol::{Command, PlayerId};

impl<C: Core> Worker<C> {
    fn screen_runtime(&mut self) -> Result<&Screens, String> {
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

    pub(crate) fn screen_data(
        &mut self,
        player: PlayerId,
        screen: &str,
        args: &[(String, i64)],
    ) -> Result<String, String> {
        let result = self.screen_runtime().and_then(|runtime| {
            runtime
                .data(player, screen, args)
                .map_err(|e| e.to_string())
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
            let messages = self
                .screen_runtime()?
                .action(player, screen, action, args, text.as_deref())
                .map_err(|e| e.to_string())?;
            self.core
                .replica()
                .ok_or("screen actions are unavailable for this core")?
                .net_submit(player, &gamecore::replica::encode_net(&messages))
        })();
        if let Err(reason) = &result {
            tracing::warn!(screen, action, %reason, "screen action failed");
        }
        result.map_err(Refusal::other)
    }
}
