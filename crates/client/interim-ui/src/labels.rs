//! Unit flags and city banners over the map (play-ui §3.3, AC-4.3, AC-4.4), drawn with the egui
//! painter on the background layer at the screen points of `entity-render`'s `WorldLabel`s.
//!
//! * **Flag:** owner's colours (fill and border), a two-letter type mark, an HP bar when wounded, a
//!   yellow dot when the unit needs orders, a yellow border when selected. Civilians get a smaller
//!   flag. A flag is drawn over everything on the map, so a unit in a forest stays visible.
//! * **Banner:** population and name in the owner's colours, a star on a capital, the current
//!   production and its turns for an own city, an HP bar when damaged, and a "Strike" button when
//!   the city can shoot now.
//!
//! Far labels are drawn first. Left and right clicks on a flag or banner come back as
//! [`UiAction::LabelClicked`]; the map under a label does not get those presses ([`hit`]).
//!
//! SIMPLIFIED(SIMP-014): own flags and banners (a text mark instead of the unit's flag icon, no
//! banner art); original: `UnitFlagManager` and `CityBannerManager` Lua screens with icon atlases.

use std::sync::Arc;

use egui::{
    Align2, Color32, CornerRadius, FontId, Id, Pos2, Rect, Sense, Stroke, StrokeKind, vec2,
};

use crate::screens::UiAction;

/// What a label stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelTarget {
    Unit(i32),
    City(i32),
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct FlagView {
    pub unit: i32,
    /// The plot the unit stands on.
    pub cell: (i32, i32),
    /// Screen point (egui points): the flag's bottom centre.
    pub pos: [f32; 2],
    /// NDC depth; larger is farther.
    pub depth: f32,
    /// Owner colours, 0xRRGGBBAA.
    pub primary: u32,
    pub secondary: u32,
    /// Two letters of the unit type ("Wa").
    pub mark: String,
    /// HP as 0..1 when wounded, `None` at full health.
    pub health: Option<f32>,
    pub needs_orders: bool,
    pub selected: bool,
    pub civilian: bool,
    /// "Warrior (Egypt), HP 7/10".
    pub tooltip: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct BannerView {
    pub city: i32,
    pub cell: (i32, i32),
    /// Screen point (egui points) over the city.
    pub pos: [f32; 2],
    pub depth: f32,
    pub primary: u32,
    pub secondary: u32,
    pub name: String,
    /// 0 when unknown (a foreign city out of sight).
    pub population: i32,
    pub capital: bool,
    /// "Warrior 4" for an own city.
    pub production: Option<String>,
    pub health: Option<f32>,
    /// The city can shoot now (the brain listed targets for it).
    pub strike: bool,
    pub tooltip: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct MapLabelsView {
    pub flags: Vec<FlagView>,
    pub banners: Vec<BannerView>,
}

/// The rectangles of the last frame's labels: presses there are the labels', not the map's.
#[derive(Debug, Clone, Default)]
struct Hits(Arc<Vec<Rect>>);

fn hits_id() -> Id {
    Id::unique("map-label-hits")
}

/// Is `pos` (points) on a flag or banner drawn in the last frame?
pub fn hit(ctx: &egui::Context, pos: Pos2) -> bool {
    ctx.data(|d| d.get_temp::<Hits>(hits_id()))
        .is_some_and(|h| h.0.iter().any(|r| r.contains(pos)))
}

fn color(rgba: u32) -> Color32 {
    let [r, g, b, a] = rgba.to_be_bytes();
    Color32::from_rgba_unmultiplied(r, g, b, a.max(200))
}

/// The text colour on `fill`: the owner's second colour, unless it is too close to the fill.
fn text_on(fill: Color32, secondary: u32) -> Color32 {
    let s = color(secondary);
    let luma =
        |c: Color32| 0.299 * f32::from(c.r()) + 0.587 * f32::from(c.g()) + 0.114 * f32::from(c.b());
    if (luma(fill) - luma(s)).abs() > 60.0 {
        s
    } else if luma(fill) > 128.0 {
        Color32::BLACK
    } else {
        Color32::WHITE
    }
}

/// The capital's star before the city name: its outer radius and the room it takes.
const STAR_RADIUS: f32 = 6.0;
const STAR_WIDTH: f32 = 15.0;

/// A five-pointed star (the default fonts have no star glyph): a pentagon and five points.
fn star(painter: &egui::Painter, centre: Pos2, r: f32, fill: Color32) {
    let at = |i: usize, radius: f32| {
        let a = (-90.0 + 36.0 * i as f32).to_radians();
        centre + vec2(a.cos(), a.sin()) * radius
    };
    let inner: Vec<Pos2> = (0..5).map(|i| at(2 * i + 1, r * 0.42)).collect();
    painter.add(egui::Shape::convex_polygon(
        inner.clone(),
        fill,
        Stroke::NONE,
    ));
    for i in 0..5 {
        let points = vec![inner[(i + 4) % 5], at(2 * i, r), inner[i]];
        painter.add(egui::Shape::convex_polygon(points, fill, Stroke::NONE));
    }
}

const SELECTED: Color32 = Color32::from_rgb(240, 230, 115);
const NEEDS_ORDERS: Color32 = Color32::from_rgb(255, 220, 40);

fn hp_bar(painter: &egui::Painter, rect: Rect, health: f32) {
    painter.rect_filled(rect, 0.0, Color32::from_black_alpha(200));
    let mut fill = rect.shrink(1.0);
    fill.set_width(fill.width() * health.clamp(0.0, 1.0));
    painter.rect_filled(fill, 0.0, crate::screens::unit_panel::health_color(health));
}

pub fn show(ctx: &egui::Context, view: &MapLabelsView) -> Vec<UiAction> {
    let mut actions = Vec::new();
    let mut hits = Vec::new();
    let screen = ctx.content_rect();
    egui::Area::new(Id::unique("map-labels"))
        .order(egui::Order::Background)
        .fixed_pos(screen.min)
        .constrain(false)
        .show(ctx, |ui| {
            // No size: the area must not cover the map, or egui would claim every press there (its
            // widgets are hit wherever they are; `hit` routes the presses on labels).
            // Far first, so near labels cover them.
            enum L<'a> {
                F(&'a FlagView),
                B(&'a BannerView),
            }
            let mut all: Vec<(f32, L<'_>)> = view
                .flags
                .iter()
                .map(|f| (f.depth, L::F(f)))
                .chain(view.banners.iter().map(|b| (b.depth, L::B(b))))
                .collect();
            all.sort_by(|a, b| b.0.total_cmp(&a.0));
            for (_, l) in all {
                match l {
                    L::F(f) => flag(ui, f, &mut actions, &mut hits),
                    L::B(b) => banner(ui, b, &mut actions, &mut hits),
                }
            }
        });
    ctx.data_mut(|d| d.insert_temp(hits_id(), Hits(Arc::new(hits))));
    actions
}

fn clicks(r: &egui::Response, target: LabelTarget, cell: (i32, i32), actions: &mut Vec<UiAction>) {
    if r.clicked() {
        actions.push(UiAction::LabelClicked {
            target,
            cell,
            secondary: false,
        });
    } else if r.secondary_clicked() {
        actions.push(UiAction::LabelClicked {
            target,
            cell,
            secondary: true,
        });
    }
}

fn flag(ui: &mut egui::Ui, f: &FlagView, actions: &mut Vec<UiAction>, hits: &mut Vec<Rect>) {
    let size = if f.civilian {
        vec2(22.0, 17.0)
    } else {
        vec2(28.0, 21.0)
    };
    let at = Pos2::new(f.pos[0], f.pos[1]);
    let rect = Rect::from_min_size(at - vec2(size.x / 2.0, size.y), size);
    let painter = ui.painter();
    let fill = color(f.primary);
    let border = if f.selected {
        Stroke::new(2.5, SELECTED)
    } else {
        Stroke::new(1.5, color(f.secondary))
    };
    let round = if f.civilian { 8.0 } else { 3.0 };
    painter.rect(
        rect,
        CornerRadius::same(round as u8),
        fill,
        border,
        StrokeKind::Outside,
    );
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        &f.mark,
        FontId::proportional(if f.civilian { 11.0 } else { 13.0 }),
        text_on(fill, f.secondary),
    );
    if let Some(h) = f.health {
        let bar = Rect::from_min_size(rect.left_bottom() + vec2(0.0, 2.0), vec2(size.x, 5.0));
        hp_bar(painter, bar, h);
    }
    if f.needs_orders {
        painter.circle(
            rect.right_top() + vec2(-1.0, 1.0),
            3.5,
            NEEDS_ORDERS,
            Stroke::new(1.0, Color32::BLACK),
        );
    }
    // Unit and city ids are unique per owner only: the plot tells two owners apart.
    let r = ui
        .interact(
            rect,
            Id::unique(("unit-flag", f.unit, f.cell)),
            Sense::click(),
        )
        .on_hover_text(&f.tooltip);
    clicks(&r, LabelTarget::Unit(f.unit), f.cell, actions);
    hits.push(rect.expand(2.0));
}

fn banner(ui: &mut egui::Ui, b: &BannerView, actions: &mut Vec<UiAction>, hits: &mut Vec<Rect>) {
    let fill = color(b.primary);
    let text = text_on(fill, b.secondary);
    let painter = ui.painter();
    let star_w = if b.capital { STAR_WIDTH } else { 0.0 };
    let name = painter.layout_no_wrap(b.name.clone(), FontId::proportional(14.0), text);
    let pop = painter.layout_no_wrap(
        // A known city out of sight has no live population.
        if b.population > 0 {
            b.population.to_string()
        } else {
            "?".to_owned()
        },
        FontId::proportional(13.0),
        text,
    );
    let prod = b
        .production
        .as_ref()
        .map(|p| painter.layout_no_wrap(p.clone(), FontId::proportional(11.0), text));
    let pad = 6.0;
    let pop_w = pop.size().x.max(14.0) + 6.0;
    let mut w = pop_w + pad + star_w + name.size().x + pad;
    if let Some(p) = &prod {
        w = w.max(pop_w + pad + p.size().x + pad);
    }
    let h = 20.0 + prod.as_ref().map_or(0.0, |p| p.size().y);
    // Centred on the anchor (`CityBannerManager`: the 160 x 48 `CityBannerButton` is centred on the
    // projected point, re-b17 3.4).
    let rect = banner_rect(b.pos, w, h);
    painter.rect(
        rect,
        CornerRadius::same(4),
        fill,
        Stroke::new(1.5, color(b.secondary)),
        StrokeKind::Outside,
    );
    let pop_rect = Rect::from_min_size(rect.min, vec2(pop_w, h));
    painter.rect_filled(
        pop_rect,
        CornerRadius::same(4),
        Color32::from_black_alpha(90),
    );
    painter.galley(pop_rect.center() - pop.size() / 2.0, pop, text);
    let x = rect.min.x + pop_w + pad;
    if b.capital {
        star(
            painter,
            Pos2::new(x + STAR_RADIUS, rect.min.y + 10.0),
            STAR_RADIUS,
            text,
        );
    }
    painter.galley(Pos2::new(x + star_w, rect.min.y + 2.0), name, text);
    if let Some(p) = prod {
        painter.galley(Pos2::new(x, rect.min.y + 19.0), p, text);
    }
    if let Some(hp) = b.health {
        let bar = Rect::from_min_size(rect.left_bottom() + vec2(0.0, 2.0), vec2(w, 5.0));
        hp_bar(painter, bar, hp);
    }
    let r = ui
        .interact(
            rect,
            Id::unique(("city-banner", b.city, b.cell)),
            Sense::click(),
        )
        .on_hover_text(&b.tooltip);
    clicks(&r, LabelTarget::City(b.city), b.cell, actions);
    hits.push(rect.expand(2.0));
    if b.strike {
        let s = Rect::from_min_size(rect.left_top() - vec2(52.0, 0.0), vec2(48.0, 20.0));
        let painter = ui.painter();
        painter.rect(
            s,
            CornerRadius::same(4),
            Color32::from_rgb(170, 30, 30),
            Stroke::new(1.5, Color32::from_rgb(255, 200, 200)),
            StrokeKind::Outside,
        );
        painter.text(
            s.center(),
            Align2::CENTER_CENTER,
            "Strike",
            FontId::proportional(12.0),
            Color32::WHITE,
        );
        let r = ui
            .interact(
                s,
                Id::unique(("city-strike", b.city, b.cell)),
                Sense::click(),
            )
            .on_hover_text("Ranged attack from the city");
        if r.clicked() {
            actions.push(UiAction::CityRangeAttack(b.city));
        }
        hits.push(s.expand(2.0));
    }
}

/// The box of a city banner of `w` by `h` at the screen point `pos` of its city: centred on it.
fn banner_rect(pos: [f32; 2], w: f32, h: f32) -> Rect {
    Rect::from_center_size(Pos2::new(pos[0], pos[1]), vec2(w, h))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::{click_at, find_widget, run_headless};

    #[test]
    fn a_city_banner_is_centred_on_the_point_of_its_city() {
        let r = banner_rect([400.0, 300.0], 160.0, 48.0);
        assert_eq!(r.center(), Pos2::new(400.0, 300.0));
        assert_eq!(r.size(), vec2(160.0, 48.0));
    }

    fn view() -> MapLabelsView {
        MapLabelsView {
            flags: vec![
                FlagView {
                    unit: 4,
                    cell: (8, 11),
                    pos: [400.0, 300.0],
                    depth: 0.5,
                    primary: 0xB0_20_20_FF,
                    secondary: 0xF0_D0_40_FF,
                    mark: "Ar".into(),
                    health: Some(0.6),
                    needs_orders: true,
                    selected: true,
                    civilian: false,
                    tooltip: "Your Archer".into(),
                },
                FlagView {
                    unit: 9,
                    cell: (13, 9),
                    pos: [600.0, 300.0],
                    depth: 0.6,
                    mark: "Wa".into(),
                    ..FlagView::default()
                },
            ],
            banners: vec![BannerView {
                city: 1,
                cell: (6, 12),
                pos: [200.0, 400.0],
                depth: 0.4,
                primary: 0xB0_20_20_FF,
                secondary: 0xF0_D0_40_FF,
                name: "Roma".into(),
                population: 3,
                capital: true,
                production: Some("Warrior 4".into()),
                health: Some(0.5),
                strike: true,
                tooltip: "Roma".into(),
            }],
        }
    }

    #[test]
    fn flags_and_banners_draw_their_text() {
        let v = view();
        for t in ["Ar", "Wa", "Roma", "3", "Warrior 4", "Strike"] {
            assert!(find_widget(t, |ctx| show(ctx, &v)).is_some(), "{t}");
        }
        assert!(run_headless(|ctx| drop(show(ctx, &MapLabelsView::default()))).is_empty());
    }

    #[test]
    fn clicks_on_labels_and_the_strike_button() {
        let v = view();
        let flag = find_widget("Ar", |ctx| show(ctx, &v)).unwrap().center();
        assert_eq!(
            click_at(flag, false, |ctx| show(ctx, &v)),
            [UiAction::LabelClicked {
                target: LabelTarget::Unit(4),
                cell: (8, 11),
                secondary: false
            }]
        );
        let banner = find_widget("Roma", |ctx| show(ctx, &v)).unwrap().center();
        assert_eq!(
            click_at(banner, false, |ctx| show(ctx, &v)),
            [UiAction::LabelClicked {
                target: LabelTarget::City(1),
                cell: (6, 12),
                secondary: false
            }]
        );
        let strike = find_widget("Strike", |ctx| show(ctx, &v)).unwrap().center();
        assert_eq!(
            click_at(strike, false, |ctx| show(ctx, &v)),
            [UiAction::CityRangeAttack(1)]
        );
    }

    #[test]
    fn the_map_does_not_get_presses_on_labels() {
        let v = view();
        let ctx = egui::Context::default();
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1280.0, 720.0))),
            ..egui::RawInput::default()
        };
        for _ in 0..2 {
            ctx.run_pass(raw.clone(), |ctx| drop(show(ctx, &v)))
                .drop_without_applying_deltas();
        }
        assert!(hit(&ctx, Pos2::new(400.0, 290.0)), "on the archer's flag");
        assert!(!hit(&ctx, Pos2::new(900.0, 600.0)), "open map");
        let area = ctx.memory(|m| m.area_rect(Id::unique("map-labels")));
        assert!(
            area.is_some_and(|r| r.area() == 0.0),
            "the labels' area does not cover the map: {area:?}"
        );
    }
}
