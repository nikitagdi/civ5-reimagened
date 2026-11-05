//! `TextEngine` of the client: `ui-text` for the layout of a string, `loc` for the texts.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

use glam::Vec2;
use ui::{LocArg, TextEngine, TextLayout, TextQuad, TextRequest};
use ui_text::{FontId, FontSource, TextParams, TextSystem, pack_control_color};

use crate::store::UiStore;

/// Texts of one language over the `C5LT` table of the set.
pub struct LocaleRows(pub asset_format::LocaleTable);

impl loc::TextTable for LocaleRows {
    fn get(&self, tag: &str) -> Option<loc::TextEntry<'_>> {
        let text = self.0.get(tag)?;
        let (gender, plurality) = self.0.grammar_of(tag);
        Some(loc::TextEntry {
            text,
            gender,
            plurality,
        })
    }
}

/// Icon fonts (`FontIcons.ggxml`) come from the store when the first icon of one is drawn.
struct StoreFonts(Rc<UiStore>);

impl FontSource for StoreFonts {
    fn ggxml(&self, file_stem: &str) -> Option<String> {
        self.0.text_of_name(&format!("{file_stem}.ggxml"))
    }
}

pub struct ClientText {
    system: TextSystem,
    loc: Rc<loc::Localizer>,
    store: Rc<UiStore>,
    /// Fonts by lower-case name: the index spells names in lower case and the XML any way.
    fonts: HashMap<String, FontId>,
    /// The font a request falls back to when its name is not loaded.
    fallback: Option<FontId>,
    /// Counters of the layouts, for the layer to read back.
    stats: Rc<Cell<ui_text::TextStats>>,
}

impl ClientText {
    /// Loads every font of the index (`UiIndex::fonts`) whose `.ggxml` has arrived.
    pub fn new(
        store: Rc<UiStore>,
        lang: &str,
        texts: asset_format::LocaleTable,
        stats: Rc<Cell<ui_text::TextStats>>,
    ) -> ClientText {
        let loc = Rc::new(make_localizer(&store, lang, texts));
        Self::with_localizer(store, loc, stats)
    }

    /// [`ClientText::new`] over a localizer that exists already (the exe API's `Locale` shares it).
    pub fn with_localizer(
        store: Rc<UiStore>,
        loc: Rc<loc::Localizer>,
        stats: Rc<Cell<ui_text::TextStats>>,
    ) -> ClientText {
        let tables = store
            .bytes(asset_format::UI_TABLES_ID)
            .and_then(|b| match asset_format::UiTables::decode(&b) {
                Ok(t) => Some(t),
                Err(e) => {
                    tracing::warn!("ui:tables: {e}");
                    None
                }
            })
            .unwrap_or_default();
        let mut icons = ui_text::IconTable::new();
        for (key, file) in &tables.icon_textures {
            icons.add_texture(key, file);
        }
        for i in &tables.icons {
            icons.add_icon(&i.name, i.glyph, &i.texture);
        }
        let mut colors = ui_text::ColorTable::new();
        for (name, c) in &tables.colors {
            colors.add(name, *c);
        }
        let mut system = TextSystem::new(icons, colors);
        system.set_font_source(Box::new(StoreFonts(Rc::clone(&store))));
        let mut fonts = HashMap::new();
        for f in &store.index().fonts {
            let Some(text) = store.text_of_name(&f.ggxml) else {
                tracing::warn!(font = %f.name, file = %f.ggxml, "font file is not loaded");
                continue;
            };
            match system.load_font(&f.name, &text) {
                Ok(id) => {
                    check_atlases(&store, &f.name, f, system.font_data(id));
                    fonts.insert(f.name.to_ascii_lowercase(), id);
                }
                Err(e) => tracing::warn!("{e}"),
            }
        }
        let fallback = fonts
            .get("twcenmt16")
            .or_else(|| fonts.values().next())
            .copied();
        ClientText {
            system,
            loc,
            store,
            fonts,
            fallback,
            stats,
        }
    }
}

/// The localizer over the texts of `lang`.
pub fn make_localizer(
    store: &UiStore,
    lang: &str,
    texts: asset_format::LocaleTable,
) -> loc::Localizer {
    let tables = store
        .bytes(asset_format::UI_TABLES_ID)
        .and_then(|b| asset_format::UiTables::decode(&b).ok())
        .unwrap_or_default();
    let info = match tables.languages.iter().find(|l| l.lang == lang) {
        Some(l) => loc::LanguageInfo::new(lang, lang, l.plural_rule),
        // SIMPLIFIED(SIMP-uicl-4): the game data gives only en_US its `PluralRule`; another language composes by rule 2.
        None => loc::LanguageInfo::new(lang, lang, 2),
    };
    loc::Localizer::new(info, Box::new(LocaleRows(texts)))
}

/// The `.ggxml` normalises glyph rectangles by its own texture size; an atlas of another size draws garbage.
fn check_atlases(store: &UiStore, name: &str, font: &asset_format::UiFont, parsed: &ui_text::Font) {
    for t in &font.textures {
        let Some(blob) = store
            .texture_id_of_name(t)
            .and_then(|id| store.texture_blob(id))
        else {
            continue;
        };
        if (blob.width, blob.height) != parsed.texture_size {
            tracing::warn!(
                font = name,
                texture = %t,
                got = ?(blob.width, blob.height),
                want = ?parsed.texture_size,
                "the font texture is not the size its .ggxml names; its glyphs will not line up"
            );
        }
    }
}

impl ClientText {
    fn font(&self, name: &str) -> Option<FontId> {
        self.fonts
            .get(&name.to_ascii_lowercase())
            .copied()
            .or(self.fallback)
    }
}

fn loc_arg(a: &LocArg) -> loc::Arg {
    match a {
        LocArg::Bool(b) => loc::Arg::from_bool(*b),
        LocArg::Number(n) => {
            if n.fract() == 0.0 && n.abs() < f64::from(i32::MAX) {
                loc::Arg::Int(*n as i32)
            } else {
                loc::Arg::Double(*n)
            }
        }
        LocArg::Str(s) => loc::Arg::Str(s.clone()),
    }
}

impl TextEngine for ClientText {
    fn layout(&mut self, req: &TextRequest) -> TextLayout {
        let Some(font) = self.font(&req.font) else {
            return TextLayout::default();
        };
        let mut p = TextParams::new(&req.text, font);
        p.style = self.system.style_index(font, &req.style);
        p.colors = req.colors.map(pack_control_color);
        p.wrap_width = req.wrap_width;
        p.truncate_width = req.truncate_width;
        p.leading_offset = req.leading_offset;
        p.parse = req.parse_markup;
        p.obscured = req.obscured;
        let laid = self.system.layout(&p);
        self.stats.set(self.system.stats());
        // A font texture that is not in the store draws nothing, as `TextLayout::emit` does.
        let ids: Vec<_> = laid
            .textures
            .iter()
            .map(|t| self.store.texture_id_of_name(&t.src))
            .collect();
        let quads = laid
            .quads
            .iter()
            .filter_map(|q| {
                Some(TextQuad {
                    texture: ids[q.texture as usize]?,
                    min: Vec2::new(q.rect[0], q.rect[1]),
                    max: Vec2::new(q.rect[2], q.rect[3]),
                    uv_min: Vec2::new(q.uv[0], q.uv[1]),
                    uv_max: Vec2::new(q.uv[2], q.uv[3]),
                    color: q.color,
                })
            })
            .collect();
        TextLayout {
            size: Vec2::new(laid.size[0] as f32, laid.size[1] as f32),
            quads,
            truncated: laid.truncated,
        }
    }

    fn measure(&mut self, text: &str, font: &str) -> i32 {
        match self.font(font) {
            Some(f) => self.system.measure(text, f, false),
            None => 0,
        }
    }

    fn localize(&mut self, key: &str, args: &[LocArg]) -> String {
        let args: Vec<loc::Arg> = args.iter().map(loc_arg).collect();
        self.loc.convert(key, &args)
    }
}
