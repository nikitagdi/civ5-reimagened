//! `ToHexFromGrid`, `ToGridFromHex`, `HexToWorld`, `GridToWorld` (`LuaContext.h`), and the constants the
//! projection of `WorldAnchor` shares (WO-6).
//!
//! A plot has grid coordinates `(x, y)` (odd rows sit half a hex east) and hex coordinates
//! `(hx, hy) = (x - floor(y / 2), y)`. The Lua world has x east, y north, z up, in the units of the
//! renderer (`render::hexgrid`: a hex is 64 wide, rows are 64 * sqrt(3) / 2 apart; the renderer draws
//! north as -Z, the Lua world does not).
//!
//! z is the height of the relief at the plot's centre, as the exe gives it (the host's
//! [`crate::ExeHost::plot_height`]; 0 without a relief): `PathHelpManager.lua` puts the turn numbers 3 over it,
//! the yield and resource icons and the unit flags stand on it.
//! SIMPLIFIED(SIMP-exe-11): the constants are the renderer's (RV-11), since the exe's bodies for these four
//! functions are not recovered.

use std::rc::Rc;

use lua_sys::{Lua, LuaValue, MultiValue, Result, Table};

use crate::registry::Reg;
use crate::{ExeHost, Inner};

/// Hex width, world units (`render::hexgrid::HEX_WIDTH`).
pub const HEX_WIDTH: f32 = 64.0;
/// Distance between rows (`render::hexgrid::ROW_STEP`).
pub const ROW_STEP: f32 = 55.425_625;

/// `ToHexFromGrid`.
pub fn grid_to_hex(x: i32, y: i32) -> (i32, i32) {
    (x - y.div_euclid(2), y)
}

/// `ToGridFromHex`.
pub fn hex_to_grid(hx: i32, hy: i32) -> (i32, i32) {
    (hx + hy.div_euclid(2), hy)
}

/// `GridToWorld`: the centre of the plot, `(x east, y north, z up)`.
pub fn grid_to_world(x: i32, y: i32) -> [f32; 3] {
    [
        HEX_WIDTH * (x as f32 + 0.5 * (y & 1) as f32),
        ROW_STEP * y as f32,
        0.0,
    ]
}

/// `HexToWorld`.
pub fn hex_to_world(hx: i32, hy: i32) -> [f32; 3] {
    let (x, y) = hex_to_grid(hx, hy);
    grid_to_world(x, y)
}

/// `GridToWorld` with the height of the relief the host knows.
pub fn grid_to_world_on(host: &dyn ExeHost, x: i32, y: i32) -> [f32; 3] {
    let [wx, wy, _] = grid_to_world(x, y);
    [wx, wy, host.plot_height(x, y).unwrap_or(0.0)]
}

fn num(v: &LuaValue<'_>) -> Option<i32> {
    v.as_f64().map(|n| n.floor() as i32)
}

/// Two coordinates from the arguments: `(x, y)` or `{x = .., y = ..}`.
fn pair(args: &MultiValue<'_>) -> Result<(i32, i32)> {
    let bad = || lua_sys::LuaError::msg("expected a position: (x, y) or a table with x and y");
    match args.get(0) {
        Some(LuaValue::Table(t)) => {
            let x: LuaValue = t.get("x")?;
            let y: LuaValue = t.get("y")?;
            Ok((num(&x).ok_or_else(bad)?, num(&y).ok_or_else(bad)?))
        }
        Some(a) => {
            let b = args.get(1).ok_or_else(bad)?;
            Ok((num(a).ok_or_else(bad)?, num(b).ok_or_else(bad)?))
        }
        None => Err(bad()),
    }
}

pub(crate) fn install<'l>(inner: &Rc<Inner>, lua: &'l Lua, g: &Table<'l>) -> Result<()> {
    let reg = Reg::new(inner);
    reg.global_real(lua, g, "ToHexFromGrid", |lua, args: MultiValue| {
        let (x, y) = pair(&args)?;
        let (hx, hy) = grid_to_hex(x, y);
        let t = lua.create_table();
        t.raw_set("x", hx)?;
        t.raw_set("y", hy)?;
        Ok(t)
    })?;
    reg.global_real(lua, g, "ToGridFromHex", |_, args: MultiValue| {
        let (hx, hy) = pair(&args)?;
        let (x, y) = hex_to_grid(hx, hy);
        Ok((x, y))
    })?;
    let host = Rc::clone(&inner.host);
    reg.global_real(lua, g, "GridToWorld", move |_, args: MultiValue| {
        let (x, y) = pair(&args)?;
        let [wx, wy, wz] = grid_to_world_on(&*host, x, y);
        Ok((f64::from(wx), f64::from(wy), f64::from(wz)))
    })?;
    let host = Rc::clone(&inner.host);
    reg.global_real(lua, g, "HexToWorld", move |lua, args: MultiValue| {
        let (hx, hy) = pair(&args)?;
        let (x, y) = hex_to_grid(hx, hy);
        let [wx, wy, wz] = grid_to_world_on(&*host, x, y);
        let t = lua.create_table();
        t.raw_set("x", f64::from(wx))?;
        t.raw_set("y", f64::from(wy))?;
        t.raw_set("z", f64::from(wz))?;
        Ok(t)
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_and_grid_round_trip_including_negative_rows() {
        for y in -5..8 {
            for x in -4..12 {
                let (hx, hy) = grid_to_hex(x, y);
                assert_eq!(hex_to_grid(hx, hy), (x, y));
            }
        }
        assert_eq!(grid_to_hex(5, 3), (4, 3));
    }

    #[test]
    fn odd_rows_sit_half_a_hex_east() {
        assert_eq!(grid_to_world(2, 0)[0], 2.0 * HEX_WIDTH);
        assert_eq!(grid_to_world(2, 1)[0], 2.5 * HEX_WIDTH);
        assert_eq!(hex_to_world(1, 2), grid_to_world(2, 2));
    }
}
