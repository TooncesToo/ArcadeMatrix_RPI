//! Home Assistant engine: shows pages published by Home Assistant over MQTT.
//!
//! Each instance lists up to 6 topics; each topic is one page (a weather topic
//! expands to its NOW page plus one page per forecast day). Pages cycle every
//! `page_seconds` while the slot is on screen.
//!
//! Nothing runs in the background. The engine connects when its slot becomes
//! active (credentials from the global `data_mqtt` settings), subscribes to its
//! topics, receives the retained snapshot, stays subscribed for live updates
//! while on screen, and on deactivate stops its thread, disconnects and frees
//! everything (Discussion #59). Payloads are parsed on the MQTT thread; the
//! render path only reads the parsed pages.

pub mod graph;
pub mod payload;
pub mod values;
pub mod weather;

use crate::api::DayForecast;
use crate::core::config::DataMqttConfig;
use crate::core::engine_contract::{
    Capabilities, ConfigField, ConfigSchema, ConfigType, Engine, EngineConfig, EngineContext,
    EngineDescriptor, EngineError, EngineMetadata, Requirements, ValidationPolicy,
};
use crate::core::i18n::{self, Lang};
use crate::core::matrix::MatrixBackend;
use crate::engines::dashboard::font::{draw_text_scaled, measure_text};
use crate::engines::renderers::weather_page::draw_weather_page;
use arc_swap::ArcSwapOption;
use linkme::distributed_slice;
use payload::{parse_page, PageData, MAX_PAYLOAD_BYTES, MAX_TOPICS};
use rumqttc::{Client, Event, MqttOptions, Packet, QoS};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tracing::{info, warn};

/// Default page time in seconds, and its minimum.
pub const DEFAULT_PAGE_SECONDS: i32 = 10;
pub const MIN_PAGE_SECONDS: i32 = 3;
/// A page whose topic sent nothing this long after connecting shows NO DATA.
pub const NO_DATA_AFTER: Duration = Duration::from_secs(3);
/// How long the MQTT thread waits per poll before checking its stop flag.
const POLL_SLICE: Duration = Duration::from_millis(150);

const NOTICE_COLOR: (u8, u8, u8) = (0x78, 0x78, 0x78);
const NO_CONNECTION_COLOR: (u8, u8, u8) = (0xB4, 0x50, 0x50);

/// Connection state shared with the MQTT thread.
pub const STATE_CONNECTING: u8 = 0;
pub const STATE_CONNECTED: u8 = 1;
pub const STATE_FAILED: u8 = 2;

/// Splits the `topics` setting: comma, semicolon or newline separated, blanks
/// dropped, at most [`MAX_TOPICS`].
pub fn parse_topics(raw: &str) -> Vec<String> {
    raw.split([',', ';', '\n'])
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .take(MAX_TOPICS)
        .map(String::from)
        .collect()
}

/// MQTT client id for this sign's data connection (`<hostname>-data`).
pub fn data_client_id(hostname: &str, device_name: &str) -> String {
    let base = if !hostname.trim().is_empty() {
        hostname.trim()
    } else if !device_name.trim().is_empty() {
        device_name.trim()
    } else {
        "arcadematrix"
    };
    format!("{}-data", base)
}

/// State shared between the engine (render thread) and its MQTT thread.
/// Lives only while the slot is active.
pub struct Shared {
    stop: AtomicBool,
    state: AtomicU8,
    /// Milliseconds after session start when the broker first accepted us (0 = not yet).
    connected_at_ms: AtomicU64,
    /// Bumped on every page update so the engine knows to rebuild.
    generation: AtomicU64,
    /// One slot per topic, in topic order.
    pages: Vec<ArcSwapOption<PageData>>,
}

impl Shared {
    pub fn new(topics: usize) -> Self {
        Self {
            stop: AtomicBool::new(false),
            state: AtomicU8::new(STATE_CONNECTING),
            connected_at_ms: AtomicU64::new(0),
            generation: AtomicU64::new(0),
            pages: (0..topics).map(|_| ArcSwapOption::empty()).collect(),
        }
    }

    /// Stores (or with `None`, clears) the page of topic `idx`.
    pub fn set_page(&self, idx: usize, page: Option<PageData>) {
        if let Some(slot) = self.pages.get(idx) {
            slot.store(page.map(Arc::new));
            self.generation.fetch_add(1, Ordering::Release);
        }
    }

    pub fn page(&self, idx: usize) -> Option<Arc<PageData>> {
        self.pages.get(idx).and_then(|s| s.load_full())
    }

    pub fn state(&self) -> u8 {
        self.state.load(Ordering::Acquire)
    }

    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }
}

/// The MQTT side of one activation: connect, subscribe, parse into `shared`,
/// until `shared.stop` is set; then disconnect.
fn run_session(
    shared: Arc<Shared>,
    cfg: DataMqttConfig,
    client_id: String,
    topics: Vec<String>,
    started: Instant,
) {
    let mut options = MqttOptions::new(client_id.clone(), cfg.broker.trim(), cfg.port);
    options.set_keep_alive(Duration::from_secs(30));
    options.set_max_packet_size(MAX_PAYLOAD_BYTES + 1024, 1024);
    if !cfg.user.is_empty() {
        options.set_credentials(cfg.user.clone(), cfg.pass.clone());
    }
    info!(
        "Home Assistant: connecting to {}:{} as '{}' for {} topic(s)",
        cfg.broker,
        cfg.port,
        client_id,
        topics.len()
    );
    let (client, mut connection) = Client::new(options, 16);
    let mut backoff = Duration::from_millis(500);
    let mut warned_large = false;

    while !shared.stop.load(Ordering::Acquire) {
        match connection.recv_timeout(POLL_SLICE) {
            Ok(Ok(Event::Incoming(Packet::ConnAck(_)))) => {
                shared.state.store(STATE_CONNECTED, Ordering::Release);
                let ms = started.elapsed().as_millis().max(1) as u64;
                let _ = shared.connected_at_ms.compare_exchange(
                    0,
                    ms,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                );
                backoff = Duration::from_millis(500);
                // Clean session: (re)subscribe on every connect so the broker
                // sends the retained snapshot.
                for t in &topics {
                    if let Err(e) = client.try_subscribe(t.as_str(), QoS::AtMostOnce) {
                        warn!("Home Assistant: subscribe to '{}' failed: {}", t, e);
                    }
                }
            }
            Ok(Ok(Event::Incoming(Packet::Publish(p)))) => {
                let Some(idx) = topics.iter().position(|t| *t == p.topic) else {
                    continue;
                };
                if p.payload.len() > MAX_PAYLOAD_BYTES {
                    if !warned_large {
                        warned_large = true;
                        warn!(
                            "Home Assistant: payload on '{}' too large ({} > {} bytes)",
                            p.topic,
                            p.payload.len(),
                            MAX_PAYLOAD_BYTES
                        );
                    }
                    continue;
                }
                // Empty payload = retained message deleted: the page goes back to NO DATA.
                let page = std::str::from_utf8(&p.payload).ok().and_then(parse_page);
                shared.set_page(idx, page);
            }
            Ok(Ok(_)) => {}
            Ok(Err(e)) => {
                if shared.connected_at_ms.load(Ordering::Acquire) == 0 {
                    shared.state.store(STATE_FAILED, Ordering::Release);
                } else {
                    shared.state.store(STATE_CONNECTING, Ordering::Release);
                }
                warn!(
                    "Home Assistant: connection error: {} (retry in {:?})",
                    e, backoff
                );
                // Sleep in short steps so deactivation never waits long.
                let until = Instant::now() + backoff;
                while Instant::now() < until && !shared.stop.load(Ordering::Acquire) {
                    std::thread::sleep(Duration::from_millis(50));
                }
                backoff = (backoff * 2).min(Duration::from_secs(5));
            }
            Err(_timeout) => {}
        }
    }
    let _ = client.try_disconnect();
    let _ = connection.recv_timeout(Duration::from_millis(50));
    info!("Home Assistant: disconnected");
}

/// One displayed page: topic index and, for weather topics, the day slide.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageRef {
    pub topic: usize,
    pub sub: usize,
}

/// Everything allocated for one activation; dropped on deactivate.
struct Session {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
    started: Instant,
    seen_generation: u64,
    seen_wday: u32,
    pages: Vec<PageRef>,
    /// Weather slides per topic (empty for non-weather topics).
    weather: Vec<Vec<DayForecast>>,
    current: usize,
    page_started: Instant,
    /// The current weather slide, pre-drawn (rebuilt only when it changes).
    weather_image: Option<(PageRef, u64, image::RgbImage)>,
    axis_scratch: (String, String),
}

impl Session {
    fn stop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.stop();
    }
}

/// What the current page shows when it has no data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Notice {
    Connecting,
    NoConnection,
    NoData,
}

/// Which notice a page without data shows, from the connection state.
pub fn notice_for(state: u8, connected_for: Option<Duration>) -> Notice {
    match (state, connected_for) {
        (STATE_FAILED, None) => Notice::NoConnection,
        (_, Some(d)) if d >= NO_DATA_AFTER => Notice::NoData,
        _ => Notice::Connecting,
    }
}

/// Draws a notice centred in G1 (layout spec section D).
pub fn draw_notice(m: &mut dyn MatrixBackend, notice: Notice, lang: Lang) {
    let (text, color) = match notice {
        Notice::Connecting => (i18n::connecting_label(lang), NOTICE_COLOR),
        Notice::NoConnection => (i18n::no_connection_label(lang), NO_CONNECTION_COLOR),
        Notice::NoData => (i18n::no_data_label(lang), NOTICE_COLOR),
    };
    let (w, h) = (m.width() as i32, m.height() as i32);
    let x = ((w - measure_text(text)) / 2).max(0);
    let y = ((h - 7) / 2).max(0);
    m.clear();
    draw_text_scaled(m, text, x, y, 0, w, 0, h, 1, color);
}

/// Builds the page list: one page per topic, a weather topic expanding to one
/// page per slide (at least one).
pub fn build_pages(weather: &[Vec<DayForecast>], topics: usize) -> Vec<PageRef> {
    let mut pages = Vec::with_capacity(topics + 4);
    for topic in 0..topics {
        let n = weather.get(topic).map(|w| w.len()).unwrap_or(0).max(1);
        for sub in 0..n {
            pages.push(PageRef { topic, sub });
        }
    }
    pages
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

fn weekday_now() -> u32 {
    use chrono::Datelike;
    chrono::Local::now().weekday().num_days_from_sunday()
}

pub struct HomeAssistantEngine {
    topics: Vec<String>,
    page_seconds: u64,
    show_header: bool,
    lang: Lang,
    lang_code: String,
    active: bool,
    session: Option<Session>,
}

impl Default for HomeAssistantEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl HomeAssistantEngine {
    pub fn new() -> Self {
        Self {
            topics: Vec::new(),
            page_seconds: DEFAULT_PAGE_SECONDS as u64,
            show_header: true,
            lang: Lang::En,
            lang_code: "en".to_string(),
            active: false,
            session: None,
        }
    }

    fn read_config(&mut self, config: &dyn EngineConfig) {
        self.topics = parse_topics(&config.get_string("topics", ""));
        self.page_seconds = config
            .get_int("page_seconds", DEFAULT_PAGE_SECONDS)
            .max(MIN_PAGE_SECONDS) as u64;
        self.show_header = config.get_bool("show_header", true);
    }

    /// Whether a session (thread, client, buffers) currently exists.
    pub fn has_session(&self) -> bool {
        self.session.is_some()
    }

    /// Starts the connection for this activation (first update after activate).
    fn start_session(&mut self, ctx: &EngineContext) {
        let (cfg, client_id) = {
            let s = ctx.config.settings.read();
            (
                s.data_mqtt.clone(),
                data_client_id(&s.wifi.hostname, &s.mqtt.device_name),
            )
        };
        let shared = Arc::new(Shared::new(self.topics.len()));
        let started = Instant::now();
        let thread = if cfg.broker.trim().is_empty() || self.topics.is_empty() {
            // Nothing to connect to: show NO CONNECTION straight away.
            shared.state.store(STATE_FAILED, Ordering::Release);
            None
        } else {
            let (sh, topics) = (Arc::clone(&shared), self.topics.clone());
            std::thread::Builder::new()
                .name("ha-mqtt".to_string())
                .spawn(move || run_session(sh, cfg, client_id, topics, started))
                .map_err(|e| warn!("Home Assistant: failed to start MQTT thread: {}", e))
                .ok()
        };
        self.session = Some(Session {
            shared,
            thread,
            started,
            seen_generation: u64::MAX,
            seen_wday: u32::MAX,
            pages: build_pages(&[], self.topics.len()),
            weather: vec![Vec::new(); self.topics.len()],
            current: 0,
            page_started: Instant::now(),
            weather_image: None,
            axis_scratch: (String::with_capacity(16), String::with_capacity(16)),
        });
    }

    /// The page currently shown (for tests and diagnostics).
    pub fn current_page(&self) -> Option<PageRef> {
        self.session
            .as_ref()
            .and_then(|s| s.pages.get(s.current).copied())
    }

    /// Test hook: the session's shared state (to inject pages without a broker).
    pub fn shared(&self) -> Option<Arc<Shared>> {
        self.session.as_ref().map(|s| Arc::clone(&s.shared))
    }
}

impl Engine for HomeAssistantEngine {
    fn initialize(
        &mut self,
        ctx: &mut EngineContext,
        config: &dyn EngineConfig,
    ) -> Result<(), EngineError> {
        self.read_config(config);
        self.lang_code = ctx.config.settings.read().system.lang.clone();
        self.lang = lang_from_code(&self.lang_code);
        Ok(())
    }

    fn activate(&mut self) {
        self.active = true;
        self.session = None; // a fresh session starts on the next update()
    }

    fn update(&mut self, ctx: &mut EngineContext) {
        if !self.active {
            return;
        }
        let lang_code = ctx.config.settings.read().system.lang.clone();
        if lang_code != self.lang_code {
            self.lang = lang_from_code(&lang_code);
            self.lang_code = lang_code;
            if let Some(s) = self.session.as_mut() {
                s.seen_generation = u64::MAX; // relabel weather slides
            }
        }
        if self.session.is_none() {
            self.start_session(ctx);
        }
        let page_time = Duration::from_secs(self.page_seconds);
        let lang_code = self.lang_code.clone();
        let Some(s) = self.session.as_mut() else {
            return;
        };

        // Rebuild the page list when data (or the day) changed.
        let generation = s.shared.generation();
        let wday = weekday_now();
        if generation != s.seen_generation || wday != s.seen_wday {
            s.seen_generation = generation;
            s.seen_wday = wday;
            for (i, slides) in s.weather.iter_mut().enumerate() {
                *slides = match s.shared.page(i).as_deref() {
                    Some(PageData::Weather(w)) => weather::ha_forecasts(w, &lang_code, wday),
                    _ => Vec::new(),
                };
            }
            let current = s.pages.get(s.current).copied();
            s.pages = build_pages(&s.weather, s.weather.len());
            s.current = current
                .and_then(|c| s.pages.iter().position(|p| *p == c))
                .unwrap_or(0);
            s.weather_image = None;
        }

        // Cycle pages.
        if s.pages.len() > 1 && s.page_started.elapsed() >= page_time {
            s.current = (s.current + 1) % s.pages.len();
            s.page_started = Instant::now();
        }

        // Pre-draw the current weather slide off the render path.
        if let Some(page) = s.pages.get(s.current).copied() {
            if let Some(slide) = s.weather.get(page.topic).and_then(|w| w.get(page.sub)) {
                let fresh = matches!(&s.weather_image,
                    Some((p, g, _)) if *p == page && *g == s.seen_generation);
                if !fresh {
                    let (w, h) = (ctx.matrix.width(), ctx.matrix.height());
                    let mut img = image::RgbaImage::new(w, h);
                    draw_weather_page(&mut img, slide, 0, w, h, 0, 0);
                    let rgb = image::DynamicImage::ImageRgba8(img).to_rgb8();
                    s.weather_image = Some((page, s.seen_generation, rgb));
                }
            }
        }
    }

    fn render(&mut self, ctx: &mut EngineContext) {
        let lang = self.lang;
        let show_header = self.show_header;
        let Some(s) = self.session.as_mut() else {
            draw_notice(ctx.matrix, Notice::Connecting, lang);
            return;
        };
        let Some(page) = s.pages.get(s.current).copied() else {
            draw_notice(ctx.matrix, Notice::NoConnection, lang);
            return;
        };
        let data = s.shared.page(page.topic);
        match data.as_deref() {
            Some(PageData::Graph(g)) => graph::render_graph_with(
                ctx.matrix,
                Some(g),
                show_header,
                i18n::no_data_label(lang),
                &mut s.axis_scratch,
            ),
            Some(PageData::Values(v)) => {
                values::render_values(ctx.matrix, Some(v), show_header, i18n::no_data_label(lang))
            }
            Some(PageData::Weather(_)) => {
                ctx.matrix.clear();
                if let Some((_, _, img)) = &s.weather_image {
                    ctx.matrix.draw_image(img, 0, 0);
                } else {
                    draw_notice(ctx.matrix, Notice::NoData, lang);
                }
            }
            None => {
                let at = s.shared.connected_at_ms.load(Ordering::Acquire);
                let connected_for = (at > 0).then(|| {
                    s.started
                        .elapsed()
                        .saturating_sub(Duration::from_millis(at))
                });
                draw_notice(
                    ctx.matrix,
                    notice_for(s.shared.state(), connected_for),
                    lang,
                );
            }
        }
    }

    fn deactivate(&mut self) {
        self.active = false;
        // Stops and joins the MQTT thread, disconnects, frees every buffer.
        self.session = None;
    }

    fn on_config_changed(&mut self, config: &dyn EngineConfig) {
        let before = (self.topics.clone(), self.page_seconds);
        self.read_config(config);
        if before.0 != self.topics {
            // New topics: reconnect on the next update.
            self.session = None;
        }
    }
}

#[distributed_slice(crate::core::registry::ENGINES)]
fn register_homeassistant_engine() -> EngineDescriptor {
    EngineDescriptor {
        metadata: EngineMetadata {
            id: "homeassistant",
            name: "Home Assistant",
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
                    id: "topics",
                    field_type: ConfigType::String,
                    label: "MQTT Topics",
                    description: "Up to 6 full topics, comma separated; one page each (a weather topic shows its NOW page and forecast days)",
                    default_value: "",
                    validation_policy: ValidationPolicy::Accept,
                    ..Default::default()
                },
                ConfigField {
                    id: "page_seconds",
                    field_type: ConfigType::Integer,
                    label: "Seconds per Page",
                    description: "How long each page stays on screen",
                    default_value: "10",
                    min_val: Some("3"),
                    max_val: Some("600"),
                    validation_policy: ValidationPolicy::Clamp,
                    ..Default::default()
                },
                ConfigField {
                    id: "show_header",
                    field_type: ConfigType::Boolean,
                    label: "Show Header",
                    description: "Show the graph header / values title",
                    default_value: "true",
                    validation_policy: ValidationPolicy::FallbackDefault,
                    ..Default::default()
                },
            ],
        },
        factory: || Box::new(HomeAssistantEngine::new()),
    }
}
