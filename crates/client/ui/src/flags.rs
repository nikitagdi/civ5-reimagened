//! `ControlBase::m_Flags`: the same bits as in the exe (names from `FlagNode::GetDataMembers`).
//! Bits from `0x400000` up belong to the class of the control; they are kept in the data of
//! the kind, not here (see the `*_FLAG` constants next to each kind).

pub const INITIALIZED: u32 = 0x1;
pub const HIDDEN: u32 = 0x2;
pub const MODAL_PASS: u32 = 0x4;
pub const PARENT_DELETE: u32 = 0x8;
pub const H_CENTER: u32 = 0x10;
pub const H_RIGHT: u32 = 0x20;
pub const H_OUTER: u32 = 0x40;
pub const V_CENTER: u32 = 0x80;
pub const V_BOTTOM: u32 = 0x100;
pub const V_OUTER: u32 = 0x200;
pub const CONSUME_BUTTON: u32 = 0x400;
pub const CONSUME_OVER: u32 = 0x800;
pub const CONSUME_WHEEL: u32 = 0x1000;
/// `ConsumeMouse` is all three.
pub const CONSUME_MOUSE: u32 = CONSUME_BUTTON | CONSUME_OVER | CONSUME_WHEEL;
pub const MOUSE_OVER_HIDDEN: u32 = 0x2000;
/// The control has a tooltip (a string or a type); the name in the exe table is `HAS_TT_TYPE`.
pub const HAS_TOOLTIP: u32 = 0x4000;
pub const NEEDS_MOUSE_OVER: u32 = 0x8000;
pub const MOUSE_OVER: u32 = 0x10000;
pub const DISABLED: u32 = 0x20000;
pub const NO_CLIP: u32 = 0x40000;
/// Set by `SetModal` (the exe tests it in `ProcessNormalInput`).
pub const MODAL: u32 = 0x80000;
pub const SIZE_FULL_X: u32 = 0x100000;
pub const SIZE_FULL_Y: u32 = 0x200000;

/// Not taking part in layout, drawing or input: hidden or hidden by mouse-over.
pub const NOT_ACTIVE: u32 = HIDDEN | MOUSE_OVER_HIDDEN;
/// Anchor bits that `SetAnchor` clears (`m_Flags &= 0xfffffe4f`).
pub const ANCHOR_BITS: u32 = H_CENTER | H_RIGHT | V_CENTER | V_BOTTOM;
/// Children that do not take part in the flow of a stack, a grid or a scroll panel.
pub const OUTER_BITS: u32 = H_OUTER | V_OUTER;
