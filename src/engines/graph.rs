//! Graph engine: draws a bar/area chart from a JSON payload received over the
//! data feed MQTT connection (`data_mqtt`), e.g. 24 h of air-conditioning power
//! published by Home Assistant.
//!
//! Data path: the data MQTT thread parses the payload once into a
//! [`GraphData`] (see `core::data_feed`); this engine only swaps an `Arc` when
//! the store's generation changes, so `update()`/`render()` never parse JSON or
//! allocate.

use crate::core::data_feed::{data_feed, GraphData};
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

pub const DEFAULT_TOPIC: &str = "arcadematrix/graph/ac";

const TITLE_COLOR: (u8, u8, u8) = (255, 255, 255);
const NO_DATA_COLOR: (u8, u8, u8) = (120, 120, 120);
const GRID_COLOR: (u8, u8, u8) = (0x20, 0x20, 0x20);
const ZERO_COLOR: (u8, u8, u8) = (0x30, 0x30, 0x30);
const AXIS_COLOR: (u8, u8, u8) = (0x70, 0x70, 0x70);

/// Vertical layout of the graph for a given panel size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GraphLayout {
    /// Text scale for the 5x7 header font (1 on 32 px panels, 2 on 64 px).
    pub text_scale: i32,
    pub header: bool,
    /// First row of the plot area.
    pub graph_top: i32,
    /// Plot height in rows (graph_top..height).
    pub graph_height: i32,
    /// Draw the 50 % gridline (tall panels only).
    pub gridline: bool,
}

pub fn layout_for(height: i32, show_header: bool) -> GraphLayout {
    let text_scale = if height >= 64 { 2 } else { 1 };
    let header = show_header && height >= 24;
    // 128x32: header rows 0..6, plot 8..31. 256x64: header rows 0..13, plot 15..63.
    let graph_top = if header { 7 * text_scale + 1 } else { 0 };
    GraphLayout {
        text_scale,
        header,
        graph_top,
        graph_height: (height - graph_top).max(0),
        gridline: height >= 64,
    }
}

/// Slot index shown in column `x` of a `width`-pixel plot with `slots` slots.
#[inline]
pub fn slot_for_column(x: i32, width: i32, slots: usize) -> usize {
    ((x as i64 * slots as i64) / width.max(1) as i64) as usize
}

fn fill_column(
    matrix: &mut dyn MatrixBackend,
    x: i32,
    y_from: i32,
    y_to_exclusive: i32,
    c: (u8, u8, u8),
) {
    for y in y_from..y_to_exclusive {
        matrix.set_pixel(x, y, c.0, c.1, c.2);
    }
}

/// Vertical mapping of a value range onto the plot (see layout-v2.md).
#[derive(Debug, Clone, Copy)]
pub struct YScale {
    pub lo: f32,
    pub hi: f32,
    /// Plot height in rows.
    pub gh: i32,
    /// Panel height (the plot's exclusive bottom).
    pub h: i32,
}

impl YScale {
    /// Bar height in rows (counted up from the bottom) for value `v`.
    pub fn rows(&self, v: f32) -> i32 {
        let span = self.hi - self.lo;
        if span <= 0.0 || self.gh <= 0 {
            return 0;
        }
        (((v - self.lo) / span * self.gh as f32).round() as i32).clamp(0, self.gh)
    }

    /// Row of a one-pixel feature (line point, zero line, gridline) at value `v`.
    pub fn line_y(&self, v: f32) -> i32 {
        let span = self.hi - self.lo;
        let t = if span > 0.0 {
            ((v - self.lo) / span).clamp(0.0, 1.0)
        } else {
            0.0
        };
        self.h - 1 - (t * (self.gh - 1).max(0) as f32).round() as i32
    }

    /// Fills the bar rows strictly between values `a` and `b` in column `x`.
    fn fill_between(
        &self,
        matrix: &mut dyn MatrixBackend,
        x: i32,
        a: f32,
        b: f32,
        c: (u8, u8, u8),
    ) {
        let (ra, rb) = (self.rows(a), self.rows(b));
        if ra != rb {
            fill_column(matrix, x, self.h - ra.max(rb), self.h - ra.min(rb), c);
        }
    }
}

/// Integer y-axis label for `v` (rounded half away from zero, never "-0").
fn axis_label(v: f32, out: &mut String) {
    use std::fmt::Write;
    out.clear();
    let _ = write!(out, "{}", v.round() as i64);
}

/// Draws the whole screen for `graph` (or the "no data" notice). Pure drawing
/// with no I/O, so it can be unit-tested on the mock matrix.
pub fn render_graph(
    matrix: &mut dyn MatrixBackend,
    graph: Option<&GraphData>,
    show_header: bool,
    no_data_label: &str,
) {
    let mut scratch = (String::new(), String::new());
    render_graph_with(matrix, graph, show_header, no_data_label, &mut scratch);
}

/// [`render_graph`] with caller-owned buffers for the y-axis label text, so
/// the engine's render path does not allocate.
pub fn render_graph_with(
    matrix: &mut dyn MatrixBackend,
    graph: Option<&GraphData>,
    show_header: bool,
    no_data_label: &str,
    scratch: &mut (String, String),
) {
    let w = matrix.width() as i32;
    let h = matrix.height() as i32;
    matrix.clear();

    let Some(g) = graph else {
        let tw = measure_text(no_data_label);
        let x = ((w - tw) / 2).max(0);
        let y = ((h - 7) / 2).max(0);
        draw_text_scaled(matrix, no_data_label, x, y, 0, w, 0, h, 1, NO_DATA_COLOR);
        return;
    };

    let lay = layout_for(h, show_header);
    let bottom = h; // exclusive
    let gh = lay.graph_height;

    // Header: title left, summary right-aligned in the first series' colour.
    if lay.header {
        let s = lay.text_scale;
        let summary_w = if g.summary.is_empty() {
            0
        } else {
            measure_text(&g.summary) * s
        };
        let summary_x = w - summary_w;
        let title_max_x = if summary_w > 0 { summary_x - s } else { w };
        // Full title when it fits, else the short form, else the shorter one clipped.
        let room = title_max_x.max(0);
        let fits = |t: &str| measure_text(t) * s <= room;
        let title: &str = if fits(&g.title) || g.title_short.is_empty() {
            &g.title
        } else if fits(&g.title_short) || g.title_short.len() < g.title.len() {
            &g.title_short
        } else {
            &g.title
        };
        draw_text_scaled(
            matrix,
            title,
            0,
            0,
            0,
            title_max_x.max(0),
            0,
            lay.graph_top,
            s,
            TITLE_COLOR,
        );
        if summary_w > 0 {
            let color = g.series.first().map(|s| s.color).unwrap_or(TITLE_COLOR);
            draw_text_scaled(
                matrix,
                &g.summary,
                summary_x.max(0),
                0,
                0,
                w,
                0,
                lay.graph_top,
                s,
                color,
            );
        }
    }

    if gh <= 0 {
        return;
    }
    let slots = g.slots.max(1);
    let ys = YScale {
        lo: g.lo,
        hi: g.hi,
        gh,
        h,
    };

    // Optional y-axis labels (64 px tall panels only): the plot shifts right.
    let mut px0 = 0;
    if g.yaxis && h >= 64 {
        let (lmax, lmin) = (&mut scratch.0, &mut scratch.1);
        axis_label(g.hi, lmax);
        axis_label(g.lo, lmin);
        let lw = tiny_text_width(lmax).max(tiny_text_width(lmin));
        draw_tiny_text(
            matrix,
            lmax,
            lw - tiny_text_width(lmax),
            lay.graph_top,
            0,
            lw,
            AXIS_COLOR,
        );
        draw_tiny_text(
            matrix,
            lmin,
            lw - tiny_text_width(lmin),
            h - TINY_H,
            0,
            lw,
            AXIS_COLOR,
        );
        px0 = lw + 1;
    }
    let pw = w - px0;
    if pw <= 0 {
        return;
    }
    let draw_zero = g.lo < 0.0 && g.hi > 0.0 && g.has_bars();
    let zero_y = ys.line_y(0.0);
    let grid_y = ys.line_y((g.lo + g.hi) / 2.0);

    for x in px0..w {
        let slot = slot_for_column(x - px0, pw, slots);

        // 1. Bands: background columns, full plot height.
        for b in &g.bands {
            if slot >= b.from && slot < b.to {
                fill_column(matrix, x, lay.graph_top, bottom, b.color);
            }
        }

        // 2. Zero line when the range crosses 0 and bars are drawn.
        if draw_zero {
            matrix.set_pixel(x, zero_y, ZERO_COLOR.0, ZERO_COLOR.1, ZERO_COLOR.2);
        }

        // 3. Mid-range gridline on tall panels (behind the data).
        if lay.gridline {
            matrix.set_pixel(x, grid_y, GRID_COLOR.0, GRID_COLOR.1, GRID_COLOR.2);
        }

        // 4. Marks: dotted vertical line at the left edge of slot `at`.
        let left_edge = x == px0 || slot_for_column(x - 1 - px0, pw, slots) != slot;
        if left_edge {
            for m in &g.marks {
                if m.at == slot {
                    let mut y = lay.graph_top;
                    while y < bottom {
                        matrix.set_pixel(x, y, m.color.0, m.color.1, m.color.2);
                        y += 2;
                    }
                }
            }
        }

        // 5. Bars, from the zero line toward the value (missing = 0).
        let (mut cp, mut cn) = (0.0f32, 0.0f32);
        for s in g.series.iter().filter(|s| !s.line) {
            let v = g.value_at(s, slot);
            if g.stack {
                if v > 0.0 {
                    ys.fill_between(matrix, x, cp, cp + v, s.color);
                    cp += v;
                } else if v < 0.0 {
                    ys.fill_between(matrix, x, cn, cn + v, s.color);
                    cn += v;
                }
            } else {
                ys.fill_between(matrix, x, 0.0, v, s.color);
            }
        }
    }

    // 6. Lines on top, in series order; null / missing points leave a gap.
    for s in g.series.iter().filter(|s| s.line) {
        let c = s.color;
        let mut prev_y: Option<i32> = None;
        for x in px0..w {
            let slot = slot_for_column(x - px0, pw, slots);
            let Some(v) = g.value_opt(s, slot) else {
                prev_y = None;
                continue;
            };
            let y = ys.line_y(v);
            let (y0, y1) = match prev_y {
                Some(p) => (p.min(y), p.max(y)),
                None => (y, y),
            };
            fill_column(matrix, x, y0, y1 + 1, c);
            prev_y = Some(y);
        }
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

pub struct GraphEngine {
    topic: String,
    show_header: bool,
    graph: Option<Arc<GraphData>>,
    seen_generation: u64,
    lang: Lang,
    /// Reused y-axis label buffers (no allocation while rendering).
    axis_scratch: (String, String),
}

impl Default for GraphEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl GraphEngine {
    pub fn new() -> Self {
        Self {
            topic: DEFAULT_TOPIC.to_string(),
            show_header: true,
            graph: None,
            // Force a lookup on the first update.
            seen_generation: u64::MAX,
            lang: Lang::En,
            axis_scratch: (String::with_capacity(16), String::with_capacity(16)),
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
        self.show_header = config.get_bool("show_header", true);
        self.seen_generation = u64::MAX;
    }

    fn refresh_data(&mut self) {
        let store = data_feed();
        let generation = store.generation();
        if generation == self.seen_generation {
            return;
        }
        self.seen_generation = generation;
        self.graph = store.get(&self.topic).and_then(|e| e.graph.clone());
    }

    /// The graph currently shown (for tests and diagnostics).
    pub fn current(&self) -> Option<&GraphData> {
        self.graph.as_deref()
    }
}

impl Engine for GraphEngine {
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
        render_graph_with(
            ctx.matrix,
            self.graph.as_deref(),
            self.show_header,
            i18n::no_data_label(self.lang),
            &mut self.axis_scratch,
        );
    }

    fn deactivate(&mut self) {}

    fn on_config_changed(&mut self, config: &dyn EngineConfig) {
        self.read_config(config);
        self.refresh_data();
    }
}

#[distributed_slice(crate::core::registry::ENGINES)]
fn register_graph_engine() -> EngineDescriptor {
    EngineDescriptor {
        metadata: EngineMetadata {
            id: "graph",
            name: "Graph",
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
                    description: "Full topic of the graph payload on the Data Feed MQTT broker",
                    default_value: DEFAULT_TOPIC,
                    validation_policy: ValidationPolicy::Accept,
                    ..Default::default()
                },
                ConfigField {
                    id: "show_header",
                    field_type: ConfigType::Boolean,
                    label: "Show Header",
                    description: "Show the title and summary line above the graph",
                    default_value: "true",
                    validation_policy: ValidationPolicy::FallbackDefault,
                    ..Default::default()
                },
            ],
        },
        factory: || Box::new(GraphEngine::new()),
    }
}
