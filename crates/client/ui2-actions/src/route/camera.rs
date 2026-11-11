//! `camera.to_plot`, `camera.to_city`, `camera.to_unit`: ask the host to centre the camera. The position of a
//! city or unit comes from the model.

use ui2_model::generated::RefuseWhy;

use crate::intent::Intent;
use crate::route::Ctx;
use crate::sink::CameraReq;
use crate::validate;

/// Run a `camera.*` intent.
pub(crate) fn run(ctx: &mut Ctx<'_>, intent: &Intent) {
    match intent.name() {
        "camera.to_plot" => {
            if let Some((x, y)) = intent.plot("plot") {
                to(ctx, x, y);
            }
        }
        "camera.to_city" => {
            let Some(city) = intent.key("city") else {
                return;
            };
            // Any city on the map will do, the player's or not.
            let plot = ctx
                .model
                .get::<ui2_model::generated::Map>()
                .and_then(|m| m.cities.get(city))
                .map(|c| (c.plot.x, c.plot.y));
            match plot {
                Some((x, y)) => to(ctx, x, y),
                None => ctx.refuse(RefuseWhy::NotYourCity),
            }
        }
        "camera.to_unit" => {
            let Some(unit) = intent.key("unit") else {
                return;
            };
            match validate::any_unit(ctx.model, unit).map(|u| (u.plot.x, u.plot.y)) {
                Some((x, y)) => to(ctx, x, y),
                None => ctx.refuse(RefuseWhy::NoUnit),
            }
        }
        _ => {}
    }
}

/// Centre the camera on a plot.
pub(crate) fn to(ctx: &mut Ctx<'_>, x: i16, y: i16) {
    ctx.sink.camera(CameraReq::Plot(x, y));
}
