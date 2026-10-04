//! MQTT Data payloads: parsed once when a message arrives (on the MQTT
//! thread), so the render path never touches JSON. The contract is
//! docs/MQTT_DATA_CONTRACT.md.

use serde_json::Value;

/// Maximum accepted payload size in bytes; larger payloads are dropped.
pub const MAX_PAYLOAD_BYTES: usize = 8192;
/// Maximum number of topics per MQTT Data instance.
pub const MAX_TOPICS: usize = 6;
/// Maximum number of series in one graph payload (extra series are ignored).
pub const MAX_SERIES: usize = 4;
/// Maximum number of points per series (extra points are ignored).
pub const MAX_POINTS: usize = 288;
/// Maximum number of background bands in one graph payload. When a payload
/// sends more, the newest (highest `from`) are kept.
pub const MAX_BANDS: usize = 96;
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

/// One legend entry (drawn only on 64 px tall panels).
#[derive(Debug, Clone, PartialEq)]
pub struct GraphLegendItem {
    pub text: String,
    pub color: (u8, u8, u8),
}

/// Maximum number of legend items read from a graph payload.
pub const MAX_LEGEND: usize = 4;

#[derive(Debug, Clone, PartialEq)]
pub struct GraphMark {
    pub at: usize,
    pub color: (u8, u8, u8),
}

/// A parsed graph payload (spec v1 + round 2 additions), ready to draw without
/// further parsing.
#[derive(Debug, Clone, PartialEq)]
pub struct GraphData {
    /// Payload `show_header` (default true).
    pub show_header: bool,
    pub title: String,
    /// Optional abbreviation, used only when `title` doesn't fit the header.
    pub title_short: String,
    pub summary: String,
    /// Optional colour of the header summary (else the first series' colour).
    pub summary_color: Option<(u8, u8, u8)>,
    /// Optional legend row (64 px tall panels only).
    pub legend: Vec<GraphLegendItem>,
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

pub(crate) fn parse_graph_value(root: &Value) -> Option<GraphData> {
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
    if bands.len() > MAX_BANDS {
        // Keep the newest bands (highest `from`), drawn oldest first.
        bands.sort_by_key(|b| b.from);
        bands.drain(..bands.len() - MAX_BANDS);
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
        show_header: root
            .get("show_header")
            .and_then(|v| v.as_bool())
            .unwrap_or(true),
        title: short_text(root.get("title")),
        title_short: short_text(root.get("title_short")),
        summary: short_text(root.get("summary")),
        summary_color: root
            .get("summary_color")
            .and_then(|c| c.as_str())
            .and_then(parse_hex_color),
        legend: root
            .get("legend")
            .and_then(|l| l.as_array())
            .map(|arr| {
                arr.iter()
                    .take(MAX_LEGEND)
                    .filter_map(|it| {
                        let text = short_text(it.get("text"));
                        let color = it
                            .get("color")
                            .and_then(|c| c.as_str())
                            .and_then(parse_hex_color)?;
                        (!text.is_empty()).then_some(GraphLegendItem { text, color })
                    })
                    .collect()
            })
            .unwrap_or_default(),
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

/// The y range to draw (docs/MQTT_DATA_CONTRACT.md, `min` / `max`).
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
pub struct TableData {
    pub title: String,
    /// Payload `show_title` (default true).
    pub show_title: bool,
    pub tiles: Vec<ValueTile>,
}

pub fn parse_table_payload(json: &str) -> Option<TableData> {
    let root: Value = serde_json::from_str(json).ok()?;
    parse_table_value(&root)
}

pub(crate) fn parse_table_value(root: &Value) -> Option<TableData> {
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
    Some(TableData {
        title: short_text(root.get("title")),
        show_title: root
            .get("show_title")
            .and_then(|v| v.as_bool())
            .unwrap_or(true),
        tiles,
    })
}

// ---------------------------------------------------------------------------
// Weather payload (`"current"` / `"days"`)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Default)]
pub struct WeatherDay {
    pub temp_max: Option<f32>,
    pub temp_min: Option<f32>,
    pub condition: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WeatherPayload {
    /// true = °F labels, false = °C. The sign never converts numbers.
    pub imperial: bool,
    pub current_temp: Option<f32>,
    pub current_condition: String,
    pub humidity: Option<f32>,
    pub wind: Option<f32>,
    pub wind_unit: String,
    /// 8-point compass direction ("NE"), empty when not sent.
    pub wind_dir: String,
    pub days: Vec<WeatherDay>,
}

pub fn parse_weather_payload(json: &str) -> Option<WeatherPayload> {
    let root: Value = serde_json::from_str(json).ok()?;
    parse_weather_value(&root)
}

pub(crate) fn parse_weather_value(root: &Value) -> Option<WeatherPayload> {
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
                .map(|d| WeatherDay {
                    temp_max: finite(d.get("temp_max")),
                    temp_min: finite(d.get("temp_min")),
                    condition: short_text(d.get("condition")),
                })
                .collect()
        })
        .unwrap_or_default();
    Some(WeatherPayload {
        imperial,
        current_temp: current.and_then(|c| finite(c.get("temp"))),
        current_condition: current
            .map(|c| short_text(c.get("condition")))
            .unwrap_or_default(),
        humidity: current.and_then(|c| finite(c.get("humidity"))),
        wind: current.and_then(|c| finite(c.get("wind"))),
        wind_dir: current
            .map(|c| short_text(c.get("wind_dir")).trim().to_string())
            .unwrap_or_default(),
        wind_unit: current
            .map(|c| short_text(c.get("wind_unit")))
            .unwrap_or_default(),
        days,
    })
}

// ---------------------------------------------------------------------------
// Value payload (one big value)
// ---------------------------------------------------------------------------

/// `{"type":"value","value":"21.7","unit":"°C","label":"OUTSIDE","color":"#RRGGBB"}`.
/// Drawn exactly like a one-tile table without a title.
pub(crate) fn parse_value_value(root: &Value) -> Option<TableData> {
    let value = match root.get("value")? {
        Value::String(s) => s.chars().take(MAX_TEXT_CHARS).collect(),
        Value::Number(n) => n.to_string(),
        _ => return None,
    };
    let color = |v: Option<&Value>, d: (u8, u8, u8)| {
        v.and_then(|c| c.as_str())
            .and_then(parse_hex_color)
            .unwrap_or(d)
    };
    Some(TableData {
        title: String::new(),
        show_title: false,
        tiles: vec![ValueTile {
            label: short_text(root.get("label")),
            label_short: short_text(root.get("label_short")),
            value,
            unit: short_text(root.get("unit")),
            color: color(root.get("color"), (255, 255, 255)),
            label_color: color(root.get("label_color"), (0x80, 0x80, 0x80)),
        }],
    })
}

/// Default and minimum time on one page, in seconds.
pub const DEFAULT_PAGE_SECONDS: u32 = 10;
pub const MIN_PAGE_SECONDS: u32 = 3;
/// Upper bound so one bad payload can't park a page for hours.
pub const MAX_PAGE_SECONDS: u32 = 3600;
/// Highest contract version this firmware knows. Newer payloads still render
/// the fields it understands (unknown fields are ignored).
pub const CONTRACT_VERSION: u64 = 1;

/// What one topic shows.
#[derive(Debug, Clone, PartialEq)]
pub enum PageData {
    /// `value` and `table` share the table renderer.
    Table(TableData),
    Graph(GraphData),
    Weather(WeatherPayload),
    /// `type` missing or unknown, required fields missing, or not JSON.
    Unsupported,
}

/// A parsed payload plus its time on screen.
#[derive(Debug, Clone, PartialEq)]
pub struct Page {
    pub data: PageData,
    /// Seconds per page (weather: per NOW / day page).
    pub seconds: u32,
}

/// Parses a payload against the MQTT Data contract (docs/MQTT_DATA_CONTRACT.md).
/// `type` is required: `value`, `table`, `graph` or `weather`. Returns `None`
/// for an empty payload (a cleared retained message: the page shows NO DATA);
/// anything else that can't be drawn is `PageData::Unsupported`.
pub fn parse_page(json: &str) -> Option<Page> {
    if json.trim().is_empty() {
        return None;
    }
    let Ok(root) = serde_json::from_str::<Value>(json) else {
        return Some(Page {
            data: PageData::Unsupported,
            seconds: DEFAULT_PAGE_SECONDS,
        });
    };
    let seconds = root
        .get("seconds")
        .and_then(|v| v.as_f64())
        .filter(|f| f.is_finite())
        .map(|f| (f.round().max(0.0) as u32).clamp(MIN_PAGE_SECONDS, MAX_PAGE_SECONDS))
        .unwrap_or(DEFAULT_PAGE_SECONDS);
    let kind = root
        .get("type")
        .and_then(|t| t.as_str())
        .map(|t| t.trim().to_ascii_lowercase());
    let data = match kind.as_deref() {
        Some("value") => parse_value_value(&root).map(PageData::Table),
        Some("table") => parse_table_value(&root).map(PageData::Table),
        Some("graph") => parse_graph_value(&root).map(PageData::Graph),
        Some("weather") => parse_weather_value(&root).map(PageData::Weather),
        _ => None,
    }
    .unwrap_or(PageData::Unsupported);
    Some(Page { data, seconds })
}
