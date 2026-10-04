//! Weather pages for the MQTT Data engine: a weather payload expands to a
//! NOW page (the live station reading) followed by one page per forecast day,
//! drawn with the shared weather page renderer (same layout as the
//! OpenWeatherMap weather engine and the ESP32 firmware).

use super::payload::WeatherPayload;
use crate::api::DayForecast;

fn fmt_temp(v: Option<f32>, unit_sym: &str) -> String {
    match v {
        Some(t) => format!("{}{}", t.round() as i64, unit_sym),
        None => format!("--{}", unit_sym),
    }
}

/// Builds the per-day slides from a weather payload, filling
/// the same `DayForecast` fields the OpenWeatherMap provider fills: day 0 =
/// today with the live station temperature; labels from the firmware's own
/// day logic; conditions mapped to the OWM icon codes and localized labels.
pub fn weather_pages(w: &WeatherPayload, lang: &str, current_wday: u32) -> Vec<DayForecast> {
    use crate::core::i18n::{self, Lang};
    let l = Lang::from_code(lang);
    let unit_sym = if w.imperial { "°F" } else { "°C" };
    // Days come only from the forecast; with none, the NOW page stands alone (as on the ESP32).
    let days = &w.days;
    let mut slides: Vec<DayForecast> = Vec::with_capacity(days.len() + 1);
    // "NOW" page first: the live station reading, humidity and wind.
    if let Some(t) = w.current_temp {
        let cond_label = i18n::weather_condition_labels(l, &w.current_condition)
            .map(|(short, _)| short.to_string())
            .unwrap_or_else(|| w.current_condition.clone());
        let hum = w.humidity.map(|h| format!("{}%", h.round() as i64));
        let wind = w
            .wind
            .map(|v| format!("{}{}", v.round() as i64, w.wind_unit.trim()));
        // Two spaces so "34%  5mph" doesn't read as one value.
        let join = |h: &Option<String>, v: &Option<String>| match (h, v) {
            (Some(h), Some(v)) => format!("{}  {}", h, v),
            (Some(h), None) => h.clone(),
            (None, Some(v)) => v.clone(),
            (None, None) => cond_label.clone(),
        };
        // Wind direction goes in front of the speed ("41%  NE 9mph"); it never stands alone.
        let wind_dir = wind
            .as_ref()
            .filter(|_| !w.wind_dir.is_empty())
            .map(|v| format!("{} {}", w.wind_dir, v));
        let (line2, now_line2_short) = match wind_dir {
            Some(wd) => (join(&hum, &Some(wd)), join(&hum, &wind)),
            None => (join(&hum, &wind), String::new()),
        };
        slides.push(DayForecast {
            label: i18n::weather_now_label(l).to_string(),
            label_long: String::new(),
            temp: fmt_temp(Some(t), unit_sym),
            temp_min: fmt_temp(Some(t), unit_sym),
            temp_max: line2,
            condition: String::new(),
            condition_long: String::new(),
            icon: i18n::weather_icon_code(&w.current_condition).to_string(),
            is_now: true,
            now_line2_short,
        });
    }
    slides.extend(days.iter().enumerate().map(|(i, d)| {
        let cond_raw = if i == 0 && d.condition.trim().is_empty() {
            w.current_condition.as_str()
        } else {
            d.condition.as_str()
        };
        let (condition, condition_long) = i18n::weather_condition_labels(l, cond_raw)
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
            temp: fmt_temp(temp, unit_sym),
            temp_min: fmt_temp(d.temp_min, unit_sym),
            temp_max: fmt_temp(d.temp_max, unit_sym),
            condition,
            condition_long,
            icon: i18n::weather_icon_code(cond_raw).to_string(),
            is_now: false,
            now_line2_short: String::new(),
        }
    }));
    slides
}
