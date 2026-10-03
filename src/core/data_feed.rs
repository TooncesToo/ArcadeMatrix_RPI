//! Data feed store: the last payload received on each `data_mqtt` topic.
//!
//! The data MQTT client (control plane) writes here; engines such as `graph`
//! read from here on their own cadence. Each topic lives in one of a fixed
//! number of lock-free slots (`ArcSwapOption`), and a global generation counter
//! lets an engine skip the lookup entirely while nothing has changed.
//!
//! Payloads are parsed once on arrival, by shape: a `"series"` array becomes a
//! [`GraphData`], a `"tiles"` array a [`ValuesData`], and `"current"`/`"days"`
//! a [`HaWeather`]. The render path never touches JSON.

use arc_swap::ArcSwapOption;
use serde_json::Value;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Instant;

/// Maximum number of distinct topics kept.
pub const MAX_TOPICS: usize = 8;
/// Maximum accepted payload size in bytes; larger payloads are dropped.
pub const MAX_PAYLOAD_BYTES: usize = 8192;
/// Maximum number of series in one graph payload (extra series are ignored).
pub const MAX_SERIES: usize = 4;
/// Maximum number of points per series (extra points are ignored).
pub const MAX_POINTS: usize = 288;
/// Maximum number of tiles in one values payload (extra tiles are ignored).
pub const MAX_TILES: usize = 4;
/// Maximum number of forecast days kept from a weather payload.
pub const MAX_WEATHER_DAYS: usize = 5;
/// Maximum characters kept for title / summary / unit / labels.
const MAX_TEXT_CHARS: usize = 32;

/// Colours used when a series has no (valid) colour of its own.
const DEFAULT_SERIES_COLORS: [(u8, u8, u8); MAX_SERIES] = [
    (0x20, 0x80, 0xFF),
    (0x00, 0xD0, 0xA0),
    (0xFF, 0xA0, 0x00),
    (0xFF, 0x40, 0x80),
];

#[derive(Debug, Clone, PartialEq)]
pub struct GraphSeries {
    pub label: String,
    pub color: (u8, u8, u8),
    /// `"style": "line"` (1 px step line); otherwise a bar series.
    pub line: bool,
    /// Points; `NaN` marks a JSON null / non-number (a gap in a line, 0 in a bar).
    pub data: Vec<f32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GraphBand {
    pub from: usize,
    pub to: usize,
    pub color: (u8, u8, u8),
}

#[derive(Debug, Clone, PartialEq)]
pub struct GraphMark {
    pub at: usize,
    pub color: (u8, u8, u8),
}

/// A parsed graph payload (spec v1 + round 2 additions), ready to draw without
/// further parsing.
#[derive(Debug, Clone, PartialEq)]
pub struct GraphData {
    pub title: String,
    /// Optional abbreviation, used only when `title` doesn't fit the header.
    pub title_short: String,
    pub summary: String,
    pub unit: String,
    /// Number of x positions; data is right-aligned into these.
    pub slots: usize,
    /// Explicit `min` / `max` from the payload, if any.
    pub min: Option<f32>,
    pub max: Option<f32>,
    /// The y range actually drawn (`lo < hi`).
    pub lo: f32,
    pub hi: f32,
    pub stack: bool,
    /// Print min/max at the left edge (64 px tall panels only).
    pub yaxis: bool,
    pub series: Vec<GraphSeries>,
    pub bands: Vec<GraphBand>,
    pub marks: Vec<GraphMark>,
}

impl GraphData {
    /// Value of `series` at `slot`, honouring right alignment. `None` for
    /// slots the series does not reach and for null points.
    pub fn value_opt(&self, series: &GraphSeries, slot: usize) -> Option<f32> {
        let n = series.data.len();
        if n == 0 || slot >= self.slots {
            return None;
        }
        // Right-aligned: the last point sits in the last slot.
        let start = self.slots as isize - n as isize;
        let idx = slot as isize - start;
        if idx < 0 || idx as usize >= n {
            return None;
        }
        let v = series.data[idx as usize];
        if v.is_nan() {
            None
        } else {
            Some(v)
        }
    }

    /// Like [`value_opt`](Self::value_opt) but missing points count as 0 (bars).
    pub fn value_at(&self, series: &GraphSeries, slot: usize) -> f32 {
        self.value_opt(series, slot).unwrap_or(0.0)
    }

    pub fn has_bars(&self) -> bool {
        self.series.iter().any(|s| !s.line)
    }

    pub fn has_lines(&self) -> bool {
        self.series.iter().any(|s| s.line)
    }
}

/// Parses `"#RRGGBB"` (the leading `#` is optional).
pub fn parse_hex_color(s: &str) -> Option<(u8, u8, u8)> {
    let hex = s.trim().trim_start_matches('#');
    if hex.len() != 6 || !hex.is_ascii() {
        return None;
    }
    let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
    Some((r, g, b))
}

fn short_text(v: Option<&Value>) -> String {
    v.and_then(|v| v.as_str())
        .unwrap_or("")
        .chars()
        .take(MAX_TEXT_CHARS)
        .collect()
}

fn finite(v: Option<&Value>) -> Option<f32> {
    v.and_then(|v| v.as_f64())
        .filter(|f| f.is_finite())
        .map(|f| f as f32)
}

fn as_index(v: Option<&Value>) -> Option<usize> {
    let v = v?;
    if let Some(u) = v.as_u64() {
        return Some(u as usize);
    }
    // Accept 12.0 style numbers too; reject negatives and non-numbers.
    v.as_f64()
        .filter(|f| f.is_finite() && *f >= 0.0)
        .map(|f| f as usize)
}

/// Parses a graph payload. Returns `None` when the JSON is invalid or has no
/// `series` array.
pub fn parse_graph_payload(json: &str) -> Option<GraphData> {
    let root: Value = serde_json::from_str(json).ok()?;
    parse_graph_value(&root)
}

fn parse_graph_value(root: &Value) -> Option<GraphData> {
    let series_val = root.get("series")?.as_array()?;

    let mut series = Vec::with_capacity(series_val.len().min(MAX_SERIES));
    for (i, s) in series_val.iter().take(MAX_SERIES).enumerate() {
        let color = s
            .get("color")
            .and_then(|c| c.as_str())
            .and_then(parse_hex_color)
            .unwrap_or(DEFAULT_SERIES_COLORS[i]);
        let line = s
            .get("style")
            .and_then(|v| v.as_str())
            .map(|v| v.trim().eq_ignore_ascii_case("line"))
            .unwrap_or(false);
        let data = s
            .get("data")
            .and_then(|d| d.as_array())
            .map(|arr| {
                arr.iter()
                    .take(MAX_POINTS)
                    .map(|p| {
                        p.as_f64()
                            .filter(|f| f.is_finite())
                            .map(|f| f as f32)
                            .unwrap_or(f32::NAN)
                    })
                    .collect::<Vec<f32>>()
            })
            .unwrap_or_default();
        series.push(GraphSeries {
            label: short_text(s.get("label")),
            color,
            line,
            data,
        });
    }

    let longest = series.iter().map(|s| s.data.len()).max().unwrap_or(0);
    let slots = as_index(root.get("slots"))
        .filter(|&n| n > 0)
        .map(|n| n.min(MAX_POINTS))
        .unwrap_or(longest)
        .max(1);

    let min = finite(root.get("min"));
    let max = finite(root.get("max"));
    let stack = root.get("stack").and_then(|s| s.as_bool()).unwrap_or(true);
    let yaxis = root.get("yaxis").and_then(|s| s.as_bool()).unwrap_or(false);

    let mut bands = Vec::new();
    if let Some(arr) = root.get("bands").and_then(|b| b.as_array()) {
        for b in arr {
            let (Some(from), Some(to)) = (as_index(b.get("from")), as_index(b.get("to"))) else {
                continue;
            };
            let color = b
                .get("color")
                .and_then(|c| c.as_str())
                .and_then(parse_hex_color);
            if let Some(color) = color {
                if to > from {
                    bands.push(GraphBand { from, to, color });
                }
            }
        }
    }

    let mut marks = Vec::new();
    if let Some(arr) = root.get("marks").and_then(|m| m.as_array()) {
        for m in arr {
            let Some(at) = as_index(m.get("at")) else {
                continue;
            };
            if let Some(color) = m
                .get("color")
                .and_then(|c| c.as_str())
                .and_then(parse_hex_color)
            {
                marks.push(GraphMark { at, color });
            }
        }
    }

    let mut graph = GraphData {
        title: short_text(root.get("title")),
        title_short: short_text(root.get("title_short")),
        summary: short_text(root.get("summary")),
        unit: short_text(root.get("unit")),
        slots,
        min,
        max,
        lo: 0.0,
        hi: 1.0,
        stack,
        yaxis,
        series,
        bands,
        marks,
    };
    let (lo, hi) = y_range(&graph);
    graph.lo = lo;
    graph.hi = hi;
    Some(graph)
}

/// Lowest / highest value the bars reach over all slots: stacked sums
/// (positive and negative separately) or single values when not stacked.
fn bar_extent(g: &GraphData) -> Option<(f32, f32)> {
    if !g.has_bars() {
        return None;
    }
    let (mut lo, mut hi) = (0.0f32, 0.0f32);
    for slot in 0..g.slots {
        let (mut cp, mut cn) = (0.0f32, 0.0f32);
        for s in g.series.iter().filter(|s| !s.line) {
            let v = g.value_at(s, slot);
            if g.stack {
                if v > 0.0 {
                    cp += v;
                } else {
                    cn += v;
                }
            } else {
                hi = hi.max(v);
                lo = lo.min(v);
            }
        }
        hi = hi.max(cp);
        lo = lo.min(cn);
    }
    Some((lo, hi))
}

/// The y range to draw (see layout-v2.md "Y range").
fn y_range(g: &GraphData) -> (f32, f32) {
    let bars = bar_extent(g);
    let mut dmin = f32::INFINITY;
    let mut dmax = f32::NEG_INFINITY;
    for s in g.series.iter().filter(|s| s.line) {
        for slot in 0..g.slots {
            if let Some(v) = g.value_opt(s, slot) {
                dmin = dmin.min(v);
                dmax = dmax.max(v);
            }
        }
    }
    let has_line_data = dmin.is_finite();

    let (auto_lo, auto_hi) = if has_line_data {
        if let Some((blo, bhi)) = bars {
            dmin = dmin.min(blo);
            dmax = dmax.max(bhi);
        }
        let pad = (0.05 * (dmax - dmin)).max(0.5);
        (dmin - pad, dmax + pad)
    } else if let Some((blo, bhi)) = bars {
        (blo.min(0.0), bhi)
    } else {
        (0.0, 1.0)
    };

    let lo = g.min.unwrap_or(auto_lo);
    let hi = match g.max {
        Some(m) if m > lo => m,
        _ => auto_hi.max(lo + 0.001),
    };
    (lo, hi)
}

// ---------------------------------------------------------------------------
// Values payload (`"tiles"`)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct ValueTile {
    pub label: String,
    /// Optional abbreviation, used only when `label` doesn't fit its cell.
    pub label_short: String,
    pub value: String,
    pub unit: String,
    pub color: (u8, u8, u8),
    pub label_color: (u8, u8, u8),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ValuesData {
    pub title: String,
    pub tiles: Vec<ValueTile>,
}

pub fn parse_values_payload(json: &str) -> Option<ValuesData> {
    let root: Value = serde_json::from_str(json).ok()?;
    parse_values_value(&root)
}

fn parse_values_value(root: &Value) -> Option<ValuesData> {
    let arr = root.get("tiles")?.as_array()?;
    let color = |v: Option<&Value>, d: (u8, u8, u8)| {
        v.and_then(|c| c.as_str())
            .and_then(parse_hex_color)
            .unwrap_or(d)
    };
    let tiles = arr
        .iter()
        .take(MAX_TILES)
        .map(|t| {
            // Accept numbers too, but HA is expected to send preformatted strings.
            let value = match t.get("value") {
                Some(Value::String(s)) => s.chars().take(MAX_TEXT_CHARS).collect(),
                Some(Value::Number(n)) => n.to_string(),
                _ => String::new(),
            };
            ValueTile {
                label: short_text(t.get("label")),
                label_short: short_text(t.get("label_short")),
                value,
                unit: short_text(t.get("unit")),
                color: color(t.get("color"), (255, 255, 255)),
                label_color: color(t.get("label_color"), (0x80, 0x80, 0x80)),
            }
        })
        .collect();
    Some(ValuesData {
        title: short_text(root.get("title")),
        tiles,
    })
}

// ---------------------------------------------------------------------------
// Weather payload (`"current"` / `"days"`), published by Home Assistant
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Default)]
pub struct HaWeatherDay {
    pub temp_max: Option<f32>,
    pub temp_min: Option<f32>,
    pub condition: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HaWeather {
    /// true = °F labels, false = °C. The sign never converts numbers.
    pub imperial: bool,
    pub current_temp: Option<f32>,
    pub current_condition: String,
    pub humidity: Option<f32>,
    pub wind: Option<f32>,
    pub wind_unit: String,
    pub days: Vec<HaWeatherDay>,
}

pub fn parse_weather_payload(json: &str) -> Option<HaWeather> {
    let root: Value = serde_json::from_str(json).ok()?;
    parse_weather_value(&root)
}

fn parse_weather_value(root: &Value) -> Option<HaWeather> {
    let current = root.get("current");
    let days_val = root.get("days").and_then(|d| d.as_array());
    if current.is_none() && days_val.is_none() {
        return None;
    }
    let imperial = !root
        .get("units")
        .and_then(|u| u.as_str())
        .map(|u| u.trim().eq_ignore_ascii_case("metric"))
        .unwrap_or(false);
    let days = days_val
        .map(|arr| {
            arr.iter()
                .take(MAX_WEATHER_DAYS)
                .map(|d| HaWeatherDay {
                    temp_max: finite(d.get("temp_max")),
                    temp_min: finite(d.get("temp_min")),
                    condition: short_text(d.get("condition")),
                })
                .collect()
        })
        .unwrap_or_default();
    Some(HaWeather {
        imperial,
        current_temp: current.and_then(|c| finite(c.get("temp"))),
        current_condition: current
            .map(|c| short_text(c.get("condition")))
            .unwrap_or_default(),
        humidity: current.and_then(|c| finite(c.get("humidity"))),
        wind: current.and_then(|c| finite(c.get("wind"))),
        wind_unit: current
            .map(|c| short_text(c.get("wind_unit")))
            .unwrap_or_default(),
        days,
    })
}

/// One stored topic.
#[derive(Debug)]
pub struct FeedEntry {
    pub topic: String,
    pub payload: String,
    pub graph: Option<Arc<GraphData>>,
    pub values: Option<Arc<ValuesData>>,
    pub weather: Option<Arc<HaWeather>>,
    pub received: Instant,
}

/// Outcome of [`DataFeedStore::publish`].
#[derive(Debug, PartialEq, Eq)]
pub enum PublishResult {
    Stored,
    TooLarge,
    Full,
}

pub struct DataFeedStore {
    slots: [ArcSwapOption<FeedEntry>; MAX_TOPICS],
    generation: AtomicU64,
    connected: AtomicBool,
    topic_count: AtomicUsize,
}

impl Default for DataFeedStore {
    fn default() -> Self {
        Self::new()
    }
}

impl DataFeedStore {
    pub fn new() -> Self {
        Self {
            slots: std::array::from_fn(|_| ArcSwapOption::empty()),
            generation: AtomicU64::new(0),
            connected: AtomicBool::new(false),
            topic_count: AtomicUsize::new(0),
        }
    }

    /// Stores `payload` as the latest value for `topic`. Called from the MQTT
    /// client thread only (single writer).
    pub fn publish(&self, topic: &str, payload: &str) -> PublishResult {
        if payload.len() > MAX_PAYLOAD_BYTES {
            return PublishResult::TooLarge;
        }
        let mut target = None;
        let mut first_free = None;
        for (i, slot) in self.slots.iter().enumerate() {
            match slot.load().as_ref() {
                Some(e) if e.topic == topic => {
                    target = Some(i);
                    break;
                }
                None if first_free.is_none() => first_free = Some(i),
                _ => {}
            }
        }
        let is_new = target.is_none();
        let Some(idx) = target.or(first_free) else {
            return PublishResult::Full;
        };
        // Parse once here, by shape, so engines never touch JSON.
        let root: Option<Value> = serde_json::from_str(payload).ok();
        let entry = FeedEntry {
            topic: topic.to_string(),
            payload: payload.to_string(),
            graph: root.as_ref().and_then(parse_graph_value).map(Arc::new),
            values: root.as_ref().and_then(parse_values_value).map(Arc::new),
            weather: root.as_ref().and_then(parse_weather_value).map(Arc::new),
            received: Instant::now(),
        };
        self.slots[idx].store(Some(Arc::new(entry)));
        if is_new {
            self.topic_count.fetch_add(1, Ordering::Relaxed);
        }
        self.generation.fetch_add(1, Ordering::Release);
        PublishResult::Stored
    }

    /// Latest entry for `topic`, if any. Lock-free and allocation-free.
    pub fn get(&self, topic: &str) -> Option<Arc<FeedEntry>> {
        for slot in &self.slots {
            if let Some(e) = slot.load_full() {
                if e.topic == topic {
                    return Some(e);
                }
            }
        }
        None
    }

    /// Bumped on every stored payload; lets readers skip unchanged frames.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    pub fn set_connected(&self, connected: bool) {
        self.connected.store(connected, Ordering::Relaxed);
    }

    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }

    pub fn topic_count(&self) -> usize {
        self.topic_count.load(Ordering::Relaxed)
    }
}

/// The process-wide store shared by the data MQTT client and the engines.
pub fn data_feed() -> &'static DataFeedStore {
    static STORE: OnceLock<DataFeedStore> = OnceLock::new();
    STORE.get_or_init(DataFeedStore::new)
}
