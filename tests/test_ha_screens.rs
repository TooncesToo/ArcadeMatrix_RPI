//! Round 2 data-feed screens: Graph lines + y range, Values engine, Weather HA source.

use arcadematrix::core::config::Config;
use arcadematrix::core::data_feed::{
    data_feed, parse_graph_payload, parse_values_payload, parse_weather_payload, MAX_PAYLOAD_BYTES,
};
use arcadematrix::core::engine_contract::{Engine, EngineContext, HashConfig};
use arcadematrix::core::matrix::{MatrixBackend, MockMatrix};
use arcadematrix::core::registry::EngineRegistry;
use arcadematrix::engines::graph::{render_graph, YScale};
use arcadematrix::engines::renderers::tiny_font::{draw_tiny_text, tiny_text_width};
use arcadematrix::engines::values::{fit_value, render_values};
use arcadematrix::engines::weather::{ha_forecasts, WeatherEngine};
use std::collections::HashMap;

fn px(m: &MockMatrix, x: u32, y: u32) -> (u8, u8, u8) {
    let p = m.canvas.get_pixel(x, y);
    (p[0], p[1], p[2])
}

fn lit_in(
    m: &MockMatrix,
    xs: std::ops::Range<u32>,
    ys: std::ops::Range<u32>,
    c: (u8, u8, u8),
) -> bool {
    xs.clone().any(|x| ys.clone().any(|y| px(m, x, y) == c))
}

/// The store has a single writer in production (the MQTT thread); parallel
/// tests must not publish at the same time or they can race for a free slot.
fn publish_test(topic: &str, payload: &str) {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    data_feed().publish(topic, payload);
}

const RED: (u8, u8, u8) = (0xFF, 0x40, 0x20);
const BLUE: (u8, u8, u8) = (0x20, 0x60, 0xFF);

// ---------------------------------------------------------------------------
// Graph v2
// ---------------------------------------------------------------------------

#[test]
fn payload_limit_is_8k() {
    assert_eq!(MAX_PAYLOAD_BYTES, 8192);
}

#[test]
fn old_payloads_keep_v1_range() {
    let g = parse_graph_payload(r#"{"max":6,"series":[{"data":[1,2]}]}"#).unwrap();
    assert_eq!((g.lo, g.hi), (0.0, 6.0));
    let auto = parse_graph_payload(r#"{"series":[{"data":[1,3]},{"data":[2,2]}]}"#).unwrap();
    assert_eq!((auto.lo, auto.hi), (0.0, 5.0));
}

#[test]
fn explicit_min_max_and_auto_ranges() {
    let g = parse_graph_payload(r#"{"min":68,"max":90,"series":[{"style":"line","data":[70]}]}"#)
        .unwrap();
    assert_eq!((g.lo, g.hi), (68.0, 90.0));
    // max <= min -> auto max.
    let g = parse_graph_payload(r#"{"min":10,"max":5,"series":[{"data":[20]}]}"#).unwrap();
    assert_eq!((g.lo, g.hi), (10.0, 20.0));
    // Lines: data range padded by max(5 % of span, 0.5).
    let g = parse_graph_payload(r#"{"series":[{"style":"line","data":[70,null,90]}]}"#).unwrap();
    assert_eq!((g.lo, g.hi), (69.0, 91.0));
    let g = parse_graph_payload(r#"{"series":[{"style":"line","data":[72,72.2]}]}"#).unwrap();
    assert!((g.lo - 71.5).abs() < 1e-4 && (g.hi - 72.7).abs() < 1e-4);
    // Negative bars: range goes below 0.
    let g = parse_graph_payload(r#"{"series":[{"data":[2,-3]},{"data":[1,-1]}]}"#).unwrap();
    assert_eq!((g.lo, g.hi), (-4.0, 3.0));
}

#[test]
fn null_is_gap_for_lines_and_zero_for_bars() {
    let g = parse_graph_payload(
        r#"{"series":[{"style":"line","data":[1,null,3]},{"data":[1,null,3]}]}"#,
    )
    .unwrap();
    assert!(g.series[0].line && !g.series[1].line);
    assert_eq!(g.value_opt(&g.series[0], 1), None);
    assert_eq!(g.value_at(&g.series[1], 1), 0.0);
}

#[test]
fn y_mapping_matches_layout() {
    let ys = YScale {
        lo: 68.0,
        hi: 90.0,
        gh: 24,
        h: 32,
    };
    assert_eq!(ys.line_y(90.0), 8);
    assert_eq!(ys.line_y(68.0), 31);
    assert_eq!(ys.line_y(200.0), 8); // clamps
    assert_eq!(ys.line_y(-5.0), 31);
    assert_eq!(ys.rows(68.0), 0);
    assert_eq!(ys.rows(90.0), 24);
}

#[test]
fn line_draws_steps_and_gaps() {
    // 4 slots on 128 columns -> 32 columns each, plot rows 0..31 (no header).
    let g = parse_graph_payload(
        r##"{"min":0,"max":31,"series":[{"style":"line","color":"#FF4020","data":[0,31,null,10]}]}"##,
    )
    .unwrap();
    let mut m = MockMatrix::new(128, 32);
    render_graph(&mut m, Some(&g), false, "");
    assert_eq!(px(&m, 0, 31), RED); // slot 0 at value 0 -> bottom row
    assert_eq!(px(&m, 31, 31), RED);
    // Step at column 32: vertical run from row 31 up to row 0.
    for y in 0..32 {
        assert_eq!(px(&m, 32, y), RED, "row {}", y);
    }
    assert_eq!(px(&m, 40, 0), RED);
    assert_eq!(px(&m, 40, 10), (0, 0, 0));
    // Gap: slot 2 has nothing, and slot 3 starts with a single pixel (no join).
    assert!((64..96).all(|x| (0..32).all(|y| px(&m, x, y) == (0, 0, 0))));
    assert_eq!(px(&m, 96, 21), RED); // 10/31 of 31 rows -> 31-10 = 21
    assert_eq!(px(&m, 96, 22), (0, 0, 0));
}

#[test]
fn negative_bars_hang_below_zero_line() {
    let g =
        parse_graph_payload(r##"{"min":-2,"max":2,"series":[{"color":"#2060FF","data":[2,-2]}]}"##)
            .unwrap();
    let mut m = MockMatrix::new(128, 32);
    render_graph(&mut m, Some(&g), false, "");
    // Zero line: row 31 - round(0.5*31) = 15 (16 -> rounds half away -> 15.5 -> 16? computed below).
    let zero = YScale {
        lo: -2.0,
        hi: 2.0,
        gh: 32,
        h: 32,
    }
    .line_y(0.0);
    // Positive bar fills the top half, negative bar the bottom half.
    assert_eq!(px(&m, 10, 0), BLUE);
    assert_eq!(px(&m, 10, 31), (0, 0, 0));
    assert_eq!(px(&m, 100, 31), BLUE);
    assert_eq!(px(&m, 100, 0), (0, 0, 0));
    // Zero line visible where no bar covers it? Both bars cover their half; the
    // line pixel is under one of them, so check the colour exists on its row.
    assert!(zero >= 15 && zero <= 16);
}

#[test]
fn zero_line_only_with_bars_crossing_zero() {
    let lines =
        parse_graph_payload(r#"{"min":-5,"max":5,"series":[{"style":"line","data":[]}]}"#).unwrap();
    let mut m = MockMatrix::new(128, 32);
    render_graph(&mut m, Some(&lines), false, "");
    assert!(m
        .canvas
        .pixels()
        .all(|p| p[0] == 0 && p[1] == 0 && p[2] == 0));

    let bars = parse_graph_payload(r#"{"min":-5,"max":5,"series":[{"data":[]}]}"#).unwrap();
    render_graph(&mut m, Some(&bars), false, "");
    let zy = YScale {
        lo: -5.0,
        hi: 5.0,
        gh: 32,
        h: 32,
    }
    .line_y(0.0) as u32;
    assert_eq!(px(&m, 50, zy), (0x30, 0x30, 0x30));
}

#[test]
fn yaxis_only_on_tall_panels() {
    let g = parse_graph_payload(
        r##"{"title":"T","min":68,"max":90,"yaxis":true,"series":[{"style":"line","color":"#FF4020","data":[90]}]}"##,
    )
    .unwrap();
    let mut big = MockMatrix::new(256, 64);
    render_graph(&mut big, Some(&g), true, "");
    // Labels "90"/"68": width 7 -> plot starts at x=8.
    assert!(lit_in(&big, 0..7, 15..20, (0x70, 0x70, 0x70)));
    assert!(lit_in(&big, 0..7, 59..64, (0x70, 0x70, 0x70)));
    assert_eq!(px(&big, 7, 15), (0, 0, 0)); // gap column
    assert_eq!(px(&big, 8, 15), RED); // first plot column, value = max -> top row
    let mut small = MockMatrix::new(128, 32);
    render_graph(&mut small, Some(&g), true, "");
    assert_eq!(px(&small, 0, 8), RED); // no axis on 32 px panels
}

#[test]
fn tiny_font_widths_and_pixels() {
    assert_eq!(tiny_text_width(""), 0);
    assert_eq!(tiny_text_width("90"), 7);
    assert_eq!(tiny_text_width("kWh"), 11);
    let mut m = MockMatrix::new(16, 8);
    draw_tiny_text(&mut m, "1", 0, 0, 0, 16, (255, 255, 255));
    // '1' = 010 110 010 010 111
    assert_eq!(px(&m, 1, 0), (255, 255, 255));
    assert_eq!(px(&m, 0, 0), (0, 0, 0));
    assert_eq!(px(&m, 0, 1), (255, 255, 255));
    assert!((0..3).all(|x| px(&m, x, 4) == (255, 255, 255)));
}

// ---------------------------------------------------------------------------
// Values
// ---------------------------------------------------------------------------

const ENERGY: &str = r##"{"v":1,"title":"ENERGY","tiles":[
  {"label":"HOUSE","value":"2.8","unit":"kW","color":"#FFB000"},
  {"label":"TODAY","value":"31.2","unit":"kWh","color":"#FFFFFF"},
  {"label":"AC $","value":"0.35","unit":"","color":"#40C0FF"},
  {"label":"OUT","value":"75","unit":"°F","color":"#FF6040"}]}"##;

#[test]
fn values_parse_and_limits() {
    let d = parse_values_payload(ENERGY).unwrap();
    assert_eq!(d.title, "ENERGY");
    assert_eq!(d.tiles.len(), 4);
    assert_eq!(d.tiles[0].label_color, (0x80, 0x80, 0x80));
    assert_eq!(d.tiles[3].unit, "°F");
    let five =
        r#"{"tiles":[{"value":"1"},{"value":"2"},{"value":"3"},{"value":"4"},{"value":"5"}]}"#;
    assert_eq!(parse_values_payload(five).unwrap().tiles.len(), 4);
    let num = parse_values_payload(r#"{"tiles":[{"value":42}]}"#).unwrap();
    assert_eq!(num.tiles[0].value, "42");
    assert_eq!(num.tiles[0].color, (255, 255, 255));
    assert!(parse_values_payload(r#"{"title":"x"}"#).is_none());
}

#[test]
fn fit_rule_drops_unit_but_never_truncates_value() {
    // Fits big with unit.
    let f = fit_value("2.8", "kW", &[3, 2, 1], 128, 24);
    assert_eq!((f.scale, f.show_unit, f.chars), (3, true, 3));
    // Too tall for G3 in 16 rows -> G2.
    let f = fit_value("2.8", "kW", &[3, 2, 1], 128, 16);
    assert_eq!(f.scale, 2);
    // G1 only. "31.2"+gap+"kWh"(tiny) = 23+2+11 = 36.
    let f = fit_value("31.2", "kWh", &[1], 36, 16);
    assert!(f.show_unit);
    let f = fit_value("31.2", "kWh", &[1], 35, 16);
    assert_eq!((f.show_unit, f.chars), (false, 4));
    // Too wide even without the unit: drawn in full anyway.
    let f = fit_value("123456", "", &[1], 20, 16);
    assert_eq!((f.show_unit, f.chars), (false, 6));
}

const HOME: &str = r##"{"v":1,"title":"HOME","tiles":[
  {"label":"DOWNSTAIRS","label_short":"DOWN","value":"75","unit":"°F","color":"#FFFFFF"},
  {"label":"UPSTAIRS","label_short":"UP","value":"76","unit":"°F","color":"#FFFFFF"},
  {"label":"AC","label_short":"AC","value":"18.2","unit":"HOURS","color":"#40C0FF"},
  {"label":"OUTSIDE","label_short":"OUT","value":"90","unit":"°F","color":"#FF6040"}]}"##;

#[test]
fn values_grid_is_stacked_on_128x32() {
    let d = parse_values_payload(HOME).unwrap();
    assert_eq!(d.tiles[0].label_short, "DOWN");
    let mut m = MockMatrix::new(128, 32);
    render_values(&mut m, Some(&d), true, "NO DATA");
    let grey = (0x80, 0x80, 0x80);
    // Block = 5 + 1 + 7 = 13 rows, top = cell_y + 1: labels on rows 1..5 / 17..21,
    // values on rows 7..13 / 23..29.
    assert!(lit_in(&m, 0..62, 1..6, grey));
    assert!(!lit_in(&m, 0..128, 0..1, grey));
    assert!(lit_in(&m, 0..62, 7..14, (255, 255, 255)));
    assert!(!lit_in(&m, 0..62, 1..6, (255, 255, 255)));
    assert!(lit_in(&m, 0..62, 23..30, (0x40, 0xC0, 0xFF)));
    assert!(lit_in(&m, 66..128, 23..30, (0xFF, 0x60, 0x40)));
    // "DOWNSTAIRS" (39 px) fits the 62 px cell, so the full label is used: centred at x = 11.
    assert!(lit_in(&m, 11..13, 1..6, grey));
    assert!(!lit_in(&m, 0..11, 1..6, grey));
    // Gutter columns 62..65 stay dark.
    assert!((62..66).all(|x| (0..32).all(|y| px(&m, x, y) == (0, 0, 0))));
}

#[test]
fn short_label_used_only_when_full_does_not_fit() {
    let d = parse_values_payload(
        r#"{"tiles":[{"label":"DOWNSTAIRS","label_short":"DOWN","value":"1"},{"value":"2"},{"value":"3"}]}"#,
    )
    .unwrap();
    // 3 columns on 128x32: inner width 40 >= 39 -> full label still fits.
    let mut m = MockMatrix::new(128, 32);
    render_values(&mut m, Some(&d), false, "");
    let grey = (0x80, 0x80, 0x80);
    let (lo, hi) = (0..40)
        .filter(|&x| (0..32).any(|y| px(&m, x, y) == grey))
        .fold((u32::MAX, 0), |(a, b), x| (a.min(x), b.max(x)));
    assert_eq!(hi - lo + 1, 39);
    // A 30 px wide cell can't hold it -> "DOWN" (15 px).
    let d4 = parse_values_payload(
        r#"{"tiles":[{"label":"DOWNSTAIRS","label_short":"DOWN","value":"1"},{"value":"2"},{"value":"3"},{"value":"4"}]}"#,
    )
    .unwrap();
    let mut small = MockMatrix::new(64, 32); // 2x2 cells of 30 px
    render_values(&mut small, Some(&d4), false, "");
    let (lo, hi) = (0..30)
        .filter(|&x| (0..16).any(|y| px(&small, x, y) == grey))
        .fold((u32::MAX, 0), |(a, b), x| (a.min(x), b.max(x)));
    assert_eq!(hi - lo + 1, 15);
}

#[test]
fn six_char_label_fits_grid_with_unit() {
    // "AC 24H" (23 px tiny) + 2 + "17.0" (23) + 2 + "h" (3) = 53 <= 62 px inner cell.
    let aw = 62 - tiny_text_width("AC 24H") - 2;
    let f = fit_value("17.0", "h", &[1], aw, 16);
    assert!(f.show_unit && f.chars == 4);
}

#[test]
fn values_single_tile_stacks_under_title() {
    let d = parse_values_payload(
        r##"{"title":"POWER","tiles":[{"label":"HOUSE","value":"2.8","unit":"kW","color":"#FFB000"}]}"##,
    )
    .unwrap();
    let mut m = MockMatrix::new(128, 32);
    render_values(&mut m, Some(&d), true, "NO DATA");
    assert!(lit_in(&m, 0..30, 0..7, (255, 255, 255))); // title
                                                       // ch = 24: G3 doesn't fit (27), G2 block = 5 + 1 + 14 = 20, top = 8 + 2 = 10.
    assert!(lit_in(&m, 0..128, 10..15, (0x80, 0x80, 0x80))); // label rows 10..14
    assert!(lit_in(&m, 0..128, 16..30, (0xFF, 0xB0, 0x00))); // value rows 16..29
    assert!(!lit_in(&m, 0..128, 30..32, (0xFF, 0xB0, 0x00)));
}

#[test]
fn values_no_data() {
    let mut m = MockMatrix::new(128, 32);
    render_values(&mut m, None, true, "NO DATA");
    assert!(m.canvas.pixels().any(|p| p[0] > 0));
    let empty = parse_values_payload(r#"{"tiles":[]}"#).unwrap();
    let mut m2 = MockMatrix::new(128, 32);
    render_values(&mut m2, Some(&empty), true, "NO DATA");
    assert_eq!(m.canvas, m2.canvas);
}

#[test]
fn values_engine_registered() {
    let d = EngineRegistry::get_descriptor("values").expect("values descriptor");
    let ids: Vec<&str> = d.schema.fields.iter().map(|f| f.id).collect();
    assert_eq!(ids, vec!["topic", "show_title"]);
    assert_eq!(
        d.schema.fields[0].default_value,
        "arcadematrix/values/energy"
    );
    assert_eq!(EngineRegistry::engine_name_to_id("values"), 17);
}

// ---------------------------------------------------------------------------
// Weather HA source
// ---------------------------------------------------------------------------

const WEATHER: &str = r#"{"v":1,"units":"imperial",
 "current":{"temp":75.4,"condition":"clear-night","humidity":47,"wind":7,"wind_unit":"mph"},
 "days":[{"temp_max":89,"temp_min":70,"condition":"sunny","precip_prob":0},
         {"temp_max":90,"temp_min":71,"condition":"partlycloudy","precip_prob":5},
         {"temp_max":91,"temp_min":72,"condition":"lightning-rainy"},
         {"temp_max":88,"temp_min":69,"condition":"windy-variant"},
         {"temp_max":87,"temp_min":68,"condition":"weird"},
         {"temp_max":86,"temp_min":67,"condition":"sunny"}]}"#;

#[test]
fn weather_payload_to_day_forecasts() {
    let w = parse_weather_payload(WEATHER).unwrap();
    assert!(w.imperial);
    assert_eq!(w.days.len(), 5);
    let f = ha_forecasts(&w, "en", 5); // Friday
    assert_eq!(f.len(), 6); // NOW + 5 days
                            // NOW page: live reading (white line 1) + humidity / wind (grey line 2).
    assert!(f[0].is_now);
    assert_eq!(f[0].label, "NOW");
    assert_eq!(f[0].temp_min, "75°F");
    assert_eq!(f[0].temp_max, "47%  7mph");
    assert_eq!(f[0].icon, "01n"); // current.condition = clear-night
    assert!(f[0].condition.is_empty());
    assert!(!f[1].is_now);
    assert_eq!(f[1].label, "TODAY");
    assert_eq!(f[1].temp, "75°F"); // live station reading
    assert_eq!(
        (f[1].temp_min.as_str(), f[1].temp_max.as_str()),
        ("70°F", "89°F")
    );
    assert_eq!(f[1].condition, "Sunny");
    assert_eq!(f[1].icon, "01d");
    assert_eq!(f[2].icon, "02d");
    assert_eq!(f[3].condition, "Storm+rain");
    assert_eq!(f[3].icon, "11d");
    assert_eq!(f[4].icon, "03d");
    assert_eq!(f[5].condition, "weird");
    assert_eq!(f[5].icon, "03d");
    let fr = ha_forecasts(&w, "fr", 5);
    assert_eq!(fr[0].label, "ACTU.");
    assert_eq!(fr[1].condition, "Soleil");
    assert_eq!(ha_forecasts(&w, "es", 5)[0].label, "AHORA");
    let metric = parse_weather_payload(r#"{"units":"metric","current":{"temp":21}}"#).unwrap();
    let m = ha_forecasts(&metric, "en", 0);
    assert_eq!(m.len(), 2); // NOW + today built from current
    assert_eq!(m[0].temp_min, "21°C");
    assert_eq!(m[1].temp, "21°C");
    assert_eq!(m[1].temp_max, "--°C");
}

#[test]
fn now_page_second_line_falls_back() {
    let only_hum =
        parse_weather_payload(r#"{"current":{"temp":60,"humidity":30.4,"condition":"rainy"}}"#)
            .unwrap();
    assert_eq!(ha_forecasts(&only_hum, "en", 0)[0].temp_max, "30%");
    let only_wind =
        parse_weather_payload(r#"{"current":{"temp":60,"wind":12,"wind_unit":"km/h"}}"#).unwrap();
    assert_eq!(ha_forecasts(&only_wind, "en", 0)[0].temp_max, "12km/h");
    let neither = parse_weather_payload(r#"{"current":{"temp":60,"condition":"rainy"}}"#).unwrap();
    assert_eq!(ha_forecasts(&neither, "en", 0)[0].temp_max, "Rain");
    // No live temperature -> no NOW page.
    let no_temp = parse_weather_payload(r#"{"days":[{"temp_max":70,"temp_min":50}]}"#).unwrap();
    let f = ha_forecasts(&no_temp, "en", 0);
    assert_eq!(f.len(), 1);
    assert!(!f[0].is_now);
}

#[test]
fn now_page_widest_line_fits_128() {
    // "34%  12mph" (two-digit wind) must stay on a 128 px panel.
    let dir = std::env::temp_dir().join(format!("am_weather_fit_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let config = Config::new(dir.join("config.json"));
    let topic = "test/ha_screens/weather_fit";
    publish_test(
        topic,
        r#"{"units":"imperial","current":{"temp":104,"condition":"sunny","humidity":34,"wind":12,"wind_unit":"mph"}}"#,
    );
    let mut cfg = HashMap::new();
    cfg.insert("source".to_string(), "ha".to_string());
    cfg.insert("topic".to_string(), topic.to_string());
    let mut engine = WeatherEngine::new();
    let mut m = MockMatrix::new(128, 32);
    let mut ctx = EngineContext {
        matrix: &mut m,
        config: &config,
    };
    engine
        .initialize(&mut ctx, &HashConfig { data: &cfg })
        .unwrap();
    engine.activate();
    engine.update(&mut ctx);
    engine.render(&mut ctx);
    // Line 2 (grey) must not reach the last column, i.e. it isn't clipped.
    let grey = |x: u32| (0..32).any(|y| px(&m, x, y) == (210, 210, 210));
    let last = (0..128).rev().find(|&x| grey(x)).expect("line 2 drawn");
    assert!(last < 127, "line 2 ends at x={}", last);
    if let Ok(out) = std::env::var("HA_PREVIEW_DIR") {
        save(&m, &out, "weather_ha_now_34_12mph");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Colours of the top-most and bottom-most rows that use `c` in a region.
fn rows_with(m: &MockMatrix, xs: std::ops::Range<u32>, c: (u8, u8, u8)) -> Option<(u32, u32)> {
    let rows: Vec<u32> = (0..m.canvas.height())
        .filter(|&y| xs.clone().any(|x| px(m, x, y) == c))
        .collect();
    Some((*rows.first()?, *rows.last()?))
}

fn render_weather_day(payload: &str, w: u32, h: u32, name: &str) -> MockMatrix {
    let dir = std::env::temp_dir().join(format!("am_weather_{}_{}", name, std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let config = Config::new(dir.join("config.json"));
    let topic = format!("test/ha_screens/{}", name);
    publish_test(&topic, payload);
    let mut cfg = HashMap::new();
    cfg.insert("source".to_string(), "ha".to_string());
    cfg.insert("topic".to_string(), topic);
    let mut engine = WeatherEngine::new();
    let mut m = MockMatrix::new(w, h);
    let mut ctx = EngineContext {
        matrix: &mut m,
        config: &config,
    };
    engine
        .initialize(&mut ctx, &HashConfig { data: &cfg })
        .unwrap();
    engine.activate();
    engine.update(&mut ctx);
    engine.render(&mut ctx);
    let _ = std::fs::remove_dir_all(&dir);
    m
}

#[test]
fn fahrenheit_puts_high_above_low_celsius_does_not() {
    const LOW: (u8, u8, u8) = (120, 200, 255);
    const HIGH: (u8, u8, u8) = (255, 150, 50);
    // No current temp -> no NOW page, so the first page is TODAY.
    let f = r#"{"units":"imperial","days":[{"temp_max":89,"temp_min":70,"condition":"sunny"}]}"#;
    let c = r#"{"units":"metric","days":[{"temp_max":31,"temp_min":21,"condition":"sunny"}]}"#;
    for (w, h) in [(128u32, 32u32), (256, 64)] {
        let mf = render_weather_day(f, w, h, &format!("hi_f_{}", w));
        let (hi_top, _) = rows_with(&mf, 0..w, HIGH).unwrap();
        let (lo_top, _) = rows_with(&mf, 0..w, LOW).unwrap();
        assert!(hi_top < lo_top, "{}x{}: °F high must be on top", w, h);
        let mc = render_weather_day(c, w, h, &format!("hi_c_{}", w));
        let (hi_top, _) = rows_with(&mc, 0..w, HIGH).unwrap();
        let (lo_top, _) = rows_with(&mc, 0..w, LOW).unwrap();
        assert!(lo_top < hi_top, "{}x{}: °C keeps low on top", w, h);
    }
}

#[test]
fn day0_condition_falls_back_to_current() {
    let w = parse_weather_payload(
        r#"{"current":{"temp":60,"condition":"rainy"},"days":[{"temp_max":70,"temp_min":50}]}"#,
    )
    .unwrap();
    let f = ha_forecasts(&w, "en", 1);
    assert_eq!(f[1].condition, "Rain");
    assert_eq!(f[1].icon, "10d");
}

#[test]
fn weather_schema_hides_owm_fields_in_ha_mode() {
    let d = EngineRegistry::get_descriptor("weather").unwrap();
    let get = |id: &str| d.schema.fields.iter().find(|f| f.id == id).unwrap();
    assert_eq!(get("source").default_value, "owm");
    assert_eq!(get("topic").visible_when, Some("source=ha"));
    for id in ["api_key", "city", "units"] {
        assert_eq!(get(id).visible_when, Some("source=owm"), "{}", id);
    }
    assert_eq!(get("offset_x").visible_when, None);
}

#[test]
fn weather_engine_reads_ha_payload_from_store() {
    let dir = std::env::temp_dir().join(format!("am_weather_ha_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let config = Config::new(dir.join("config.json"));
    let topic = "test/ha_screens/weather_engine";
    let mut cfg = HashMap::new();
    cfg.insert("source".to_string(), "ha".to_string());
    cfg.insert("topic".to_string(), topic.to_string());
    let mut engine = WeatherEngine::new();
    let mut m = MockMatrix::new(128, 32);
    {
        let mut ctx = EngineContext {
            matrix: &mut m,
            config: &config,
        };
        engine
            .initialize(&mut ctx, &HashConfig { data: &cfg })
            .unwrap();
        engine.activate();
        engine.render(&mut ctx); // no payload yet -> "--°" notice, must not panic
    }
    publish_test(topic, WEATHER);
    m.clear();
    {
        let mut ctx = EngineContext {
            matrix: &mut m,
            config: &config,
        };
        engine.update(&mut ctx);
        engine.render(&mut ctx);
    }
    // Sun icon (yellow) drawn somewhere on the first slide.
    assert!(m
        .canvas
        .pixels()
        .any(|p| p[0] == 255 && p[1] == 255 && p[2] == 0));
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// Preview renders for layout-v2.md
// `HA_PREVIEW_DIR=out cargo test --test test_ha_screens -- --ignored`
// ---------------------------------------------------------------------------

fn save(m: &MockMatrix, dir: &str, name: &str) {
    let (w, h) = (m.canvas.width(), m.canvas.height());
    let big = image::imageops::resize(&m.canvas, w * 8, h * 8, image::imageops::Nearest);
    big.save(format!("{}/{}_{}x{}.png", dir, name, w, h))
        .unwrap();
}

#[test]
#[ignore]
fn render_v2_previews() {
    let Ok(dir) = std::env::var("HA_PREVIEW_DIR") else {
        return;
    };
    std::fs::create_dir_all(&dir).unwrap();

    // Synthetic downstairs-cooling card (lines, min/max, yaxis, band, mark, gaps).
    let n = 96;
    let wave = |base: f32, amp: f32, phase: f32| -> Vec<String> {
        (0..n)
            .map(|i| {
                let v = base + amp * ((i as f32 / 96.0 * 6.283) + phase).sin();
                format!("{:.1}", v)
            })
            .collect()
    };
    let mut out = wave(80.0, 9.0, -1.5);
    for v in out.iter_mut().skip(50).take(4) {
        *v = "null".into();
    }
    let t10 = wave(75.5, 1.2, 0.3);
    let set: Vec<String> = (0..n)
        .map(|i| {
            if (60..76).contains(&i) {
                "80".into()
            } else {
                "76".into()
            }
        })
        .collect();
    let cher = wave(76.5, 1.8, 1.0);
    let cool = format!(
        r##"{{"v":1,"title":"DN COOL","summary":"76°","unit":"F","slots":96,"min":68,"max":90,"yaxis":true,
         "series":[{{"label":"OUT","style":"line","color":"#FF4020","data":[{}]}},
                   {{"label":"T10","style":"line","color":"#2060FF","data":[{}]}},
                   {{"label":"SET","style":"line","color":"#5050A0","data":[{}]}},
                   {{"label":"CHERYLE","style":"line","color":"#FF9000","data":[{}]}}],
         "bands":[{{"from":60,"to":76,"color":"#502800"}}],
         "marks":[{{"at":19,"color":"#404040"}}]}}"##,
        out.join(","),
        t10.join(","),
        set.join(","),
        cher.join(",")
    );
    let net: Vec<String> = (0..48)
        .map(|i| format!("{:.2}", 2.5 * ((i as f32 / 48.0 * 6.283) - 1.0).sin() + 0.4))
        .collect();
    let net_temp: Vec<String> = (0..48)
        .map(|i| format!("{:.1}", 1.5 + 1.0 * (i as f32 / 48.0 * 6.283).cos()))
        .collect();
    let mixed = format!(
        r##"{{"title":"NET kWh","summary":"+3.1","yaxis":true,
         "series":[{{"label":"NET","color":"#40C0FF","data":[{}]}},
                   {{"label":"T","style":"line","color":"#FFFFFF","data":[{}]}}]}}"##,
        net.join(","),
        net_temp.join(",")
    );
    let title_case = format!(
        r##"{{"title":"DOWNSTAIRS COOL","title_short":"DOWN COOL","summary":"76°","min":68,"max":90,"yaxis":true,
         "series":[{{"label":"T10","style":"line","color":"#2060FF","data":[{}]}},
                   {{"label":"OUT","style":"line","color":"#FF4020","data":[{}]}}]}}"##,
        t10.join(","),
        out.join(",")
    );
    let graphs = [
        ("graph_cool_lines", cool),
        ("graph_neg_bars_mixed", mixed),
        ("graph_title_full_short", title_case),
    ];
    for (name, json) in graphs.iter() {
        let g = parse_graph_payload(json).expect(name);
        for (w, h) in [(128u32, 32u32), (256, 64)] {
            let mut m = MockMatrix::new(w, h);
            render_graph(&mut m, Some(&g), true, "NO DATA");
            save(&m, &dir, name);
        }
    }

    let values_cases = [
        ("values_4tiles", ENERGY.to_string()),
        ("values_home", HOME.to_string()),
        (
            "values_home_worst",
            r##"{"v":1,"title":"HOME","tiles":[
              {"label":"DOWNSTAIRS","label_short":"DOWN","value":"100","unit":"°F","color":"#FFFFFF"},
              {"label":"UPSTAIRS","label_short":"UP","value":"112","unit":"°F","color":"#FFFFFF"},
              {"label":"AC","label_short":"AC","value":"24.0","unit":"HOURS","color":"#40C0FF"},
              {"label":"OUTSIDE","label_short":"OUT","value":"112","unit":"°F","color":"#FF6040"}]}"##
                .to_string(),
        ),
        (
            "values_3tiles",
            r##"{"title":"ENERGY","tiles":[{"label":"HOUSE","value":"2.8","unit":"kW","color":"#FFB000"},
              {"label":"TODAY","value":"31.2","unit":"kWh"},{"label":"AC $","value":"0.35","color":"#40C0FF"}]}"##
                .to_string(),
        ),
        (
            "values_2tiles",
            r##"{"title":"ENERGY","tiles":[{"label":"HOUSE","value":"2.8","unit":"kW","color":"#FFB000"},
              {"label":"TODAY","value":"31.2","unit":"kWh"}]}"##
                .to_string(),
        ),
        (
            "values_1tile",
            r##"{"title":"POWER","tiles":[{"label":"HOUSE","value":"2.8","unit":"kW","color":"#FFB000"}]}"##
                .to_string(),
        ),
        (
            "values_1tile_notitle",
            r##"{"tiles":[{"label":"OUTSIDE","value":"75","unit":"°F","color":"#FF6040"}]}"##.to_string(),
        ),
        (
            "values_overflow",
            r##"{"title":"LONG","tiles":[{"label":"VERYLONGLABEL","value":"123456.78","unit":"kWh"},
              {"label":"B","value":"99999999","unit":"W"},{"label":"C","value":"-12.5","unit":"°C","color":"#40C0FF"},
              {"label":"D","value":"OK","unit":"","label_color":"#00FF00"}]}"##
                .to_string(),
        ),
    ];
    for (name, json) in values_cases.iter() {
        let d = parse_values_payload(json).expect(name);
        for (w, h) in [(128u32, 32u32), (256, 64)] {
            let mut m = MockMatrix::new(w, h);
            render_values(&mut m, Some(&d), true, "NO DATA");
            save(&m, &dir, name);
        }
    }
    for (w, h) in [(128u32, 32u32), (256, 64)] {
        let mut m = MockMatrix::new(w, h);
        render_values(&mut m, None, true, "NO DATA");
        save(&m, &dir, "values_nodata");
    }

    // Weather HA: first slide of each layout.
    let cfgdir = std::env::temp_dir().join(format!("am_weather_prev_{}", std::process::id()));
    std::fs::create_dir_all(&cfgdir).unwrap();
    let config = Config::new(cfgdir.join("config.json"));
    let topic = "test/ha_screens/weather_preview";
    publish_test(topic, WEATHER);
    let mut cfg = HashMap::new();
    cfg.insert("source".to_string(), "ha".to_string());
    cfg.insert("topic".to_string(), topic.to_string());
    for (w, h) in [(128u32, 32u32), (256, 64)] {
        let mut engine = WeatherEngine::new();
        let mut m = MockMatrix::new(w, h);
        let mut ctx = EngineContext {
            matrix: &mut m,
            config: &config,
        };
        engine
            .initialize(&mut ctx, &HashConfig { data: &cfg })
            .unwrap();
        engine.activate();
        engine.update(&mut ctx);
        engine.render(&mut ctx);
        save(&m, &dir, "weather_ha"); // first page = NOW
                                      // Second page (TODAY) after one 5 s dwell + 1 s slide.
        std::thread::sleep(std::time::Duration::from_millis(6300));
        let mut ctx = EngineContext {
            matrix: &mut m,
            config: &config,
        };
        ctx.matrix.clear();
        engine.update(&mut ctx);
        engine.render(&mut ctx);
        save(&m, &dir, "weather_ha_today");
    }
    let _ = std::fs::remove_dir_all(&cfgdir);
}

/// Renders whatever payloads are saved in `HA_LIVE_DIR` (graph_*.json,
/// values_*.json) next to them as PNGs.
#[test]
#[ignore]
fn render_live_payloads() {
    let Ok(dir) = std::env::var("HA_LIVE_DIR") else {
        return;
    };
    for entry in std::fs::read_dir(&dir).unwrap().flatten() {
        let path = entry.path();
        let name = path.file_stem().unwrap().to_string_lossy().to_string();
        if path.extension().map(|e| e != "json").unwrap_or(true) {
            continue;
        }
        let json = std::fs::read_to_string(&path).unwrap();
        for (w, h) in [(128u32, 32u32), (256, 64)] {
            let mut m = MockMatrix::new(w, h);
            if name.starts_with("graph") {
                render_graph(&mut m, parse_graph_payload(&json).as_ref(), true, "NO DATA");
            } else if name.starts_with("values") {
                render_values(
                    &mut m,
                    parse_values_payload(&json).as_ref(),
                    true,
                    "NO DATA",
                );
            } else {
                continue;
            }
            save(&m, &dir, &name);
        }
    }
}
