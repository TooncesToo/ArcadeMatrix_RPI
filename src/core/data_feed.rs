//! Data feed store: the last payload received on each `data_mqtt` topic.
//!
//! The data MQTT client (control plane) writes here; engines such as `graph`
//! read from here on their own cadence. Each topic lives in one of a fixed
//! number of lock-free slots (`ArcSwapOption`), and a global generation counter
//! lets an engine skip the lookup entirely while nothing has changed.
//!
//! Payloads that look like a graph (`"series"` array) are parsed once on
//! arrival into a [`GraphData`], so the render path never touches JSON.

use arc_swap::ArcSwapOption;
use serde_json::Value;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Instant;

/// Maximum number of distinct topics kept.
pub const MAX_TOPICS: usize = 8;
/// Maximum accepted payload size in bytes; larger payloads are dropped.
pub const MAX_PAYLOAD_BYTES: usize = 4096;
/// Maximum number of series in one graph payload (extra series are ignored).
pub const MAX_SERIES: usize = 4;
/// Maximum number of points per series (extra points are ignored).
pub const MAX_POINTS: usize = 288;
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

/// A parsed graph payload (spec v1), ready to draw without further parsing.
#[derive(Debug, Clone, PartialEq)]
pub struct GraphData {
    pub title: String,
    pub summary: String,
    pub unit: String,
    /// Number of x positions; data is right-aligned into these.
    pub slots: usize,
    /// Explicit y scale top from the payload (0 = auto).
    pub max: f32,
    /// The scale actually used: `max` when > 0, else the auto value.
    pub effective_max: f32,
    pub stack: bool,
    pub series: Vec<GraphSeries>,
    pub bands: Vec<GraphBand>,
    pub marks: Vec<GraphMark>,
}

impl GraphData {
    /// Value of `series` at slot index `slot`, honouring right alignment.
    /// Returns 0 for slots the series does not reach.
    pub fn value_at(&self, series: &GraphSeries, slot: usize) -> f32 {
        let n = series.data.len();
        if n == 0 || slot >= self.slots {
            return 0.0;
        }
        // Right-aligned: the last point sits in the last slot.
        let start = self.slots as isize - n as isize;
        let idx = slot as isize - start;
        if idx < 0 || idx as usize >= n {
            0.0
        } else {
            series.data[idx as usize]
        }
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

/// Parses a graph payload (spec v1). Returns `None` when the JSON is invalid or
/// has no `series` array.
pub fn parse_graph_payload(json: &str) -> Option<GraphData> {
    let root: Value = serde_json::from_str(json).ok()?;
    let series_val = root.get("series")?.as_array()?;

    let mut series = Vec::with_capacity(series_val.len().min(MAX_SERIES));
    for (i, s) in series_val.iter().take(MAX_SERIES).enumerate() {
        let color = s
            .get("color")
            .and_then(|c| c.as_str())
            .and_then(parse_hex_color)
            .unwrap_or(DEFAULT_SERIES_COLORS[i]);
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
                            .unwrap_or(0.0)
                    })
                    .collect::<Vec<f32>>()
            })
            .unwrap_or_default();
        series.push(GraphSeries {
            label: short_text(s.get("label")),
            color,
            data,
        });
    }

    let longest = series.iter().map(|s| s.data.len()).max().unwrap_or(0);
    let slots = as_index(root.get("slots"))
        .filter(|&n| n > 0)
        .map(|n| n.min(MAX_POINTS))
        .unwrap_or(longest)
        .max(1);

    let max = root
        .get("max")
        .and_then(|m| m.as_f64())
        .filter(|f| f.is_finite())
        .unwrap_or(0.0) as f32;
    let stack = root.get("stack").and_then(|s| s.as_bool()).unwrap_or(true);

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
        summary: short_text(root.get("summary")),
        unit: short_text(root.get("unit")),
        slots,
        max,
        effective_max: 0.0,
        stack,
        series,
        bands,
        marks,
    };
    graph.effective_max = if max > 0.0 { max } else { auto_max(&graph) };
    Some(graph)
}

/// Auto scale: the highest stacked sum (stack) or highest single value
/// (overlay) over all slots, never below 0.001.
fn auto_max(g: &GraphData) -> f32 {
    let mut top = 0.0f32;
    for slot in 0..g.slots {
        if g.stack {
            let sum: f32 = g.series.iter().map(|s| g.value_at(s, slot).max(0.0)).sum();
            top = top.max(sum);
        } else {
            for s in &g.series {
                top = top.max(g.value_at(s, slot));
            }
        }
    }
    top.max(0.001)
}

/// One stored topic.
#[derive(Debug)]
pub struct FeedEntry {
    pub topic: String,
    pub payload: String,
    pub graph: Option<Arc<GraphData>>,
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
        let entry = FeedEntry {
            topic: topic.to_string(),
            payload: payload.to_string(),
            graph: parse_graph_payload(payload).map(Arc::new),
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
