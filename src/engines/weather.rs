use crate::api::{DayForecast, WeatherProvider};
use crate::core::engine_contract::{Engine, EngineConfig, EngineContext, EngineError};
use crate::engines::renderers::BaseRenderer;
use image::{imageops, RgbImage};
use linkme::distributed_slice;

use std::time::{Duration, Instant};

pub struct WeatherEngine {
    base_renderer: BaseRenderer,
    providers: Vec<Box<dyn WeatherProvider>>,
    forecasts: Vec<DayForecast>,
    last_fetch: Option<Instant>,
    /// Pre-rendered panorama (3 × matrix_width) as RgbImage
    panorama: Option<RgbImage>,
    panorama_w: u32,
    panorama_mw: u32,
    scroll_start: Instant,
    api_key: String,
    city: String,
    lang: String,
    last_lang: String,
    units: String,
    last_units: String,
    offset_x: i32,
    offset_y: i32,
    /// `source = ha`: forecasts come from a Home Assistant payload on the data
    /// feed MQTT connection instead of an OpenWeatherMap HTTP fetch.
    source_ha: bool,
    topic: String,
    ha_seen_generation: u64,
    /// Inputs of the last HA build: payload, language, weekday (labels change at midnight).
    ha_built: Option<(
        std::sync::Arc<crate::core::data_feed::HaWeather>,
        String,
        u32,
    )>,
}

impl WeatherEngine {
    pub fn new() -> Self {
        Self {
            base_renderer: BaseRenderer::new(),
            providers: Vec::new(),
            forecasts: Vec::new(),
            last_fetch: None,
            panorama: None,
            panorama_w: 0,
            panorama_mw: 0,
            scroll_start: Instant::now(),
            api_key: "".to_string(),
            city: "".to_string(),
            lang: "en".to_string(),
            last_lang: "en".to_string(),
            units: "metric".to_string(),
            last_units: "metric".to_string(),
            offset_x: 0,
            offset_y: 0,
            source_ha: false,
            topic: HA_DEFAULT_TOPIC.to_string(),
            ha_seen_generation: u64::MAX,
            ha_built: None,
        }
    }

    pub fn add_provider(&mut self, provider: Box<dyn WeatherProvider>) {
        self.providers.push(provider);
    }

    /// Parse the instance config into engine state. Shared by `initialize()`
    /// and `on_config_changed()` so edits apply live without an app restart.
    fn apply_config(&mut self, config: &dyn EngineConfig) {
        self.api_key = config.get_string("api_key", "");
        self.city = config.get_string("city", "");
        self.lang = config.get_string("lang", "en");
        self.units = config.get_string("units", "metric");
        self.offset_x = config.get_int("offset_x", 0);
        self.offset_y = config.get_int("offset_y", 0);
        self.source_ha = config.get_string("source", "owm").trim() == "ha";
        let topic = config.get_string("topic", HA_DEFAULT_TOPIC);
        self.topic = if topic.trim().is_empty() {
            HA_DEFAULT_TOPIC.to_string()
        } else {
            topic.trim().to_string()
        };
        self.ha_seen_generation = u64::MAX;
        self.ha_built = None;
    }

    /// HA mode: rebuild `forecasts` when the payload, language or day changed.
    /// Allocates only on such a change, not per frame.
    fn refresh_ha(&mut self, lang: &str) {
        let store = crate::core::data_feed::data_feed();
        let generation = store.generation();
        let wday = chrono::Local::now()
            .format("%w")
            .to_string()
            .parse::<u32>()
            .unwrap_or(0);
        let latest = if generation != self.ha_seen_generation {
            self.ha_seen_generation = generation;
            store.get(&self.topic).and_then(|e| e.weather.clone())
        } else {
            self.ha_built.as_ref().map(|b| b.0.clone())
        };
        let Some(w) = latest else {
            if self.ha_built.take().is_some() || !self.forecasts.is_empty() {
                self.forecasts.clear();
                self.panorama = None;
            }
            return;
        };
        let unchanged = matches!(&self.ha_built,
            Some((prev, l, d)) if std::sync::Arc::ptr_eq(prev, &w) && l == lang && *d == wday);
        if unchanged {
            return;
        }
        self.forecasts = ha_forecasts(&w, lang, wday);
        self.panorama = None;
        self.ha_built = Some((w, lang.to_string(), wday));
    }
}

impl Engine for WeatherEngine {
    fn initialize(
        &mut self,
        _context: &mut EngineContext,
        config: &dyn EngineConfig,
    ) -> Result<(), EngineError> {
        self.apply_config(config);
        Ok(())
    }

    fn on_config_changed(&mut self, config: &dyn EngineConfig) {
        self.apply_config(config);
        // City/lang/offset edits must take effect now: drop cached forecast and
        // pre-rendered panorama so the next render refetches and rebuilds.
        self.last_fetch = None;
        self.forecasts.clear();
        self.panorama = None;
    }

    fn activate(&mut self) {
        self.last_fetch = None; // Force refresh on activation
        self.ha_seen_generation = u64::MAX;
    }

    fn on_display_geometry_changed(&mut self, _geometry: &crate::core::types::DisplayGeometry) {
        self.panorama = None;
    }

    fn update(&mut self, _context: &mut EngineContext) {
        // Handle logic that doesn't draw
    }

    fn render(&mut self, context: &mut EngineContext) {
        if self.source_ha {
            let (sys_lang, sys_unit) = {
                let s = context.config.settings.read();
                (s.system.lang.clone(), s.system.temp_unit.clone())
            };
            let lang = if !sys_lang.is_empty() {
                sys_lang
            } else {
                "fr".to_string()
            };
            self.refresh_ha(&lang);
            if self.forecasts.is_empty() {
                // Same "no data yet" notice as the OpenWeatherMap path.
                let empty_text = if sys_unit.eq_ignore_ascii_case("F") {
                    "--°F"
                } else {
                    "--°C"
                };
                self.base_renderer.render_text(
                    context.matrix,
                    empty_text,
                    0,
                    2,
                    self.offset_x,
                    self.offset_y,
                    None,
                    None,
                );
                return;
            }
        } else {
            if self.api_key.is_empty() || self.api_key == "API_KEY" {
                self.base_renderer.render_text(
                    context.matrix,
                    "No API key",
                    0,
                    1,
                    0,
                    0,
                    Some((180, 80, 80)),
                    None,
                );
                return;
            }
            if self.city.is_empty() {
                self.base_renderer.render_text(
                    context.matrix,
                    "No city",
                    0,
                    1,
                    0,
                    0,
                    Some((180, 80, 80)),
                    None,
                );
                return;
            }

            let (sys_lang, sys_unit) = {
                let s = context.config.settings.read();
                (s.system.lang.clone(), s.system.temp_unit.clone())
            };

            let active_lang = if !self.lang.is_empty() && self.lang != "system" {
                self.lang.clone()
            } else if !sys_lang.is_empty() {
                sys_lang
            } else {
                "fr".to_string()
            };

            let active_units = if !self.units.is_empty() && self.units != "system" {
                self.units.clone()
            } else if sys_unit.eq_ignore_ascii_case("F") {
                "imperial".to_string()
            } else {
                "metric".to_string()
            };

            if self.last_lang != active_lang || self.last_units != active_units {
                self.last_lang = active_lang.clone();
                self.last_units = active_units.clone();
                self.last_fetch = None;
                self.forecasts.clear();
                self.panorama = None;
            }

            let should_fetch = self
                .last_fetch
                .map(|t| t.elapsed() > Duration::from_secs(1800))
                .unwrap_or(true);

            if should_fetch {
                self.fetch_forecast(
                    &self.api_key.clone(),
                    &self.city.clone(),
                    &active_lang,
                    &active_units,
                );
            }

            if self.forecasts.is_empty() {
                let empty_text = if active_units.eq_ignore_ascii_case("imperial")
                    || active_units.eq_ignore_ascii_case("fahrenheit")
                    || active_units.eq_ignore_ascii_case("f")
                {
                    "--°F"
                } else {
                    "--°C"
                };
                self.base_renderer.render_text(
                    context.matrix,
                    empty_text,
                    0,
                    2,
                    self.offset_x,
                    self.offset_y,
                    None,
                    None,
                );
                return;
            }
        }

        let mw = context.matrix.width();
        let mh = context.matrix.height();

        // (Re)build panorama if needed
        if self.panorama.is_none() || self.panorama_mw != mw {
            self.build_panorama(mw, mh, self.offset_x, self.offset_y);
        }

        // Animated horizontal scroll with ease-in/out
        let num_slides = self.forecasts.len() as u64;
        let slide_dur = 5u64;
        let trans_dur = 1u64;
        let cycle = (slide_dur + trans_dur) * num_slides;

        let t = self.scroll_start.elapsed().as_secs() % cycle;
        let slide_idx = t / (slide_dur + trans_dur);
        let local_t = t % (slide_dur + trans_dur);

        let x_scroll = if local_t < slide_dur {
            (slide_idx * mw as u64) as u32
        } else {
            let progress = (local_t - slide_dur) as f32 / trans_dur as f32;
            let ease = progress * progress * (3.0 - 2.0 * progress);
            ((slide_idx as f32 + ease) * mw as f32) as u32
        };

        // Crop view from panorama
        if let Some(ref pano) = self.panorama {
            let view_x = x_scroll.min(pano.width().saturating_sub(mw));
            let view = imageops::crop_imm(pano, view_x, 0, mw, mh);
            let view_img = view.to_image();
            context.matrix.draw_image(&view_img, 0, 0);
        }

        // Render dynamic scrolling condition in Tate / vertical layout when text overflows
        let is_wide = mw >= 128;
        let is_tall = mh >= 64;
        let is_vertical = !(is_wide && is_tall)
            && !(is_wide && !is_tall)
            && !(mw >= 256 && mh >= 64)
            && !(mw <= 64 && mh <= 32);

        if is_vertical && !self.forecasts.is_empty() {
            let desc_y = if mh >= 120 {
                self.offset_y + (mh as i32 / 5) + 32
            } else if mh >= 96 {
                self.offset_y + 50
            } else {
                self.offset_y + 40
            };

            let elapsed_ms = self.scroll_start.elapsed().as_millis() as u64;
            let cycle_ms = (slide_dur + trans_dur) * 1000 * num_slides;
            let t_ms = elapsed_ms % cycle_ms;
            let cur_idx = (t_ms / ((slide_dur + trans_dur) * 1000)) as usize % self.forecasts.len();
            let local_ms = t_ms % ((slide_dur + trans_dur) * 1000);

            let font = self.base_renderer.font();
            let cur_slide = &self.forecasts[cur_idx];
            let (_, cur_w, _) = font.get_pixel_map(&cur_slide.condition, 1.0);

            let color_desc = (210, 210, 210);

            if local_ms < slide_dur * 1000 {
                // Stationary slide
                if cur_w > mw as i32 - 4 {
                    let overflow = cur_w - (mw as i32 - 4);
                    let x = if local_ms < 1000 {
                        2 + self.offset_x
                    } else if local_ms < 4200 {
                        let p = (local_ms - 1000) as f32 / 3200.0;
                        2 + self.offset_x - (p * overflow as f32) as i32
                    } else {
                        2 + self.offset_x - overflow
                    };
                    BaseRenderer::draw_text_clipped(
                        context.matrix,
                        &cur_slide.condition,
                        &font,
                        1.0,
                        x,
                        desc_y,
                        0,
                        mw as i32,
                        color_desc,
                        (0, 0, 0),
                    );
                }
            } else {
                // Transitioning slide
                let trans_ms = local_ms - slide_dur * 1000;
                let progress = trans_ms as f32 / (trans_dur * 1000) as f32;
                let ease = progress * progress * (3.0 - 2.0 * progress);
                let ease_dx = (ease * mw as f32) as i32;

                // Outgoing slide condition
                if cur_w > mw as i32 - 4 {
                    let overflow = cur_w - (mw as i32 - 4);
                    let end_x = 2 + self.offset_x - overflow;
                    BaseRenderer::draw_text_clipped(
                        context.matrix,
                        &cur_slide.condition,
                        &font,
                        1.0,
                        end_x - ease_dx,
                        desc_y,
                        0,
                        mw as i32,
                        color_desc,
                        (0, 0, 0),
                    );
                }

                // Incoming slide condition
                let next_idx = (cur_idx + 1) % self.forecasts.len();
                let next_slide = &self.forecasts[next_idx];
                let (_, next_w, _) = font.get_pixel_map(&next_slide.condition, 1.0);
                if next_w > mw as i32 - 4 {
                    let in_x = (2 + self.offset_x) + (mw as i32 - ease_dx);
                    BaseRenderer::draw_text_clipped(
                        context.matrix,
                        &next_slide.condition,
                        &font,
                        1.0,
                        in_x,
                        desc_y,
                        0,
                        mw as i32,
                        color_desc,
                        (0, 0, 0),
                    );
                }
            }
        }
    }

    fn deactivate(&mut self) {
        // Cleanup if necessary
    }
}

impl WeatherEngine {
    fn draw_arcade_text(
        &self,
        img: &mut image::RgbaImage,
        text: &str,
        start_x: i32,
        start_y: i32,
        color: (u8, u8, u8),
        scale: f32,
    ) {
        let font = self.base_renderer.font();
        let (pixels_by_char, _, _) = font.get_pixel_map(text, scale);

        for char_pixels in pixels_by_char {
            for (px, py) in char_pixels {
                let draw_x = start_x + px;
                let draw_y = start_y + py;
                if draw_x >= 0
                    && draw_x < img.width() as i32
                    && draw_y >= 0
                    && draw_y < img.height() as i32
                {
                    img.put_pixel(
                        draw_x as u32,
                        draw_y as u32,
                        image::Rgba([color.0, color.1, color.2, 255]),
                    );
                }
            }
        }
    }

    fn build_panorama(&mut self, mw: u32, mh: u32, offset_x: i32, offset_y: i32) {
        let num_slides = self.forecasts.len() as u32 + 1; // +1 for wrap-around
        let pano_w = mw * num_slides;
        let mut panorama = image::RgbaImage::new(pano_w, mh);

        let slides: Vec<DayForecast> = self
            .forecasts
            .iter()
            .cloned()
            .chain(std::iter::once(self.forecasts[0].clone()))
            .collect();

        let is_wide = mw >= 128;
        let is_tall = mh >= 64;

        for (i, slide) in slides.iter().enumerate() {
            let base_x = i as u32 * mw;
            let font = self.base_renderer.font();

            // In °F the high goes above the low (US convention): swap the two
            // temperature rows for forecast days. °C keeps low on top.
            let high_on_top = !slide.is_now && slide.temp_max.ends_with("°F");
            let swapped;
            let slide = if high_on_top {
                swapped = DayForecast {
                    temp_min: slide.temp_max.clone(),
                    temp_max: slide.temp_min.clone(),
                    ..slide.clone()
                };
                &swapped
            } else {
                slide
            };

            let color_label = (180, 180, 255);
            let color_desc = (210, 210, 210);
            let color_low = (120, 200, 255);
            let color_high = (255, 150, 50);
            // Top / bottom temperature row colours. The HA "NOW" page draws the
            // live reading in white and the humidity / wind line in the condition grey.
            let color_morning = if slide.is_now {
                (255, 255, 255)
            } else if high_on_top {
                color_high
            } else {
                color_low
            };
            let color_afternoon = if slide.is_now {
                color_desc
            } else if high_on_top {
                color_low
            } else {
                color_high
            };

            if mw >= 256 && mh >= 64 {
                // --- 256x64: three columns, as the ESP32 `drawForecast` (layout-v2.md B) ---
                let (row1, row2) = (slide.temp_min.as_str(), slide.temp_max.as_str());
                let margin = 8;
                let icon_x = base_x as i32 + margin + offset_x;
                let icon_y = (mh as i32 - 48) / 2 + offset_y;
                Self::draw_icon_scaled(&mut panorama, &slide.icon, icon_x, icon_y, 2);

                let row_top = 10 + offset_y;
                let row_bottom = 38 + offset_y;
                let right_edge = base_x as i32 + mw as i32 - margin + offset_x;
                let temp_w = if slide.is_now {
                    glcd_w(row1, 2)
                } else {
                    glcd_w(row1, 2).max(glcd_w(row2, 2))
                };
                let mut canvas = SlideCanvas::new(&mut panorama, base_x as i32, mw as i32);
                canvas.text(
                    row1,
                    right_edge - glcd_w(row1, 2),
                    row_top,
                    2,
                    color_morning,
                );
                canvas.text(
                    row2,
                    right_edge - glcd_w(row2, 2),
                    row_bottom,
                    2,
                    color_afternoon,
                );

                let mid_x = icon_x + 48 + margin;
                let mid_w = (right_edge - temp_w - margin) - mid_x;
                let mut label = if slide.label_long.is_empty() {
                    slide.label.as_str()
                } else {
                    slide.label_long.as_str()
                };
                if glcd_w(label, 2) > mid_w {
                    label = &slide.label;
                }
                canvas.text(label, mid_x, row_top, 2, color_label);

                let mut desc = if slide.condition_long.is_empty() {
                    slide.condition.as_str()
                } else {
                    slide.condition_long.as_str()
                };
                if !desc.is_empty() {
                    let size = if glcd_w(desc, 2) <= mid_w { 2 } else { 1 };
                    if size == 1 && glcd_w(desc, 1) > mid_w {
                        desc = &slide.condition;
                    }
                    let y = if size == 2 {
                        row_bottom
                    } else {
                        row_bottom + 4
                    };
                    canvas.text(desc, mid_x, y, size, color_desc);
                }
            } else if is_wide && !is_tall {
                // --- 128x32, as the ESP32 `drawForecast` (layout-v2.md B) ---
                let (row1, row2) = (slide.temp_min.as_str(), slide.temp_max.as_str());
                let icon_x = base_x as i32 + 4 + offset_x;
                let icon_y = (mh as i32 - 24) / 2 + offset_y;
                Self::draw_icon_scaled(&mut panorama, &slide.icon, icon_x, icon_y, 1);

                let temp_x = icon_x + 28;
                let max_len = if slide.is_now {
                    row1.chars().count()
                } else {
                    row1.chars().count().max(row2.chars().count())
                };
                let text_w = max_len as i32 * 6;
                let mut canvas = SlideCanvas::new(&mut panorama, base_x as i32, mw as i32);
                canvas.text(row1, temp_x, 4 + offset_y, 1, color_morning);
                canvas.text(row2, temp_x, 18 + offset_y, 1, color_afternoon);

                let slide_x = base_x as i32;
                let mut right_x = temp_x + text_w + 6;
                if right_x < slide_x + 60 + offset_x {
                    right_x = slide_x + 60 + offset_x;
                }
                let max_right = slide_x + mw as i32 - 42;
                if right_x > max_right {
                    right_x = max_right;
                }
                canvas.text(&slide.label, right_x, 4 + offset_y, 1, color_label);

                if !slide.condition.is_empty() {
                    let avail_w = slide_x + mw as i32 - right_x - 2;
                    let max_chars = (avail_w / 6).max(1) as usize;
                    let n = slide.condition.chars().count();
                    let prefix = |k: usize| -> &str {
                        match slide.condition.char_indices().nth(k) {
                            Some((i, _)) => &slide.condition[..i],
                            None => slide.condition.as_str(),
                        }
                    };
                    let desc = if n > max_chars {
                        if max_chars > 3 {
                            format!("{}.", prefix(max_chars - 1))
                        } else {
                            prefix(max_chars).to_string()
                        }
                    } else {
                        slide.condition.clone()
                    };
                    canvas.text(&desc, right_x, 18 + offset_y, 1, color_desc);
                }
            } else if is_tall && is_wide {
                // --- 128x64 or larger Spacious Layout ---
                let icon_x = base_x as i32 + offset_x + 6;
                let icon_y = (mh as i32 - 24) / 2 + offset_y;
                self.draw_icon(&mut panorama, &slide.icon, icon_x, icon_y);

                let temp_x = icon_x + 30;
                self.draw_arcade_text(
                    &mut panorama,
                    &slide.temp_min,
                    temp_x,
                    offset_y + 12,
                    color_morning,
                    2.0,
                );
                self.draw_arcade_text(
                    &mut panorama,
                    &slide.temp_max,
                    temp_x,
                    offset_y + 38,
                    color_afternoon,
                    2.0,
                );

                let (_, min_w, _) = font.get_pixel_map(&slide.temp_min, 2.0);
                let (_, max_w, _) = font.get_pixel_map(&slide.temp_max, 2.0);
                let max_w = if slide.is_now { 0 } else { max_w };
                let mut right_x = temp_x + min_w.max(max_w) + 8;
                if right_x < base_x as i32 + 72 + offset_x {
                    right_x = base_x as i32 + 72 + offset_x;
                }
                let max_right = (base_x + mw) as i32 - 46;
                if right_x > max_right {
                    right_x = max_right;
                }

                self.draw_arcade_text(
                    &mut panorama,
                    &slide.label,
                    right_x,
                    offset_y + 14,
                    color_label,
                    1.0,
                );

                if !slide.condition.is_empty() {
                    let avail_w = ((base_x + mw) as i32 - right_x - 2).max(6);
                    let max_chars = (avail_w / 6).max(1) as usize;
                    let mut cond = slide.condition.clone();
                    if cond.chars().count() > max_chars {
                        if max_chars > 3 {
                            cond = format!(
                                "{}.",
                                &cond[..cond
                                    .char_indices()
                                    .nth(max_chars - 1)
                                    .map(|(i, _)| i)
                                    .unwrap_or(cond.len())]
                            );
                        } else {
                            cond = cond[..cond
                                .char_indices()
                                .nth(max_chars)
                                .map(|(i, _)| i)
                                .unwrap_or(cond.len())]
                                .to_string();
                        }
                    }
                    self.draw_arcade_text(
                        &mut panorama,
                        &cond,
                        right_x,
                        offset_y + 38,
                        color_desc,
                        1.0,
                    );
                }
            } else if mw <= 64 && mh <= 32 {
                // --- 64x32 Compact Horizontal Layout ---
                let icon_x = base_x as i32 + offset_x + 2;
                let icon_y = (mh as i32 - 24) / 2 + offset_y;
                self.draw_icon(&mut panorama, &slide.icon, icon_x, icon_y);

                let right_x = icon_x + 26;
                let avail_w = ((base_x + mw) as i32 - right_x - 1).max(6);
                let max_chars = (avail_w / 6).max(1) as usize;

                // Line 1: Day Label
                self.draw_arcade_text(
                    &mut panorama,
                    &slide.label,
                    right_x,
                    offset_y + 2,
                    color_label,
                    1.0,
                );

                // Line 2: Temperatures (Min + Max)
                self.draw_arcade_text(
                    &mut panorama,
                    &slide.temp_min,
                    right_x,
                    offset_y + 11,
                    color_morning,
                    1.0,
                );
                let (_, min_w, _) = font.get_pixel_map(&slide.temp_min, 1.0);
                self.draw_arcade_text(
                    &mut panorama,
                    &slide.temp_max,
                    right_x + min_w + 2,
                    offset_y + 11,
                    color_afternoon,
                    1.0,
                );

                // Line 3: Condition
                if !slide.condition.is_empty() {
                    let mut cond = slide.condition.clone();
                    if cond.chars().count() > max_chars {
                        if max_chars > 3 {
                            cond = format!(
                                "{}.",
                                &cond[..cond
                                    .char_indices()
                                    .nth(max_chars - 1)
                                    .map(|(i, _)| i)
                                    .unwrap_or(cond.len())]
                            );
                        } else {
                            cond = cond[..cond
                                .char_indices()
                                .nth(max_chars)
                                .map(|(i, _)| i)
                                .unwrap_or(cond.len())]
                                .to_string();
                        }
                    }
                    self.draw_arcade_text(
                        &mut panorama,
                        &cond,
                        right_x,
                        offset_y + 21,
                        color_desc,
                        1.0,
                    );
                }
            } else {
                // --- 64x64 or Vertical Layout ---
                let (icon_y, desc_y, min_y, max_y) = if mh >= 120 {
                    (
                        offset_y + (mh as i32 / 5),
                        offset_y + (mh as i32 / 5) + 32,
                        offset_y + (3 * mh as i32 / 5),
                        offset_y + (3 * mh as i32 / 5) + 14,
                    )
                } else if mh >= 96 {
                    (offset_y + 20, offset_y + 50, offset_y + 64, offset_y + 76)
                } else {
                    (offset_y + 14, offset_y + 40, offset_y + 49, offset_y + 57)
                };

                let icon_x = base_x as i32 + (mw as i32 - 24) / 2 + offset_x;
                self.draw_icon(&mut panorama, &slide.icon, icon_x, icon_y);

                let (_, label_w, _) = font.get_pixel_map(&slide.label, 1.0);
                let label_x = base_x as i32 + (mw as i32 - label_w) / 2 + offset_x;
                self.draw_arcade_text(
                    &mut panorama,
                    &slide.label,
                    label_x,
                    offset_y + 3,
                    color_label,
                    1.0,
                );

                if !slide.condition.is_empty() {
                    let (_, desc_w, _) = font.get_pixel_map(&slide.condition, 1.0);
                    // If condition fits within the display width, draw it static and centered in panorama.
                    // If it overflows, leave it out of panorama so render() draws it with horizontal scrolling!
                    if desc_w <= mw as i32 - 4 {
                        let desc_x = base_x as i32 + (mw as i32 - desc_w) / 2 + offset_x;
                        self.draw_arcade_text(
                            &mut panorama,
                            &slide.condition,
                            desc_x,
                            desc_y,
                            color_desc,
                            1.0,
                        );
                    }
                }

                let (_, min_w, _) = font.get_pixel_map(&slide.temp_min, 1.0);
                let min_x = base_x as i32 + (mw as i32 - min_w) / 2 + offset_x;
                self.draw_arcade_text(
                    &mut panorama,
                    &slide.temp_min,
                    min_x,
                    min_y,
                    color_morning,
                    1.0,
                );

                let (_, max_w, _) = font.get_pixel_map(&slide.temp_max, 1.0);
                let max_x = base_x as i32 + (mw as i32 - max_w) / 2 + offset_x;
                self.draw_arcade_text(
                    &mut panorama,
                    &slide.temp_max,
                    max_x,
                    max_y,
                    color_afternoon,
                    1.0,
                );
            }
        }

        let mut rgb_panorama = RgbImage::new(pano_w, mh);
        for y in 0..mh {
            for x in 0..pano_w {
                let px = panorama.get_pixel(x, y);
                rgb_panorama.put_pixel(x, y, image::Rgb([px[0], px[1], px[2]]));
            }
        }

        self.panorama = Some(rgb_panorama);
        self.panorama_w = pano_w;
        self.panorama_mw = mw;
        self.scroll_start = Instant::now();
    }

    fn draw_icon(&self, img: &mut image::RgbaImage, icon: &str, x: i32, y: i32) {
        Self::draw_icon_scaled(img, icon, x, y, 1);
    }

    /// The 24x24 weather icon (same design as the ESP32 `drawIcon`) at an
    /// integer scale: coordinates and radii are multiplied, and a 1 px line
    /// becomes `scale` parallel lines offset in x.
    fn draw_icon_scaled(img: &mut image::RgbaImage, icon: &str, x: i32, y: i32, scale: i32) {
        use image::Rgba;
        use imageproc::drawing::{
            draw_filled_circle_mut, draw_filled_rect_mut, draw_line_segment_mut,
        };
        use imageproc::rect::Rect;

        let s = scale.max(1);
        let px = |v: i32| x + v * s;
        let py = |v: i32| y + v * s;
        let circle = |img: &mut image::RgbaImage, cx: i32, cy: i32, r: i32, c: Rgba<u8>| {
            draw_filled_circle_mut(img, (px(cx), py(cy)), r * s, c);
        };
        let rect = |img: &mut image::RgbaImage, rx: i32, ry: i32, w: u32, h: u32, c: Rgba<u8>| {
            draw_filled_rect_mut(
                img,
                Rect::at(px(rx), py(ry)).of_size(w * s as u32, h * s as u32),
                c,
            );
        };
        let line = |img: &mut image::RgbaImage, x0: i32, y0: i32, x1: i32, y1: i32, c: Rgba<u8>| {
            for o in 0..s {
                draw_line_segment_mut(
                    img,
                    ((px(x0) + o) as f32, py(y0) as f32),
                    ((px(x1) + o) as f32, py(y1) as f32),
                    c,
                );
            }
        };

        let yellow = Rgba([255, 255, 0, 255]);
        let dark_yellow = Rgba([255, 200, 0, 255]);
        let light_grey = Rgba([200, 200, 200, 255]);
        let dark_grey = Rgba([150, 150, 150, 255]);
        let thunder_grey = Rgba([100, 100, 100, 255]);
        let blue = Rgba([0, 150, 255, 255]);
        let white = Rgba([255, 255, 255, 255]);
        let green = Rgba([0, 255, 0, 255]);

        if icon.contains("01") {
            // Sun
            circle(img, 12, 12, 6, yellow);
            line(img, 12, 2, 12, 4, dark_yellow);
            line(img, 12, 20, 12, 22, dark_yellow);
            line(img, 2, 12, 4, 12, dark_yellow);
            line(img, 20, 12, 22, 12, dark_yellow);
            line(img, 5, 5, 7, 7, dark_yellow);
            line(img, 19, 19, 17, 17, dark_yellow);
            line(img, 19, 5, 17, 7, dark_yellow);
            line(img, 5, 19, 7, 17, dark_yellow);
        } else if icon.contains("02") || icon.contains("03") || icon.contains("04") {
            // Clouds
            if icon.contains("02") {
                // Sun behind cloud
                circle(img, 8, 8, 4, yellow);
            }
            circle(img, 8, 14, 5, light_grey);
            circle(img, 14, 11, 6, white);
            circle(img, 20, 14, 5, light_grey);
            rect(img, 8, 14, 12, 6, light_grey);
        } else if icon.contains("09") || icon.contains("10") {
            // Rain
            circle(img, 8, 10, 5, dark_grey);
            circle(img, 14, 8, 6, light_grey);
            circle(img, 20, 10, 5, dark_grey);
            rect(img, 8, 10, 12, 6, dark_grey);
            line(img, 8, 18, 6, 22, blue);
            line(img, 14, 18, 12, 22, blue);
            line(img, 20, 18, 18, 22, blue);
        } else if icon.contains("11") {
            // Thunder
            circle(img, 8, 10, 5, thunder_grey);
            circle(img, 14, 8, 6, dark_grey);
            circle(img, 20, 10, 5, thunder_grey);
            rect(img, 8, 10, 12, 6, thunder_grey);
            line(img, 14, 16, 10, 20, yellow);
            line(img, 10, 20, 16, 20, yellow);
            line(img, 16, 20, 12, 24, yellow);
        } else if icon.contains("13") {
            // Snow
            circle(img, 14, 14, 2, white);
            line(img, 14, 8, 14, 20, white);
            line(img, 8, 14, 20, 14, white);
            line(img, 10, 10, 18, 18, white);
            line(img, 18, 10, 10, 18, white);
        } else {
            // Unknown
            circle(img, 12, 12, 6, green);
        }
    }

    fn fetch_forecast(&mut self, api_key: &str, city: &str, lang: &str, units: &str) {
        self.last_fetch = Some(Instant::now());
        self.panorama = None; // Invalidate panorama cache

        for provider in &self.providers {
            if let Some(forecasts) = provider.fetch_forecast(api_key, city, lang, units) {
                self.forecasts = forecasts;
                return;
            }
        }

        tracing::warn!("Failed to fetch weather forecast from all providers");
        self.forecasts.clear();
    }
}

/// Width of `text` in the GLCD 5x7 font at `size`, as the ESP32 computes it
/// (`len*6*size - size`, 0 for empty text).
fn glcd_w(text: &str, size: i32) -> i32 {
    let n = text.chars().count() as i32;
    if n == 0 {
        0
    } else {
        n * 6 * size - size
    }
}

/// Draws GLCD text into one slide of the weather panorama, clipped to that slide.
struct SlideCanvas<'a> {
    img: &'a mut image::RgbaImage,
    x0: i32,
    w: i32,
}

impl<'a> SlideCanvas<'a> {
    fn new(img: &'a mut image::RgbaImage, x0: i32, w: i32) -> Self {
        Self { img, x0, w }
    }

    fn text(&mut self, text: &str, x: i32, y: i32, size: i32, c: (u8, u8, u8)) {
        let (x0, x1) = (self.x0, self.x0 + self.w);
        let h = self.img.height() as i32;
        crate::engines::dashboard::font::draw_text_scaled(self, text, x, y, x0, x1, 0, h, size, c);
    }
}

impl crate::core::matrix::MatrixBackend for SlideCanvas<'_> {
    fn width(&self) -> u32 {
        self.img.width()
    }
    fn height(&self) -> u32 {
        self.img.height()
    }
    fn set_pixel(&mut self, x: i32, y: i32, r: u8, g: u8, b: u8) {
        if x >= 0 && y >= 0 && (x as u32) < self.img.width() && (y as u32) < self.img.height() {
            self.img
                .put_pixel(x as u32, y as u32, image::Rgba([r, g, b, 255]));
        }
    }
    fn clear(&mut self) {}
    fn update(&mut self) {}
    fn set_brightness(&mut self, _brightness: u8) {}
}

/// Default topic of the Home Assistant weather payload (`source = ha`).
pub const HA_DEFAULT_TOPIC: &str = "arcadematrix/weather/local";

fn ha_temp(v: Option<f32>, unit_sym: &str) -> String {
    match v {
        Some(t) => format!("{}{}", t.round() as i64, unit_sym),
        None => format!("--{}", unit_sym),
    }
}

/// Builds the per-day slides from a Home Assistant weather payload, filling
/// the same `DayForecast` fields the OpenWeatherMap provider fills: day 0 =
/// today with the live station temperature; labels from the firmware's own
/// day logic; conditions mapped to the OWM icon codes and localized labels.
pub fn ha_forecasts(
    w: &crate::core::data_feed::HaWeather,
    lang: &str,
    current_wday: u32,
) -> Vec<DayForecast> {
    use crate::core::i18n::{self, Lang};
    let l = Lang::from_code(lang);
    let unit_sym = if w.imperial { "°F" } else { "°C" };
    // Days come only from the forecast; with none, the NOW page stands alone (as on the ESP32).
    let days = &w.days;
    let mut slides: Vec<DayForecast> = Vec::with_capacity(days.len() + 1);
    // "NOW" page first: the live station reading, humidity and wind.
    if let Some(t) = w.current_temp {
        let cond_label = i18n::ha_weather_condition(l, &w.current_condition)
            .map(|(short, _)| short.to_string())
            .unwrap_or_else(|| w.current_condition.clone());
        let hum = w.humidity.map(|h| format!("{}%", h.round() as i64));
        let wind = w
            .wind
            .map(|v| format!("{}{}", v.round() as i64, w.wind_unit.trim()));
        let line2 = match (hum, wind) {
            // Two spaces so "34%  5mph" doesn't read as one value.
            (Some(h), Some(v)) => format!("{}  {}", h, v),
            (Some(h), None) => h,
            (None, Some(v)) => v,
            (None, None) => cond_label,
        };
        slides.push(DayForecast {
            label: i18n::weather_now_label(l).to_string(),
            label_long: String::new(),
            temp: ha_temp(Some(t), unit_sym),
            temp_min: ha_temp(Some(t), unit_sym),
            temp_max: line2,
            condition: String::new(),
            condition_long: String::new(),
            icon: i18n::ha_weather_icon(&w.current_condition).to_string(),
            is_now: true,
        });
    }
    slides.extend(days.iter().enumerate().map(|(i, d)| {
        let cond_raw = if i == 0 && d.condition.trim().is_empty() {
            w.current_condition.as_str()
        } else {
            d.condition.as_str()
        };
        let (condition, condition_long) = i18n::ha_weather_condition(l, cond_raw)
            .map(|(short, long)| (short.to_string(), long.to_string()))
            .unwrap_or_else(|| (cond_raw.to_string(), cond_raw.to_string()));
        let temp = if i == 0 {
            w.current_temp.or(d.temp_max)
        } else {
            d.temp_max
        };
        DayForecast {
            label: i18n::weather_day_label(l, ((current_wday as usize) + i) % 7, i == 0, i == 1)
                .to_string(),
            label_long: i18n::weather_day_label_long(
                l,
                ((current_wday as usize) + i) % 7,
                i == 0,
                i == 1,
            )
            .to_string(),
            temp: ha_temp(temp, unit_sym),
            temp_min: ha_temp(d.temp_min, unit_sym),
            temp_max: ha_temp(d.temp_max, unit_sym),
            condition,
            condition_long,
            icon: i18n::ha_weather_icon(cond_raw).to_string(),
            is_now: false,
        }
    }));
    slides
}

use crate::core::engine_contract::{
    Capabilities, ConfigSchema, EngineDescriptor, EngineMetadata, Requirements,
};
#[distributed_slice(crate::core::registry::ENGINES)]
fn register_weather_engine() -> EngineDescriptor {
    EngineDescriptor {
        metadata: EngineMetadata {
            id: "weather",
            name: "WeatherEngine",
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
                crate::core::engine_contract::ConfigField {
                    id: "source",
                    field_type: crate::core::engine_contract::ConfigType::Options,
                    label: "Source",
                    description: "Where the forecast comes from",
                    default_value: "owm",
                    options: Some(vec![
                        crate::core::engine_contract::ConfigOption {
                            label: "OpenWeatherMap",
                            value: "owm",
                        },
                        crate::core::engine_contract::ConfigOption {
                            label: "Home Assistant (MQTT)",
                            value: "ha",
                        },
                    ]),
                    validation_policy:
                        crate::core::engine_contract::ValidationPolicy::FallbackDefault,
                    ..Default::default()
                },
                crate::core::engine_contract::ConfigField {
                    id: "topic",
                    field_type: crate::core::engine_contract::ConfigType::String,
                    label: "MQTT Topic",
                    description:
                        "Topic of the Home Assistant weather payload on the Data Feed MQTT broker",
                    default_value: HA_DEFAULT_TOPIC,
                    visible_when: Some("source=ha"),
                    validation_policy: crate::core::engine_contract::ValidationPolicy::Accept,
                    ..Default::default()
                },
                crate::core::engine_contract::ConfigField {
                    id: "api_key",
                    field_type: crate::core::engine_contract::ConfigType::String,
                    label: "API Key",
                    description: "OpenWeatherMap API Key",
                    default_value: "",
                    visible_when: Some("source=owm"),
                    validation_policy: crate::core::engine_contract::ValidationPolicy::Accept,
                    ..Default::default()
                },
                crate::core::engine_contract::ConfigField {
                    id: "city",
                    field_type: crate::core::engine_contract::ConfigType::String,
                    label: "City",
                    description: "City name for forecast (e.g. Paris,FR or for US: Tucson,AZ,US)",
                    default_value: "",
                    visible_when: Some("source=owm"),
                    validation_policy: crate::core::engine_contract::ValidationPolicy::Accept,
                    ..Default::default()
                },
                crate::core::engine_contract::ConfigField {
                    id: "units",
                    field_type: crate::core::engine_contract::ConfigType::Options,
                    label: "Units",
                    description: "Temperature unit: metric (°C) or imperial (°F)",
                    default_value: "metric",
                    options: Some(vec![
                        crate::core::engine_contract::ConfigOption {
                            label: "Celsius (°C)",
                            value: "metric",
                        },
                        crate::core::engine_contract::ConfigOption {
                            label: "Fahrenheit (°F)",
                            value: "imperial",
                        },
                    ]),
                    visible_when: Some("source=owm"),
                    validation_policy:
                        crate::core::engine_contract::ValidationPolicy::FallbackDefault,
                    ..Default::default()
                },
                crate::core::engine_contract::ConfigField {
                    id: "offset_x",
                    field_type: crate::core::engine_contract::ConfigType::Integer,
                    label: "X Offset",
                    description: "Horizontal shift",
                    default_value: "0",
                    min_val: Some("-64"),
                    max_val: Some("64"),
                    validation_policy: crate::core::engine_contract::ValidationPolicy::Clamp,
                    ..Default::default()
                },
                crate::core::engine_contract::ConfigField {
                    id: "offset_y",
                    field_type: crate::core::engine_contract::ConfigType::Integer,
                    label: "Y Offset",
                    description: "Vertical shift",
                    default_value: "0",
                    min_val: Some("-32"),
                    max_val: Some("32"),
                    validation_policy: crate::core::engine_contract::ValidationPolicy::Clamp,
                    ..Default::default()
                },
            ],
        },
        factory: || -> Box<dyn crate::core::engine_contract::Engine> {
            let mut engine = WeatherEngine::new();
            // Wire the HTTP provider (see crypto.rs factory for why this is required).
            engine.add_provider(Box::new(crate::api::openweathermap::OpenWeatherMapProvider));
            Box::new(engine)
        },
    }
}
