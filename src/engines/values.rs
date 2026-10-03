//! Values engine: a small dashboard of 1-4 tiles (label + preformatted value +
//! unit) received over the data feed MQTT connection, e.g. live house power and
//! today's energy published by Home Assistant. The sign never does math: HA
//! sends display-ready strings.
//!
//! Layouts are specified in layout-v2.md and shared with the ESP32 firmware.

use crate::core::data_feed::{data_feed, ValueTile, ValuesData};
use crate::core::engine_contract::{
    Capabilities, ConfigField, ConfigSchema, ConfigType, Engine, EngineConfig, EngineContext,
    EngineDescriptor, EngineError, EngineMetadata, Requirements, ValidationPolicy,
};
use crate::core::i18n::{self, Lang};
use crate::core::matrix::MatrixBackend;
use crate::engines::dashboard::font::{draw_text_scaled, measure_text};
use crate::engines::renderers::tiny_font::{draw_tiny_text, tiny_text_width, TINY_H};
use linkme::distributed_slice;
use std::sync::Arc;

pub const DEFAULT_TOPIC: &str = "arcadematrix/values/energy";

const TITLE_COLOR: (u8, u8, u8) = (255, 255, 255);
const NO_DATA_COLOR: (u8, u8, u8) = (120, 120, 120);

/// Glyph height of the GLCD 5x7 font at scale 1.
const GLCD_H: i32 = 7;

/// Small text font: the 3x5 tiny font on 32 px panels, GLCD 5x7 on 64 px ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SmallFont {
    Tiny,
    Glcd,
}

impl SmallFont {
    fn height(self) -> i32 {
        match self {
            SmallFont::Tiny => TINY_H,
            SmallFont::Glcd => GLCD_H,
        }
    }

    fn width(self, text: &str) -> i32 {
        match self {
            SmallFont::Tiny => tiny_text_width(text),
            SmallFont::Glcd => measure_text(text),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw(
        self,
        m: &mut dyn MatrixBackend,
        text: &str,
        x: i32,
        y: i32,
        min_x: i32,
        max_x: i32,
        c: (u8, u8, u8),
    ) {
        let h = m.height() as i32;
        match self {
            SmallFont::Tiny => draw_tiny_text(m, text, x, y, min_x, max_x, c),
            SmallFont::Glcd => draw_text_scaled(m, text, x, y, min_x, max_x, 0, h, 1, c),
        }
    }
}

/// GLCD scales tried for the value, biggest first (layout-v2.md "Per-panel fonts").
fn value_chain(tall: bool, n: usize) -> &'static [i32] {
    match (tall, n) {
        (false, 1) | (false, 2) => &[3, 2, 1],
        (false, 3) => &[2, 1],
        (false, _) => &[1],
        (true, 1) => &[5, 4, 3, 2],
        (true, 2) => &[4, 3, 2],
        (true, _) => &[3, 2, 1],
    }
}

/// Result of fitting a value (+ unit) into a cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ValueFit {
    pub scale: i32,
    pub show_unit: bool,
    /// Number of value characters drawn (always the whole value).
    pub chars: usize,
}

impl ValueFit {
    fn unit_font(&self) -> SmallFont {
        if self.scale >= 2 {
            SmallFont::Glcd
        } else {
            SmallFont::Tiny
        }
    }

    fn value_h(&self) -> i32 {
        GLCD_H * self.scale
    }
}

fn glcd_width(chars: usize, scale: i32) -> i32 {
    if chars == 0 {
        0
    } else {
        (chars as i32 * 6 - 1) * scale
    }
}

/// Space between value and unit: one pixel more than the value's own letter
/// spacing, so "17.0h" doesn't read as one word.
fn unit_gap(fit: &ValueFit) -> i32 {
    fit.scale + 1
}

/// Total width of value (+ gap + unit) for a fit.
fn line_width(value_chars: usize, unit: &str, fit: &ValueFit) -> i32 {
    let mut w = glcd_width(value_chars, fit.scale);
    if fit.show_unit && !unit.is_empty() {
        w += unit_gap(fit) + fit.unit_font().width(unit);
    }
    w
}

/// Fit rule from layout-v2.md: biggest font with the unit, then without the
/// unit, then the smallest font. The value itself is never truncated.
pub fn fit_value(value: &str, unit: &str, chain: &[i32], aw: i32, ah: i32) -> ValueFit {
    let n = value.chars().count();
    let smallest = *chain.last().unwrap_or(&1);
    for &scale in chain {
        let f = ValueFit {
            scale,
            show_unit: !unit.is_empty(),
            chars: n,
        };
        if f.value_h() <= ah && line_width(n, unit, &f) <= aw {
            return f;
        }
    }
    for &scale in chain {
        let f = ValueFit {
            scale,
            show_unit: false,
            chars: n,
        };
        if f.value_h() <= ah && line_width(n, unit, &f) <= aw {
            return f;
        }
    }
    // Never truncate a value: draw it in full at the smallest size.
    ValueFit {
        scale: smallest,
        show_unit: false,
        chars: n,
    }
}

/// First `chars` characters of `s` without allocating.
fn prefix(s: &str, chars: usize) -> &str {
    match s.char_indices().nth(chars) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

/// Draws value + unit with the line's top-left at (x, y) (y = value top).
fn draw_value_line(m: &mut dyn MatrixBackend, t: &ValueTile, fit: &ValueFit, x: i32, y: i32) {
    let w = m.width() as i32;
    let h = m.height() as i32;
    let v = prefix(&t.value, fit.chars);
    draw_text_scaled(m, v, x, y, 0, w, 0, h, fit.scale, t.color);
    if fit.show_unit && !t.unit.is_empty() {
        let uf = fit.unit_font();
        let ux = x + glcd_width(fit.chars, fit.scale) + unit_gap(fit);
        let uy = y + fit.value_h() - uf.height();
        uf.draw(m, &t.unit, ux, uy, 0, w, t.label_color);
    }
}

/// The label text and font chosen for a cell (layout-v2.md "Label choice").
struct LabelPick<'a> {
    text: &'a str,
    font: SmallFont,
}

/// Full label at the intended font, then the short form, then the fallback
/// font, then the shortest text truncated from the end.
fn pick_label<'a>(t: &'a ValueTile, tall: bool, iw: i32) -> Option<LabelPick<'a>> {
    if t.label.is_empty() && t.label_short.is_empty() {
        return None;
    }
    let intended = if tall {
        SmallFont::Glcd
    } else {
        SmallFont::Tiny
    };
    let mut fonts: &[SmallFont] = &[SmallFont::Tiny];
    if tall {
        fonts = &[SmallFont::Glcd, SmallFont::Tiny];
    }
    let texts = [t.label.as_str(), t.label_short.as_str()];
    for &font in fonts {
        for text in texts.iter().filter(|s| !s.is_empty()) {
            if font.width(text) <= iw {
                return Some(LabelPick { text, font });
            }
        }
    }
    let font = *fonts.last().unwrap_or(&intended);
    let shortest = if t.label_short.is_empty() {
        t.label.as_str()
    } else {
        t.label_short.as_str()
    };
    let mut chars = shortest.chars().count();
    while chars > 0 && font.width(prefix(shortest, chars)) > iw {
        chars -= 1;
    }
    Some(LabelPick {
        text: prefix(shortest, chars),
        font,
    })
}

#[allow(clippy::too_many_arguments)]
/// Draws one stacked tile (label on top, value + unit below), centred in the
/// inner box `[ix, ix + iw)` x `[top, top + ch)`.
fn draw_tile(
    m: &mut dyn MatrixBackend,
    t: &ValueTile,
    chain: &[i32],
    tall: bool,
    ix: i32,
    iw: i32,
    cell_top: i32,
    ch: i32,
) {
    let lg = if tall { 2 } else { 1 };
    let label = pick_label(t, tall, iw);
    let label_block = label.as_ref().map(|l| l.font.height() + lg).unwrap_or(0);
    let fit = fit_value(&t.value, &t.unit, chain, iw, ch - label_block);
    let block_h = label_block + fit.value_h();
    let top = cell_top + (ch - block_h) / 2;
    if let Some(l) = label {
        let lw = l.font.width(l.text);
        l.font.draw(
            m,
            l.text,
            ix + (iw - lw) / 2,
            top,
            ix,
            ix + iw,
            t.label_color,
        );
    }
    let line_w = line_width(fit.chars, &t.unit, &fit);
    draw_value_line(m, t, &fit, ix + (iw - line_w) / 2, top + label_block);
}

/// Draws the whole screen for `data` (or the "no data" notice).
pub fn render_values(
    m: &mut dyn MatrixBackend,
    data: Option<&ValuesData>,
    show_title: bool,
    no_data_label: &str,
) {
    let w = m.width() as i32;
    let h = m.height() as i32;
    m.clear();

    let Some(d) = data.filter(|d| !d.tiles.is_empty()) else {
        let tw = measure_text(no_data_label);
        draw_text_scaled(
            m,
            no_data_label,
            ((w - tw) / 2).max(0),
            ((h - GLCD_H) / 2).max(0),
            0,
            w,
            0,
            h,
            1,
            NO_DATA_COLOR,
        );
        return;
    };

    let tall = h >= 64;
    let n = d.tiles.len().min(4);
    let chain = value_chain(tall, n);

    if n == 4 {
        // 2x2 grid of stacked tiles, no title; 4 px gutter between columns.
        let (cw, chh) = (w / 2, h / 2);
        for (i, t) in d.tiles.iter().take(4).enumerate() {
            let (c, r) = ((i % 2) as i32, (i / 2) as i32);
            let x = c * cw;
            let ix = x + if c == 1 { 2 } else { 0 };
            let cell_end = x + cw - if c == 0 { 2 } else { 0 };
            draw_tile(m, t, chain, tall, ix, cell_end - ix, r * chh, chh);
        }
        return;
    }

    let titled = show_title && !d.title.is_empty();
    let ct = if titled {
        draw_text_scaled(m, &d.title, 0, 0, 0, w, 0, h, 1, TITLE_COLOR);
        if tall {
            9
        } else {
            8
        }
    } else {
        0
    };

    for (i, t) in d.tiles.iter().take(n).enumerate() {
        let cell_x = (i as i32 * w) / n as i32;
        // 4 px gutter between columns: 2 px off each neighbouring cell.
        let cell_end = ((i as i32 + 1) * w) / n as i32 - if i + 1 < n { 2 } else { 0 };
        let ix = cell_x + if i > 0 { 2 } else { 0 };
        draw_tile(m, t, chain, tall, ix, cell_end - ix, ct, h - ct);
    }
}

fn lang_from_code(code: &str) -> Lang {
    let c = code.trim();
    if c.eq_ignore_ascii_case("en") {
        Lang::En
    } else if c.eq_ignore_ascii_case("es") {
        Lang::Es
    } else {
        Lang::Fr
    }
}

pub struct ValuesEngine {
    topic: String,
    show_title: bool,
    data: Option<Arc<ValuesData>>,
    seen_generation: u64,
    lang: Lang,
}

impl Default for ValuesEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl ValuesEngine {
    pub fn new() -> Self {
        Self {
            topic: DEFAULT_TOPIC.to_string(),
            show_title: true,
            data: None,
            seen_generation: u64::MAX,
            lang: Lang::En,
        }
    }

    fn read_config(&mut self, config: &dyn EngineConfig) {
        let topic = config.get_string("topic", DEFAULT_TOPIC);
        let topic = topic.trim();
        self.topic = if topic.is_empty() {
            DEFAULT_TOPIC.to_string()
        } else {
            topic.to_string()
        };
        self.show_title = config.get_bool("show_title", true);
        self.seen_generation = u64::MAX;
    }

    fn refresh_data(&mut self) {
        let store = data_feed();
        let generation = store.generation();
        if generation == self.seen_generation {
            return;
        }
        self.seen_generation = generation;
        self.data = store.get(&self.topic).and_then(|e| e.values.clone());
    }

    /// The tiles currently shown (for tests and diagnostics).
    pub fn current(&self) -> Option<&ValuesData> {
        self.data.as_deref()
    }
}

impl Engine for ValuesEngine {
    fn initialize(
        &mut self,
        ctx: &mut EngineContext,
        config: &dyn EngineConfig,
    ) -> Result<(), EngineError> {
        self.read_config(config);
        self.lang = lang_from_code(&ctx.config.settings.read().system.lang);
        self.refresh_data();
        Ok(())
    }

    fn activate(&mut self) {
        self.seen_generation = u64::MAX;
    }

    fn update(&mut self, ctx: &mut EngineContext) {
        self.lang = lang_from_code(&ctx.config.settings.read().system.lang);
        self.refresh_data();
    }

    fn render(&mut self, ctx: &mut EngineContext) {
        render_values(
            ctx.matrix,
            self.data.as_deref(),
            self.show_title,
            i18n::no_data_label(self.lang),
        );
    }

    fn deactivate(&mut self) {}

    fn on_config_changed(&mut self, config: &dyn EngineConfig) {
        self.read_config(config);
        self.refresh_data();
    }
}

#[distributed_slice(crate::core::registry::ENGINES)]
fn register_values_engine() -> EngineDescriptor {
    EngineDescriptor {
        metadata: EngineMetadata {
            id: "values",
            name: "Values",
            category: "info",
            version: crate::core::build_info::VERSION,
        },
        capabilities: Capabilities::default(),
        requirements: Requirements {
            needs_network: true,
            ..Default::default()
        },
        available: true,
        unavailable_reason: None,
        schema: ConfigSchema {
            fields: vec![
                ConfigField {
                    id: "topic",
                    field_type: ConfigType::String,
                    label: "MQTT Topic",
                    description: "Full topic of the values payload on the Data Feed MQTT broker",
                    default_value: DEFAULT_TOPIC,
                    validation_policy: ValidationPolicy::Accept,
                    ..Default::default()
                },
                ConfigField {
                    id: "show_title",
                    field_type: ConfigType::Boolean,
                    label: "Show Title",
                    description:
                        "Show the payload title on top when the layout has room (1-3 tiles)",
                    default_value: "true",
                    validation_policy: ValidationPolicy::FallbackDefault,
                    ..Default::default()
                },
            ],
        },
        factory: || Box::new(ValuesEngine::new()),
    }
}
