//! The exe's own hotkeys: a key press that no Lua input handler took is looked up in the action table
//! (`CvGame::GetAction`: the action whose hot key, modifiers and priority fit and that `canHandleAction` allows)
//! and carried out (`CvGame::handleAction`). Unit orders (F fortify, S sleep, B found city, Space skip, ...),
//! END TURN on Enter and the other `Controls` keys live there, not in the UI Lua.
//!
//! The key value of an action is `FKBInputDevice::InputType` (`CvHotKeyInfo::GetHotKeyInt` maps "KB_F" and
//! friends to it); [`kb_of_vk`] gives the value of a Windows virtual key in the order of that enum
//! (`FirePlace/include/FireWorks/Win32/FKBInputDevice.h`).
//!
//! SIMPLIFIED(SIMP-exe-561): the lookup is Lua over `GameInfoActions`, a control goes through `Game.DoControl`.

/// `FKBInputDevice::InputType` of a Windows virtual-key code (`KB_NONE` = 0 is `None`).
pub fn kb_of_vk(vk: u32) -> Option<i32> {
    Some(match vk {
        0x1b => 1,                              // KB_ESCAPE
        0x30..=0x39 => 2 + (vk - 0x30) as i32,  // KB_0 .. KB_9
        0xbd => 12,                             // KB_MINUS
        0x41..=0x5a => 13 + (vk - 0x41) as i32, // KB_A .. KB_Z
        0xbb => 39,                             // KB_EQUALS
        0x08 => 40,                             // KB_BACKSPACE
        0x09 => 41,                             // KB_TAB
        0xdb => 42,                             // KB_LBRACKET
        0xdd => 43,                             // KB_RBRACKET
        0x0d => 44,                             // KB_RETURN
        0x11 | 0xa2 => 45,                      // KB_LCONTROL
        0xba => 46,                             // KB_SEMICOLON
        0xde => 47,                             // KB_APOSTROPHE
        0xc0 => 48,                             // KB_GRAVE
        0x10 | 0xa0 => 49,                      // KB_LSHIFT
        0xdc => 50,                             // KB_BACKSLASH
        0xbc => 51,                             // KB_COMMA
        0xbe => 52,                             // KB_PERIOD
        0xbf => 53,                             // KB_SLASH
        0xa1 => 54,                             // KB_RSHIFT
        0x6a => 55,                             // KB_NUMPADSTAR
        0x12 | 0xa4 => 56,                      // KB_LALT
        0x20 => 57,                             // KB_SPACE
        0x14 => 58,                             // KB_CAPSLOCK
        0x70..=0x7b => 59 + (vk - 0x70) as i32, // KB_F1 .. KB_F12
        0x90 => 71,                             // KB_NUMLOCK
        0x91 => 72,                             // KB_SCROLL
        0x60..=0x69 => 73 + (vk - 0x60) as i32, // KB_NUMPAD0 .. KB_NUMPAD9
        0x6d => 83,                             // KB_NUMPADMINUS
        0x6b => 84,                             // KB_NUMPADPLUS
        0x6e => 85,                             // KB_NUMPADPERIOD
        0xa3 => 91,                             // KB_RCONTROL
        0x6f => 95,                             // KB_NUMPADSLASH
        0xa5 => 97,                             // KB_RALT
        0x13 => 98,                             // KB_PAUSE
        0x24 => 99,                             // KB_HOME
        0x26 => 100,                            // KB_UP
        0x21 => 101,                            // KB_PGUP
        0x25 => 102,                            // KB_LEFT
        0x27 => 103,                            // KB_RIGHT
        0x23 => 104,                            // KB_END
        0x28 => 105,                            // KB_DOWN
        0x22 => 106,                            // KB_PGDN
        0x2d => 107,                            // KB_INSERT
        0x2e => 108,                            // KB_DELETE
        _ => return None,
    })
}

/// The Lua of one hot key: `GetAction` over `GameInfoActions` (the first action of the highest priority whose key
/// and modifiers fit and that `Game.CanHandleAction` allows), then `handleAction`. A control goes through
/// `Game.DoControl`, which the exe API wraps (END TURN is the turn handshake, SIMP-exe-65); everything else through
/// `Game.HandleAction`, as the unit panel's buttons do. Answers the `Type` of the action, or nil.
///
/// As `GameViewState::ProcessKeyboard` (re-ui-live-answers ITEM 2): nothing happens unless the active player is a living
/// human whose turn is active; the numpad keys 1, 3, 4, 6, 7, 9 move the selected unit one plot in their direction
/// (`InterfaceBuddy::DirectionAction`, before the action table). SIMPLIFIED(SIMP-exe-561): the direction move does not
/// check whether the unit is on screen (the exe then only centres the camera on it).
///
/// `executing` is the exe's `IsGameCoreExecuting` (the AI phase: every human has ended the turn and the server's core
/// moves the other civilizations): no key does anything then. An observer (`SS_OBSERVER`) is let through the turn gate.
/// SIMPLIFIED(SIMP-exe-960): "core executing" is read off the turn handshake (`Core::turn_all_complete`), not off a
/// core flag.
pub(crate) fn lua(kb: i32, ctrl: bool, alt: bool, shift: bool, executing: bool) -> String {
    let dir = numpad_direction(kb).unwrap_or(-1);
    format!(
        "local kb, ctrl, alt, shift, dir, executing = {kb}, {ctrl}, {alt}, {shift}, {dir}, {executing}
         if executing then return nil end
         local A, G = GameInfoActions, Game
         if type(A) ~= 'table' or not G or not G.CanHandleAction or not G.HandleAction then return nil end
         local me = G.GetActivePlayer and G.GetActivePlayer()
         local P = Players and me and Players[me]
         local obs = P and P.IsObserver and P:IsObserver()
         if P and not obs and not (P:IsHuman() and P:IsAlive() and P:IsTurnActive()) then return nil end
         if dir >= 0 then
           local u = UI.GetHeadSelectedUnit()
           local to = u and Map.PlotDirection(u:GetX(), u:GetY(), dir)
           if not to then return nil end
           G.SelectionListMove(to, alt, shift, ctrl)
           return 'DIRECTION_MOVE'
         end
         local best, prio = nil, -1
         local i = 0
         while A[i] do
           local a = A[i]
           local main = a.HotKeyVal == kb and a.HotKeyPriority > prio and a.AltDown == alt and a.ShiftDown == shift
             and a.CtrlDown == ctrl
           local other = a.HotKeyValAlt == kb and a.HotKeyPriorityAlt > prio and a.AltDownAlt == alt
             and a.ShiftDownAlt == shift and a.CtrlDownAlt == ctrl
           if (main or other) and G.CanHandleAction(i) then
             best, prio = i, a.HotKeyPriority
           end
           i = i + 1
         end
         if not best then return nil end
         local a = A[best]
         if a.ControlType and a.ControlType >= 0 and G.DoControl then
           G.DoControl(a.ControlType)
         else
           G.HandleAction(best)
         end
         return a.Type"
    )
}

/// `DirectionTypes` (0 NE, 1 E, 2 SE, 3 SW, 4 W, 5 NW) of the numpad keys the exe moves a unit with: 9, 6, 3, 1, 4, 7.
pub fn numpad_direction(kb: i32) -> Option<i32> {
    // KB_NUMPAD0 is 73.
    Some(match kb - 73 {
        9 => 0,
        6 => 1,
        3 => 2,
        1 => 3,
        4 => 4,
        7 => 5,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn virtual_keys_map_to_the_engine_key_enum() {
        assert_eq!(kb_of_vk(0x1b), Some(1));
        assert_eq!(kb_of_vk(u32::from(b'0')), Some(2));
        assert_eq!(kb_of_vk(u32::from(b'A')), Some(13));
        assert_eq!(kb_of_vk(u32::from(b'F')), Some(18));
        assert_eq!(kb_of_vk(u32::from(b'Z')), Some(38));
        assert_eq!(kb_of_vk(0x0d), Some(44));
        assert_eq!(kb_of_vk(0x20), Some(57));
        assert_eq!(kb_of_vk(0x70), Some(59));
        assert_eq!(kb_of_vk(0x7b), Some(70));
        assert_eq!(kb_of_vk(0x25), Some(102));
        assert_eq!(kb_of_vk(0x2e), Some(108));
        assert_eq!(kb_of_vk(0xff), None);
        assert_eq!(kb_of_vk(0x69).and_then(numpad_direction), Some(0));
        assert_eq!(kb_of_vk(0x61).and_then(numpad_direction), Some(3));
        assert_eq!(kb_of_vk(0x65).and_then(numpad_direction), None);
    }
}
