//! The game's mouse cursors (the animated `.ani` files of `Assets/UI/Cursors`): the arrow over the game, the spinning
//! globe while the game loads and while the server runs the turn, and the cursor of each interface mode (go to, route
//! to, ranged attack, airlift, nuke, paradrop, rebase, gift, buy plot...).
//!
//! Which one shows is the original's own rule: `InGame.lua` calls `UIManager:SetUICursor(n)` on every interface mode
//! change (`InterfaceModes.CursorType` of the mode, a row of the `Cursors` table of `CIV5Cursors.xml`, `n` = the row's
//! ID) and on the turn's end (busy). The pictures come from the asset set (`data:cursors`, a
//! [`CursorSet`] that `tools/asset-pipeline` makes from the user's install, set `client-data`); a browser cannot show
//! an `.ani`, so the animation is played frame by frame at the rate of the file. Without the blob (an asset set
//! without it, or no assets at all) the window keeps the platform's default cursor.
//!
//! SIMPLIFIED(SIMP-exe-1010): the cursors of the `Cursors` rows that no interface mode uses (the window resize arrows
//! Split/Size...) show the arrow, and so do `attack.ani`, `Found.ani`, `Build.ani`, `target.ani` and the other files of
//! the folder that no data row or script names: where the exe would use them is not established.

use asset_format::CursorSet;
use winit::event_loop::ActiveEventLoop;
use winit::window::{CustomCursor, CustomCursorSource};

/// The cursors by name, in the order of the targets of `ART_OF_ROW` (the names `asset-pipeline` writes).
const ART: &[&str] = &[
    "pointer",
    "waiting",
    "ping",
    "go_to",
    "route_to",
    "airlift",
    "nuke",
    "airstrike",
    "recon",
    "paradrop",
    "airbomb",
    "rebase",
    "move",
    "link",
    "grip",
    "edit",
    "claim",
    "gift",
];

/// The index in `ART` of the cursor of a `Cursors` row (the ID `UIManager:SetUICursor` gets; the rows of
/// `CIV5Cursors.xml` in order): DEFAULT 0, BUSY 1, PING 2, GO_TO 3, ROUTE_TO 4, AIRLIFT 5, NUKE 6, AIRSTRIKE 7, RECON 8,
/// PARADROP 9, AIRBOMB 10, REBASE 11, MOVE 12, SPLITV..SIZEALL 13-19, LINK 20, GRIP 21, EDIT 22, GOLD 23, GIFT 24.
const ART_OF_ROW: [usize; 25] = [
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, // up to MOVE
    0, 0, 0, 0, 0, 0, 0, // the window resize arrows: the arrow
    13, 14, 15, // LINK, GRIP, EDIT
    16, 17, // GOLD (Claim), GIFT
];

/// The row of the busy globe.
const ROW_BUSY: i32 = 1;

/// What the cursor shows: a cursor (`ART` index) and the frame of its animation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pick {
    pub art: usize,
    pub frame: usize,
}

/// Frame count and frame time of each cursor of `ART` (a cursor the set lacks: one frame of 100 ms).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Timing {
    frames: Vec<(usize, u64)>,
}

impl Timing {
    pub fn of(set: &CursorSet) -> Self {
        let fb = set.frame_bytes().max(1);
        Self {
            frames: ART
                .iter()
                .map(|name| match set.get(name) {
                    Some(c) => ((c.rgba.len() / fb).max(1), u64::from(c.frame_ms.max(1))),
                    None => (1, 100),
                })
                .collect(),
        }
    }
}

/// The cursor for the moment `now_ms`: the globe while the game is `busy`, else the one of the script's last
/// `UIManager:SetUICursor(row)` (the arrow before the first call and for a row with no picture).
pub fn pick(timing: &Timing, row: i32, busy: bool, now_ms: u64) -> Pick {
    let row = if busy { ROW_BUSY } else { row };
    let art = usize::try_from(row)
        .ok()
        .and_then(|r| ART_OF_ROW.get(r).copied())
        .unwrap_or(0);
    let (frames, ms) = timing.frames.get(art).copied().unwrap_or((1, 100));
    let frame = (now_ms / ms) as usize % frames.max(1);
    Pick { art, frame }
}

/// The cursors made on the event loop from the asset set.
pub struct Cursors {
    /// Per `ART` entry its frames; empty for a cursor the set lacks (the arrow shows instead).
    frames: Vec<Vec<CustomCursor>>,
    timing: Timing,
}

fn source(set: &CursorSet, rgba: &[u8], hot: (u16, u16)) -> Option<CustomCursorSource> {
    CustomCursor::from_rgba(rgba.to_vec(), set.size, set.size, hot.0, hot.1).ok()
}

impl Cursors {
    /// `None` when the set has no arrow (nothing to fall back to: the platform cursor stays).
    pub fn new(el: &ActiveEventLoop, set: &CursorSet) -> Option<Self> {
        let fb = set.frame_bytes();
        let mut frames = Vec::with_capacity(ART.len());
        for name in ART {
            let mut list = Vec::new();
            if let Some(c) = set.get(name) {
                for frame in c.rgba.chunks_exact(fb) {
                    list.push(el.create_custom_cursor(source(set, frame, c.hot)?));
                }
            }
            frames.push(list);
        }
        if frames[0].is_empty() {
            return None;
        }
        Some(Self {
            frames,
            timing: Timing::of(set),
        })
    }

    pub fn timing(&self) -> &Timing {
        &self.timing
    }

    pub fn get(&self, pick: Pick) -> &CustomCursor {
        let list = match self.frames.get(pick.art) {
            Some(l) if !l.is_empty() => l,
            _ => &self.frames[0],
        };
        &list[pick.frame % list.len()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use asset_format::CursorArt;

    /// A set where cursor `i` of `ART` has `i + 1` frames of 100 ms (the globe 30, the arrow 17).
    fn set() -> CursorSet {
        CursorSet {
            size: 2,
            cursors: ART
                .iter()
                .enumerate()
                .map(|(i, name)| {
                    let frames = match i {
                        0 => 17,
                        1 => 30,
                        _ => i + 1,
                    };
                    CursorArt {
                        name: (*name).into(),
                        hot: (0, 0),
                        frame_ms: 100,
                        rgba: vec![i as u8; 2 * 2 * 4 * frames],
                    }
                })
                .collect(),
        }
    }

    #[test]
    fn every_row_maps_to_a_cursor() {
        assert!(ART_OF_ROW.iter().all(|&a| a < ART.len()));
    }

    #[test]
    fn the_arrow_shows_until_the_script_asks_for_another_cursor() {
        let t = Timing::of(&set());
        assert_eq!(pick(&t, 0, false, 0), Pick { art: 0, frame: 0 });
        assert_eq!(pick(&t, 0, false, 100).frame, 1);
        // A full turn of the arrow is 17 frames; then it starts again.
        assert_eq!(pick(&t, 0, false, 1_700), Pick { art: 0, frame: 0 });
        // A row without a number (a script that never set one) and one with no picture show the arrow.
        assert_eq!(pick(&t, -1, false, 0).art, 0);
        assert_eq!(pick(&t, 99, false, 0).art, 0);
        assert_eq!(pick(&t, 15, false, 0).art, 0);
    }

    #[test]
    fn each_interface_mode_row_has_its_own_cursor() {
        let t = Timing::of(&set());
        let name = |row| ART[pick(&t, row, false, 0).art];
        assert_eq!(name(3), "go_to");
        assert_eq!(name(4), "route_to");
        assert_eq!(name(5), "airlift");
        assert_eq!(name(6), "nuke");
        assert_eq!(name(7), "airstrike");
        assert_eq!(name(9), "paradrop");
        assert_eq!(name(10), "airbomb");
        assert_eq!(name(11), "rebase");
        assert_eq!(name(12), "move");
        assert_eq!(name(23), "claim");
        assert_eq!(name(24), "gift");
    }

    #[test]
    fn the_globe_turns_while_the_game_is_busy() {
        let t = Timing::of(&set());
        assert_eq!(pick(&t, 3, true, 0), Pick { art: 1, frame: 0 });
        assert_eq!(pick(&t, 3, true, 99).frame, 0);
        assert_eq!(pick(&t, 3, true, 100).frame, 1);
        // The script's own busy row (OnActivePlayerTurnEnd) is the globe too.
        assert_eq!(pick(&t, 1, false, 0).art, 1);
        // A full turn of the globe is 30 frames; then it starts again.
        assert_eq!(pick(&t, 0, true, 3_000), Pick { art: 1, frame: 0 });
    }

    #[test]
    fn a_cursor_the_set_lacks_has_one_frame() {
        let mut s = set();
        s.cursors.retain(|c| c.name != "nuke");
        let t = Timing::of(&s);
        assert_eq!(pick(&t, 6, false, 12_345).frame, 0);
    }
}
