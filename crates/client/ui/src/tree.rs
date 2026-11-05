//! The control tree: an arena of nodes with generation-checked ids. The fields are the ones of
//! `ControlBase` (offset, position, size, "full" size, visibility box, alpha, flags) plus the data of
//! the kind of the control. No trait objects: the behaviour of a kind is a `match` in the modules
//! of layout, input and drawing.

use std::collections::HashMap;
use std::rc::Rc;

use draw_list::{TextureId, UiSampler};
use glam::Vec2;
use lua_sys::RegistryKey;

use crate::style::ColorSet;
use crate::text::{TextLayout, TextRequest};
use crate::xml::XmlElem;

/// A reference to a control that notices when the control was deleted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ControlId {
    pub(crate) index: u32,
    pub(crate) ver: u32,
}

impl ControlId {
    pub fn index(self) -> u32 {
        self.index
    }
}

/// A Lua function kept for a control; cloning shares the key.
pub type Callback = Rc<RegistryKey>;

/// A texture a control uses: the name as written, the id for drawing and the size in pixels.
#[derive(Debug, Clone, PartialEq)]
pub struct TexRef {
    pub name: String,
    pub id: TextureId,
    pub w: f32,
    pub h: f32,
}

/// `UIQuad`: one textured rectangle with a source window.
#[derive(Debug, Clone, PartialEq)]
pub struct Quad {
    pub texture: Option<TexRef>,
    pub mask: Option<TexRef>,
    /// `SetTextureHandle`: a texture the renderer owns (the minimap).
    pub handle: Option<TextureId>,
    pub color: [f32; 4],
    pub width: f32,
    pub height: f32,
    pub src_w: f32,
    pub src_h: f32,
    pub src_x: f32,
    pub src_y: f32,
    pub sampler: UiSampler,
    /// `field_40`: 2 when there is a mask.
    pub blend_slot: u8,
    pub uv_scale: Vec2,
    pub uv_offset: Vec2,
}

impl Default for Quad {
    fn default() -> Self {
        Quad {
            texture: None,
            mask: None,
            handle: None,
            color: [1.0; 4],
            width: 0.0,
            height: 0.0,
            src_w: 0.0,
            src_h: 0.0,
            src_x: 0.0,
            src_y: 0.0,
            sampler: UiSampler::Linear,
            blend_slot: 0,
            uv_scale: Vec2::ONE,
            uv_offset: Vec2::ZERO,
        }
    }
}

impl Quad {
    /// `UIQuad::NormalizeTexture`: the source window in pixels becomes UV scale and offset.
    pub fn normalize(&mut self) {
        let Some(t) = &self.texture else { return };
        if t.w == 0.0 || t.h == 0.0 {
            return;
        }
        let mut u = self.src_x / t.w;
        let mut v = self.src_y / t.h;
        if u < 0.0 {
            u += 1.0;
        }
        if v < 0.0 {
            v += 1.0;
        }
        self.uv_offset = Vec2::new(u, v);
        self.uv_scale = Vec2::new(self.src_w / t.w, self.src_h / t.h);
    }

    /// `UIQuad::SetTexture`.
    pub fn set_texture(&mut self, tex: Option<TexRef>, mask: Option<TexRef>) {
        if let Some(t) = tex {
            self.texture = Some(t);
            self.handle = None;
        }
        if let Some(m) = mask {
            self.mask = Some(m);
            self.blend_slot = 2;
        }
        self.normalize();
    }
}

/// State of the buttons that all `ButtonControlBase` descendants share.
#[derive(Default)]
pub struct ButtonBase {
    pub void1: i32,
    pub void2: i32,
    /// 0 normal, 1 mouse over, 2 pressed, 3 disabled.
    pub vis_state: u32,
    /// Callbacks by mouse slot (`Mouse.eLClick` ...), eight slots.
    pub callbacks: [Option<Callback>; 8],
    /// Where inside the control the last click happened (`field_120/124`).
    pub grab: Vec2,
    /// A handler of the exe's own code (the pull-down binds its buttons this way).
    pub native: Native,
}

/// The native delegates the UI binds to buttons.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Native {
    #[default]
    None,
    /// `PullDownControl::MainButtonPressed`.
    PullDownMain(ControlId),
    /// `PullDownControl::SubButtonPressed`.
    PullDownSub(ControlId),
    /// `ScrollPanelControl::UpPressed` / `DownPressed`.
    ScrollUp(ControlId),
    ScrollDown(ControlId),
}

pub mod btn {
    //! Class bits of `ButtonControlBase` in `m_Flags`.
    pub const LEFT_DOWN: u32 = 0x40_0000;
    pub const RIGHT_DOWN: u32 = 0x80_0000;
    pub const MIDDLE_DOWN: u32 = 0x100_0000;
    pub const NO_DEFAULT_SOUND: u32 = 0x200_0000;
    pub const FORCED_UP: u32 = 0x400_0000;
    pub const ANY_DOWN: u32 = LEFT_DOWN | RIGHT_DOWN | MIDDLE_DOWN;
}

pub struct Button {
    pub base: ButtonBase,
    pub quad: Quad,
    pub tex_offset: Vec2,
    pub state_inc: Vec2,
    pub text: Option<ControlId>,
}

pub mod button_flag {
    pub const HAS_TEXT: u32 = 0x0800_0000;
    pub const NO_QUAD: u32 = 0x1000_0000;
    pub const STATE_INC: u32 = 0x2000_0000;
    pub const TEXT_ANCHOR: u32 = 0x4000_0000;
    pub const NO_STATE_CHANGE: u32 = 0x8000_0000;
}

pub struct TextButton {
    pub base: ButtonBase,
    pub label: ControlId,
    /// Colour sets of the four states (normal, mouse over, pressed, disabled).
    pub sets: [ColorSet; 4],
    pub styles: [String; 4],
}

pub struct GridButton {
    pub base: ButtonBase,
    pub grid: ControlId,
    pub text: Option<ControlId>,
}

pub struct CheckBox {
    pub base: ButtonBase,
    pub boxed: ControlId,
    pub label: ControlId,
    pub mark: Quad,
    pub mark_size: Vec2,
    pub mark_offset: Vec2,
    pub checked: bool,
    pub handler: Option<Callback>,
    /// Radio buttons: the group name.
    pub radio_group: Option<String>,
}

pub struct Image {
    pub quad: Quad,
    /// 0 none given, 1 None, 2 Uniform, 3 Fill, 4 UniformToFill.
    pub stretch: u8,
    pub tex_w: f32,
    pub tex_h: f32,
}

pub mod image_flag {
    /// `Sampler="Linear"`: the exe selects sampler 5.
    pub const SAMPLER_SET: u32 = 0x40_0000;
}

pub struct Label {
    pub text: String,
    pub font: Option<String>,
    pub style: Option<String>,
    pub colors: ColorSet,
    pub color_explicit: bool,
    pub wrap_width: i32,
    pub truncate_width: i32,
    pub leading_offset: i32,
    pub obscured: bool,
    pub reduce_on_truncate: bool,
    pub layout: Option<Rc<TextLayout>>,
    /// The request the layout was made for: a new layout is made only when it changes.
    pub request: Option<TextRequest>,
}

impl Default for Label {
    fn default() -> Self {
        Label {
            text: String::new(),
            font: None,
            style: None,
            colors: ColorSet::default(),
            color_explicit: false,
            wrap_width: -1,
            truncate_width: -1,
            leading_offset: 0,
            obscured: false,
            reduce_on_truncate: false,
            layout: None,
            request: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnimVariant {
    Scroll,
    Flip,
    Slide,
    Alpha,
}

pub struct Anim {
    pub variant: AnimVariant,
    pub progress: f32,
    pub speed: f32,
    pub pause: f32,
    pub end_pause: f32,
    pub pause_remaining: f32,
    pub power: f32,
    /// 0 Once, 1 repeat, 2 Bounce, 3 OneBounce.
    pub cycle: u8,
    /// 0 linear, 1 `x^p`, 2 `x^(1/p)`.
    pub function: u8,
    pub reversing: bool,
    pub stopped: bool,
    pub callback: Option<Callback>,
    pub quad: Option<Quad>,
    pub alpha_start: f32,
    pub alpha_end: f32,
    pub slide_begin: Vec2,
    pub slide_end: Vec2,
    pub flip_step: Vec2,
    pub flip_columns: i32,
    pub flip_frames: i32,
    pub flip_speed: f32,
}

impl Anim {
    pub fn new(variant: AnimVariant) -> Anim {
        Anim {
            variant,
            progress: 0.0,
            speed: 1.0,
            pause: 0.0,
            end_pause: 0.0,
            pause_remaining: 0.0,
            power: 2.0,
            cycle: 1,
            function: 0,
            reversing: false,
            stopped: false,
            callback: None,
            quad: None,
            alpha_start: 0.0,
            alpha_end: 1.0,
            slide_begin: Vec2::ZERO,
            slide_end: Vec2::ZERO,
            flip_step: Vec2::ZERO,
            flip_columns: 1,
            flip_frames: 1,
            flip_speed: 1.0,
        }
    }
}

pub struct Stack {
    /// 0 down, 1 up, 2 left, 3 right.
    pub dir: i32,
    pub wrap_dir: i32,
    pub wrap_width: i32,
    /// Where the flow starts in the parent (`m_fContentWidth/Height`).
    pub content: Vec2,
    pub padding: Vec2,
}

impl Default for Stack {
    fn default() -> Self {
        Stack {
            dir: 0,
            wrap_dir: 2,
            wrap_width: 0,
            content: Vec2::ZERO,
            padding: Vec2::ZERO,
        }
    }
}

pub mod grid_flag {
    pub const AUTO_SIZE: u32 = 0x40_0000;
    pub const NO_STATE_CHANGE: u32 = 0x80_0000;
}

/// `GridControl`: nine cells in the order C, UC, LC, L, R, UL, UR, LL, LR.
pub struct Grid {
    pub auto_margin: Vec2,
    pub color: [f32; 4],
    pub hkerning: [f32; 2],
    pub vkerning: [f32; 2],
    pub state_step: Vec2,
    pub tex_offset: Vec2,
    pub cur_offset: Vec2,
    pub cell_offset: [Vec2; 9],
    pub cell_size: [Vec2; 9],
    pub cell_min: [Vec2; 9],
    pub cell_max: [Vec2; 9],
    pub uv_min: [Vec2; 9],
    pub uv_max: [Vec2; 9],
    pub texture: Option<TexRef>,
}

impl Default for Grid {
    fn default() -> Self {
        Grid {
            auto_margin: Vec2::ZERO,
            color: [1.0; 4],
            hkerning: [0.0; 2],
            vkerning: [0.0; 2],
            state_step: Vec2::ZERO,
            tex_offset: Vec2::ZERO,
            cur_offset: Vec2::ZERO,
            cell_offset: [Vec2::ZERO; 9],
            cell_size: [Vec2::ZERO; 9],
            cell_min: [Vec2::ZERO; 9],
            cell_max: [Vec2::ZERO; 9],
            uv_min: [Vec2::ZERO; 9],
            uv_max: [Vec2::ZERO; 9],
            texture: None,
        }
    }
}

pub mod sp_flag {
    pub const VERTICAL: u32 = 0x40_0000;
    /// `AutoScrollBar="0"`: disable the bar and the buttons instead of hiding the bar.
    pub const NO_AUTO_BAR: u32 = 0x80_0000;
    pub const BAR_HIDDEN: u32 = 0x100_0000;
    /// Some child is `*_OUTER`: it gets input first.
    pub const HAS_OUTER: u32 = 0x200_0000;
    pub const FULL_CLIP: u32 = 0x400_0000;
}

pub struct ScrollPanel {
    /// The size that children anchor to (`+0x8c`): the size of the content along the scroll axis.
    pub inner: Vec2,
    pub bounds: [f32; 4],
    /// The scroll offset applied to the children (`m_fOffsetX/Y` of the class).
    pub scroll: Vec2,
    pub value: f32,
    pub ratio: f32,
    pub min_offset: f32,
    pub max_offset: f32,
    pub step: f32,
    pub up_end: Option<Callback>,
    pub down_end: Option<Callback>,
    pub up: Option<ControlId>,
    pub down: Option<ControlId>,
    pub bar: Option<ControlId>,
}

impl Default for ScrollPanel {
    fn default() -> Self {
        ScrollPanel {
            inner: Vec2::ZERO,
            bounds: [0.0; 4],
            scroll: Vec2::ZERO,
            value: 0.0,
            ratio: 1.0,
            min_offset: 0.0,
            max_offset: 0.0,
            step: 0.0,
            up_end: None,
            down_end: None,
            up: None,
            down: None,
            bar: None,
        }
    }
}

pub struct Slider {
    pub base: ButtonBase,
    pub track: ControlId,
    pub shuttle: ControlId,
    pub value: f32,
    pub travel: i32,
    pub padding: i32,
    pub grab: f32,
    pub wheel_steps: f32,
    pub callback: Option<Callback>,
    pub vertical: bool,
    pub dragging: bool,
}

pub struct EditBox {
    pub label: ControlId,
    pub text: String,
    pub max_length: i32,
    pub number_input: bool,
    pub obscure: bool,
    pub call_on_char: bool,
    pub edit_mode: bool,
    pub keep_focus: bool,
    pub focus_stop: i32,
    pub void1: i32,
    pub void2: i32,
    pub callback: Option<Callback>,
    pub cursor: usize,
}

pub struct ColorBox {
    pub color: [f32; 4],
}

pub struct Bar {
    pub fg: [f32; 4],
    pub bg: [f32; 4],
    pub direction: i32,
    pub percent: f32,
}

pub struct Meter {
    pub quad: Quad,
    pub color: [f32; 4],
    pub shadow_alpha: f32,
    pub has_shadow: bool,
    pub percent: f32,
    pub shadow_percent: f32,
    pub shadow_color: [f32; 4],
}

pub struct TextureBar {
    pub quad: Quad,
    pub direction: i32,
    pub has_shadow: bool,
    pub percent: f32,
    pub shadow_percent: f32,
    pub shadow_color: [f32; 4],
    pub tex_offset: Vec2,
}

pub struct Line {
    pub start: Vec2,
    pub end: Vec2,
    pub width: f32,
    pub color: [f32; 4],
}

pub struct WorldAnchor {
    pub world: [f32; 3],
    pub depth_adjust: f32,
    /// `Scaling="True"`: the children shrink and grow with the distance of the point from the camera.
    pub scaling: bool,
    /// The factor the children of a scaling anchor are drawn at, about the anchor's point (1 otherwise).
    pub scale: f32,
    pub bounds: [f32; 4],
}

/// A scaling `WorldAnchor` draws one UI pixel of its children this many world units wide at its point.
/// SIMPLIFIED(SIMP-uic-11): the exe's formula of `Scaling` is not recovered; this size makes a 128 px cell of the
/// yield atlas 19.2 of the hex's 64 units, so a row of four yields (YieldIconManager's stacks, padding -40) spans
/// about one hex, the look of the original's map yields as remembered (not measured on a capture).
pub const SCALING_WORLD_PER_PIXEL: f32 = 0.15;

/// The replay map of the end-game replay tab: one colour per plot, set by `SetPlot` for the turn shown.
///
/// SIMPLIFIED(SIMP-exe-940): the exe draws the plots as terrain-shaded hexes; here every plot is a plain rectangle of the
/// colour the Lua gives (its terrain is not used), the rows are not staggered.
pub struct ReplayMap {
    pub map_size: Vec2,
    /// `map_size.x * map_size.y` RGBA colours, row by row (empty until `SetMapSize`).
    pub plots: Vec<[f32; 4]>,
}

impl ReplayMap {
    /// `SetMapSize(width, height, ..)`: a new grid, every plot transparent.
    pub fn resize(&mut self, width: f32, height: f32) {
        self.map_size = Vec2::new(width, height);
        // A map is at most a few hundred plots a side; anything else is a bad call.
        let n = if (0.0..=1024.0).contains(&width) && (0.0..=1024.0).contains(&height) {
            width as usize * height as usize
        } else {
            0
        };
        self.plots = vec![[0.0; 4]; n];
    }

    /// `SetPlot(x, y, terrain, r, g, b, a)`: a plot outside the grid is ignored.
    pub fn set_plot(&mut self, x: i32, y: i32, color: [f32; 4]) {
        let (w, h) = (self.map_size.x as i32, self.map_size.y as i32);
        if x >= 0
            && y >= 0
            && x < w
            && y < h
            && let Some(p) = self.plots.get_mut((y * w + x) as usize)
        {
            *p = color;
        }
    }
}

pub struct PullDown {
    pub main: Option<ControlId>,
    pub grid: ControlId,
    pub scroll: ControlId,
    pub stack: ControlId,
    pub scroll_threshold: i32,
    pub auto_size_popup: bool,
    pub space_for_scroll: bool,
    pub instances: HashMap<String, Rc<XmlElem>>,
    pub entries: Vec<ControlId>,
    pub open: bool,
    pub callback: Option<Callback>,
    /// Where the control was last drawn from the tree; an open pull-down is drawn there.
    pub origin: Vec2,
}

/// A loaded context: the control that is the root of one XML file and its Lua.
pub struct Context {
    /// Logical path, `Assets/UI/InGame/TopPanel`.
    pub path: String,
    pub doc: Option<Rc<XmlElem>>,
    /// Local styles from the `<Styles>` element of the file.
    pub styles: Option<Rc<XmlElem>>,
    pub instances: HashMap<String, Rc<XmlElem>>,
    pub color_sets: HashMap<String, ColorSet>,
    pub default_set: ColorSet,
    pub default_font: Option<String>,
    pub default_style: String,
    /// `RegisterNamedControl`: controls with an ID, by name, plus `..` for the parent context.
    pub named: HashMap<String, ControlId>,
    pub lua: Option<lua_sys::Context>,
    pub input_handler: Option<Callback>,
    pub show_hide: Option<Callback>,
    pub post_init: Option<Callback>,
    pub update_fn: Option<Callback>,
    pub shutdown_fn: Option<Callback>,
    /// Its Lua has run.
    pub loaded: bool,
}

impl Context {
    pub fn new(path: String) -> Context {
        Context {
            path,
            doc: None,
            styles: None,
            instances: HashMap::new(),
            color_sets: HashMap::new(),
            default_set: ColorSet::default(),
            default_font: None,
            default_style: String::new(),
            named: HashMap::new(),
            lua: None,
            input_handler: None,
            show_hide: None,
            post_init: None,
            update_fn: None,
            shutdown_fn: None,
            loaded: false,
        }
    }
}

pub mod ctx_flag {
    /// `DeferLoad`.
    pub const DEFER_LOAD: u32 = 0x1000_0000;
    /// A show/hide handler is set.
    pub const HAS_SHOW_HIDE: u32 = 0x0800_0000;
    pub const POST_INIT_PENDING: u32 = 0x0100_0000;
    pub const HAS_UPDATE: u32 = 0x0080_0000;
    pub const HAS_INPUT: u32 = 0x0400_0000;
}

pub enum Kind {
    Container,
    Button(Box<Button>),
    TextButton(Box<TextButton>),
    GridButton(Box<GridButton>),
    CheckBox(Box<CheckBox>),
    RadioButton(Box<CheckBox>),
    Image(Box<Image>),
    Label(Box<Label>),
    Anim(Box<Anim>),
    Context(Box<Context>),
    Stack(Stack),
    Grid(Box<Grid>),
    ScrollPanel(Box<ScrollPanel>),
    Bar(Box<Bar>),
    Meter(Box<Meter>),
    TextureBar(Box<TextureBar>),
    ColorBox(Box<ColorBox>),
    ColorBoxButton(Box<(ButtonBase, ColorBox)>),
    Slider(Box<Slider>),
    EditBox(Box<EditBox>),
    PullDown(Box<PullDown>),
    Line(Box<Line>),
    WorldAnchor(Box<WorldAnchor>),
    ReplayMap(Box<ReplayMap>),
}

impl Kind {
    /// The number `vf_19()` returns in the exe (the order of the `GetText` format list).
    pub fn type_id(&self) -> u32 {
        match self {
            Kind::Container => 0,
            Kind::Button(_) => 1,
            Kind::Image(_) => 2,
            Kind::Label(_) => 3,
            Kind::Anim(a) => match a.variant {
                AnimVariant::Scroll => 4,
                AnimVariant::Flip => 5,
                AnimVariant::Slide => 6,
                AnimVariant::Alpha => 7,
            },
            Kind::Context(_) => 8,
            Kind::Stack(_) => 9,
            Kind::Grid(_) => 10,
            Kind::CheckBox(_) => 11,
            Kind::ScrollPanel(_) => 12,
            Kind::Bar(_) => 13,
            Kind::ColorBox(_) => 14,
            Kind::ColorBoxButton(_) => 15,
            Kind::Meter(_) => 16,
            Kind::TextButton(_) => 17,
            Kind::WorldAnchor(_) => 18,
            Kind::RadioButton(_) => 19,
            Kind::Slider(_) => 20,
            Kind::GridButton(_) => 21,
            Kind::EditBox(_) => 22,
            Kind::PullDown(_) => 23,
            Kind::TextureBar(_) => 24,
            // The exe's table has no entry for these two.
            Kind::Line(_) => 25,
            Kind::ReplayMap(_) => 26,
        }
    }

    /// Class name as `Controls.X` objects know it (`GetText` format names).
    pub fn name(&self) -> &'static str {
        match self {
            Kind::Container => "ControlBase",
            Kind::Button(_) => "Button",
            Kind::TextButton(_) => "TextButton",
            Kind::GridButton(_) => "GridButton",
            Kind::CheckBox(_) => "CheckBox",
            Kind::RadioButton(_) => "RadioButton",
            Kind::Image(_) => "Image",
            Kind::Label(_) => "Label",
            Kind::Anim(a) => match a.variant {
                AnimVariant::Scroll => "ScrollAnim",
                AnimVariant::Flip => "FlipAnim",
                AnimVariant::Slide => "SlideAnim",
                AnimVariant::Alpha => "AlphaAnim",
            },
            Kind::Context(_) => "LuaContext",
            Kind::Stack(_) => "Stack",
            Kind::Grid(_) => "Grid",
            Kind::ScrollPanel(_) => "ScrollPanel",
            Kind::Bar(_) => "Bar",
            Kind::Meter(_) => "Meter",
            Kind::TextureBar(_) => "TextureBar",
            Kind::ColorBox(_) => "Box",
            Kind::ColorBoxButton(_) => "BoxButton",
            Kind::Slider(_) => "Slider",
            Kind::EditBox(_) => "EditBox",
            Kind::PullDown(_) => "PullDown",
            Kind::Line(_) => "Line",
            Kind::WorldAnchor(_) => "WorldAnchor",
            Kind::ReplayMap(_) => "ReplayMap",
        }
    }

    pub fn button_base(&self) -> Option<&ButtonBase> {
        match self {
            Kind::Button(b) => Some(&b.base),
            Kind::TextButton(b) => Some(&b.base),
            Kind::GridButton(b) => Some(&b.base),
            Kind::CheckBox(b) | Kind::RadioButton(b) => Some(&b.base),
            Kind::ColorBoxButton(b) => Some(&b.0),
            Kind::Slider(b) => Some(&b.base),
            _ => None,
        }
    }

    pub fn button_base_mut(&mut self) -> Option<&mut ButtonBase> {
        match self {
            Kind::Button(b) => Some(&mut b.base),
            Kind::TextButton(b) => Some(&mut b.base),
            Kind::GridButton(b) => Some(&mut b.base),
            Kind::CheckBox(b) | Kind::RadioButton(b) => Some(&mut b.base),
            Kind::ColorBoxButton(b) => Some(&mut b.0),
            Kind::Slider(b) => Some(&mut b.base),
            _ => None,
        }
    }
}

/// `ControlBase`.
pub struct Node {
    pub kind: Kind,
    pub flags: u32,
    pub parent: Option<ControlId>,
    /// `m_pLuaContext`: the context that owns the control (for a context: the one that loaded it).
    pub owner: Option<ControlId>,
    pub children: Vec<ControlId>,
    pub id: String,
    pub offset: Vec2,
    pub pos: Vec2,
    pub size: Vec2,
    /// `field_24/28`: size with the offset; what auto-size and stacks measure.
    pub full: Vec2,
    /// Visibility box `field_14..20`: min x, min y, max x, max y.
    pub vis: [f32; 4],
    pub alpha: f32,
    pub tt_crc: u32,
    pub tt_string: Option<String>,
    pub tt_callback: Option<Callback>,
    pub show_on_over: Vec<ControlId>,
    pub hide_on_over: Vec<ControlId>,
}

impl Node {
    pub fn new(kind: Kind) -> Node {
        Node {
            kind,
            flags: 0,
            parent: None,
            owner: None,
            children: Vec::new(),
            id: String::new(),
            offset: Vec2::ZERO,
            pos: Vec2::ZERO,
            size: Vec2::ZERO,
            full: Vec2::ZERO,
            vis: [0.0; 4],
            alpha: 1.0,
            tt_crc: 0,
            tt_string: None,
            tt_callback: None,
            show_on_over: Vec::new(),
            hide_on_over: Vec::new(),
        }
    }

    pub fn has(&self, flags: u32) -> bool {
        self.flags & flags != 0
    }
}

struct Slot {
    ver: u32,
    node: Option<Node>,
}

/// The arena.
#[derive(Default)]
pub struct Tree {
    slots: Vec<Slot>,
    free: Vec<u32>,
    live: usize,
}

impl Tree {
    pub fn alloc(&mut self, node: Node) -> ControlId {
        self.live += 1;
        if let Some(i) = self.free.pop() {
            let s = &mut self.slots[i as usize];
            s.node = Some(node);
            ControlId {
                index: i,
                ver: s.ver,
            }
        } else {
            self.slots.push(Slot {
                ver: 1,
                node: Some(node),
            });
            ControlId {
                index: (self.slots.len() - 1) as u32,
                ver: 1,
            }
        }
    }

    pub fn get(&self, id: ControlId) -> Option<&Node> {
        self.slots
            .get(id.index as usize)
            .filter(|s| s.ver == id.ver)
            .and_then(|s| s.node.as_ref())
    }

    pub fn get_mut(&mut self, id: ControlId) -> Option<&mut Node> {
        self.slots
            .get_mut(id.index as usize)
            .filter(|s| s.ver == id.ver)
            .and_then(|s| s.node.as_mut())
    }

    pub fn alive(&self, id: ControlId) -> bool {
        self.get(id).is_some()
    }

    /// Frees one node (its children are the caller's business, see `State::destroy`).
    pub fn free_node(&mut self, id: ControlId) -> Option<Node> {
        let s = self.slots.get_mut(id.index as usize)?;
        if s.ver != id.ver {
            return None;
        }
        let n = s.node.take()?;
        s.ver = s.ver.wrapping_add(1).max(1);
        self.free.push(id.index);
        self.live -= 1;
        Some(n)
    }

    /// Controls alive (`ui.controls`).
    pub fn live(&self) -> usize {
        self.live
    }

    /// Every live id.
    pub fn ids(&self) -> impl Iterator<Item = ControlId> + '_ {
        self.slots.iter().enumerate().filter_map(|(i, s)| {
            s.node.as_ref().map(|_| ControlId {
                index: i as u32,
                ver: s.ver,
            })
        })
    }
}

impl std::ops::Index<ControlId> for Tree {
    type Output = Node;
    fn index(&self, id: ControlId) -> &Node {
        self.get(id).expect("stale ControlId")
    }
}

impl std::ops::IndexMut<ControlId> for Tree {
    fn index_mut(&mut self, id: ControlId) -> &mut Node {
        self.get_mut(id).expect("stale ControlId")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stale_id_is_recognised_after_the_slot_is_reused() {
        let mut t = Tree::default();
        let a = t.alloc(Node::new(Kind::Container));
        assert!(t.alive(a));
        t.free_node(a);
        assert!(!t.alive(a));
        let b = t.alloc(Node::new(Kind::Container));
        assert_eq!(a.index, b.index);
        assert!(t.alive(b));
        assert!(!t.alive(a), "the old id must not see the new node");
        assert_eq!(t.live(), 1);
    }

    #[test]
    fn quad_normalizes_the_source_window() {
        let mut q = Quad {
            texture: Some(TexRef {
                name: "t".into(),
                id: TextureId(1),
                w: 256.0,
                h: 128.0,
            }),
            src_x: 64.0,
            src_y: 32.0,
            src_w: 32.0,
            src_h: 16.0,
            ..Quad::default()
        };
        q.normalize();
        assert_eq!(q.uv_offset, Vec2::new(0.25, 0.25));
        assert_eq!(q.uv_scale, Vec2::new(0.125, 0.125));
        // A negative offset wraps by one (UIQuad::NormalizeTexture).
        q.src_x = -64.0;
        q.normalize();
        assert_eq!(q.uv_offset.x, 0.75);
    }
}
