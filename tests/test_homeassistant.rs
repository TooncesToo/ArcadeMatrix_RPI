//! Home Assistant engine: type detection, topics, notices, page list and the
//! activate / deactivate lifecycle (nothing may stay resident).

use arcadematrix::core::config::{Config, ConfigSettings, DataMqttConfig};
use arcadematrix::core::engine_contract::{Engine, EngineContext, HashConfig};
use arcadematrix::core::i18n::Lang;
use arcadematrix::core::matrix::{MatrixBackend, MockMatrix};
use arcadematrix::core::registry::EngineRegistry;
use arcadematrix::engines::homeassistant::payload::{parse_page, PageData};
use arcadematrix::engines::homeassistant::{
    build_pages, data_client_id, draw_notice, notice_for, parse_topics, HomeAssistantEngine,
    Notice, PageRef, NO_DATA_AFTER, STATE_CONNECTED, STATE_CONNECTING, STATE_FAILED,
};
use std::collections::HashMap;
use std::time::{Duration, Instant};

fn px(m: &MockMatrix, x: u32, y: u32) -> (u8, u8, u8) {
    let p = m.canvas.get_pixel(x, y);
    (p[0], p[1], p[2])
}

fn bbox(m: &MockMatrix, c: (u8, u8, u8)) -> Option<(u32, u32, u32, u32)> {
    let mut b: Option<(u32, u32, u32, u32)> = None;
    for y in 0..m.canvas.height() {
        for x in 0..m.canvas.width() {
            if px(m, x, y) == c {
                b = Some(match b {
                    None => (x, y, x, y),
                    Some((a, bb, cc, d)) => (a.min(x), bb.min(y), cc.max(x), d.max(y)),
                });
            }
        }
    }
    b
}

// ---------------------------------------------------------------------------
// Payload type
// ---------------------------------------------------------------------------

#[test]
fn explicit_type_wins_and_shape_is_the_fallback() {
    let graph = r#"{"series":[{"data":[1]}]}"#;
    let values = r#"{"tiles":[{"value":"1"}]}"#;
    let weather = r#"{"current":{"temp":70}}"#;
    assert!(matches!(parse_page(graph), Some(PageData::Graph(_))));
    assert!(matches!(parse_page(values), Some(PageData::Values(_))));
    assert!(matches!(parse_page(weather), Some(PageData::Weather(_))));
    // Explicit type, including case/spacing.
    assert!(matches!(
        parse_page(r#"{"type":"graph","series":[{"data":[1]}]}"#),
        Some(PageData::Graph(_))
    ));
    assert!(matches!(
        parse_page(r#"{"type":" Weather ","days":[{"temp_max":1}]}"#),
        Some(PageData::Weather(_))
    ));
    // The type wins over the shape: a values payload that also has "series".
    assert!(matches!(
        parse_page(r#"{"type":"values","series":[],"tiles":[{"value":"1"}]}"#),
        Some(PageData::Values(_))
    ));
    // Declared type without its fields -> nothing, not a guess.
    assert!(parse_page(r#"{"type":"graph","tiles":[{"value":"1"}]}"#).is_none());
    // Unknown type falls back to the shape.
    assert!(matches!(
        parse_page(r#"{"type":"future","tiles":[{"value":"1"}]}"#),
        Some(PageData::Values(_))
    ));
    assert!(parse_page("").is_none());
    assert!(parse_page("not json").is_none());
    assert!(parse_page(r#"{"hello":1}"#).is_none());
}

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

#[test]
fn topics_setting_is_split_and_capped() {
    assert_eq!(
        parse_topics(" a/one, a/two ;a/three\na/four,,"),
        vec!["a/one", "a/two", "a/three", "a/four"]
    );
    let many: Vec<String> = (0..9).map(|i| format!("t/{}", i)).collect();
    assert_eq!(parse_topics(&many.join(",")).len(), 6);
    assert!(parse_topics("").is_empty());
}

#[test]
fn data_mqtt_is_credentials_only() {
    let d = DataMqttConfig::default();
    assert_eq!((d.broker.as_str(), d.port), ("", 1883));
    // Old keys (enabled / topic_prefix) from the data-feed design are ignored.
    let s: ConfigSettings = serde_json::from_str(
        r#"{"data_mqtt":{"enabled":true,"broker":"10.0.0.2","port":1884,"user":"u","pass":"p","topic_prefix":"x"}}"#,
    )
    .unwrap();
    assert_eq!(s.data_mqtt.broker, "10.0.0.2");
    assert_eq!(s.data_mqtt.port, 1884);
    let out = serde_json::to_value(&s).unwrap();
    assert!(out["data_mqtt"].get("enabled").is_none());
    assert!(out["data_mqtt"].get("topic_prefix").is_none());
    assert_eq!(data_client_id("clock1", "x"), "clock1-data");
    assert_eq!(data_client_id("", ""), "arcadematrix-data");
}

#[test]
fn engine_is_registered_with_its_schema() {
    let d = EngineRegistry::get_descriptor("homeassistant").expect("descriptor");
    assert_eq!(d.metadata.name, "Home Assistant");
    let ids: Vec<&str> = d.schema.fields.iter().map(|f| f.id).collect();
    assert_eq!(ids, vec!["topics", "page_seconds", "show_header"]);
    assert_eq!(d.schema.fields[1].default_value, "10");
    assert_eq!(d.schema.fields[1].min_val, Some("3"));
    assert_eq!(EngineRegistry::engine_name_to_id("homeassistant"), 16);
    // The data-feed engines are gone, and weather is OpenWeatherMap only.
    assert!(EngineRegistry::get_descriptor("graph").is_none());
    assert!(EngineRegistry::get_descriptor("values").is_none());
    let w = EngineRegistry::get_descriptor("weather").unwrap();
    assert!(w
        .schema
        .fields
        .iter()
        .all(|f| f.id != "source" && f.id != "topic"));
}

// ---------------------------------------------------------------------------
// Notices and pages
// ---------------------------------------------------------------------------

#[test]
fn notice_follows_connection_state() {
    assert_eq!(notice_for(STATE_CONNECTING, None), Notice::Connecting);
    assert_eq!(notice_for(STATE_FAILED, None), Notice::NoConnection);
    assert_eq!(
        notice_for(STATE_CONNECTED, Some(Duration::from_millis(500))),
        Notice::Connecting
    );
    assert_eq!(
        notice_for(STATE_CONNECTED, Some(NO_DATA_AFTER)),
        Notice::NoData
    );
    // Reconnecting after a drop with data pending: NO DATA, not NO CONNECTION.
    assert_eq!(
        notice_for(STATE_CONNECTING, Some(Duration::from_secs(10))),
        Notice::NoData
    );
}

#[test]
fn notices_are_centred_on_128x32() {
    let grey = (0x78, 0x78, 0x78);
    let red = (0xB4, 0x50, 0x50);
    let mut m = MockMatrix::new(128, 32);
    draw_notice(&mut m, Notice::Connecting, Lang::En);
    let (x0, y0, x1, y1) = bbox(&m, grey).unwrap();
    // "CONNECTING" = 59 px at x=34, rows 12..18.
    assert_eq!((x0, x1, y0, y1), (34, 34 + 58, 12, 18));
    draw_notice(&mut m, Notice::NoConnection, Lang::En);
    let (x0, _, x1, _) = bbox(&m, red).unwrap();
    assert_eq!((x0, x1), (25, 25 + 76));
    draw_notice(&mut m, Notice::NoConnection, Lang::Fr);
    let (x0, _, x1, _) = bbox(&m, red).unwrap();
    assert_eq!((x0, x1), (16, 16 + 94));
}

#[test]
fn weather_topic_expands_to_its_slides() {
    use arcadematrix::api::DayForecast;
    let slide = |l: &str| DayForecast {
        label: l.into(),
        label_long: String::new(),
        temp: String::new(),
        temp_min: String::new(),
        temp_max: String::new(),
        condition: String::new(),
        condition_long: String::new(),
        icon: String::new(),
        is_now: false,
        now_line2_short: String::new(),
    };
    let weather = vec![
        vec![slide("NOW"), slide("TODAY"), slide("TOM.")],
        vec![],
        vec![],
    ];
    let pages = build_pages(&weather, 3);
    assert_eq!(
        pages,
        vec![
            PageRef { topic: 0, sub: 0 },
            PageRef { topic: 0, sub: 1 },
            PageRef { topic: 0, sub: 2 },
            PageRef { topic: 1, sub: 0 },
            PageRef { topic: 2, sub: 0 },
        ]
    );
    // Topics without data still get one page (it shows a notice).
    assert_eq!(build_pages(&[], 2).len(), 2);
}

// ---------------------------------------------------------------------------
// Lifecycle
// ---------------------------------------------------------------------------

fn test_config(broker: &str) -> (Config, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "am_ha_{}_{}",
        std::process::id(),
        broker.replace([':', '.'], "_")
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let config = Config::new(dir.join("config.json"));
    config.settings.write().data_mqtt.broker = broker.to_string();
    config.settings.write().system.lang = "en".to_string();
    (config, dir)
}

fn instance(topics: &str, page_seconds: &str) -> HashMap<String, String> {
    let mut cfg = HashMap::new();
    cfg.insert("topics".to_string(), topics.to_string());
    cfg.insert("page_seconds".to_string(), page_seconds.to_string());
    cfg.insert("show_header".to_string(), "true".to_string());
    cfg
}

#[test]
fn no_broker_shows_no_connection_and_starts_no_thread() {
    let (config, dir) = test_config("");
    let cfg = instance("a/b", "10");
    let mut engine = HomeAssistantEngine::new();
    let mut m = MockMatrix::new(128, 32);
    let mut ctx = EngineContext {
        matrix: &mut m,
        config: &config,
    };
    engine
        .initialize(&mut ctx, &HashConfig { data: &cfg })
        .unwrap();
    assert!(!engine.has_session(), "nothing allocated before activation");
    engine.activate();
    engine.update(&mut ctx);
    assert!(engine.has_session());
    engine.render(&mut ctx);
    assert!(bbox(&m, (0xB4, 0x50, 0x50)).is_some(), "NO CONNECTION");
    let mut ctx = EngineContext {
        matrix: &mut m,
        config: &config,
    };
    engine.deactivate();
    assert!(!engine.has_session());
    engine.render(&mut ctx); // must not panic after deactivate
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn twenty_activations_free_everything() {
    // A broker that refuses connections: the thread starts, fails, and must
    // still stop and drop all its state on every deactivate.
    let (config, dir) = test_config("127.0.0.1");
    config.settings.write().data_mqtt.port = 1; // nothing listens on port 1
    let cfg = instance("a/one,a/two", "10");
    let mut engine = HomeAssistantEngine::new();
    let mut m = MockMatrix::new(128, 32);
    {
        let mut ctx = EngineContext {
            matrix: &mut m,
            config: &config,
        };
        engine
            .initialize(&mut ctx, &HashConfig { data: &cfg })
            .unwrap();
    }
    for _ in 0..20 {
        engine.activate();
        {
            let mut ctx = EngineContext {
                matrix: &mut m,
                config: &config,
            };
            engine.update(&mut ctx);
            engine.render(&mut ctx);
        }
        let shared = engine.shared().expect("session");
        let weak = std::sync::Arc::downgrade(&shared);
        drop(shared);
        let t = Instant::now();
        engine.deactivate();
        assert!(
            t.elapsed() < Duration::from_secs(2),
            "deactivate must not hang"
        );
        assert!(weak.upgrade().is_none(), "thread and buffers released");
        assert!(!engine.has_session());
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn pages_cycle_and_render_injected_data() {
    let (config, dir) = test_config("");
    let cfg = instance("a/graph,a/values", "3");
    let mut engine = HomeAssistantEngine::new();
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
    let shared = engine.shared().unwrap();
    shared.set_page(
        0,
        parse_page(r##"{"type":"graph","max":1,"series":[{"color":"#FF0000","data":[1]}]}"##),
    );
    shared.set_page(
        1,
        parse_page(r##"{"type":"values","tiles":[{"label":"A","value":"7","color":"#00FF00"}]}"##),
    );
    engine.update(&mut ctx);
    assert_eq!(engine.current_page(), Some(PageRef { topic: 0, sub: 0 }));
    ctx.matrix.clear();
    engine.render(&mut ctx);
    drop(ctx);
    assert_eq!(px(&m, 64, 31), (255, 0, 0), "graph page drawn");
    std::thread::sleep(Duration::from_millis(3100));
    let mut ctx = EngineContext {
        matrix: &mut m,
        config: &config,
    };
    engine.update(&mut ctx);
    assert_eq!(engine.current_page(), Some(PageRef { topic: 1, sub: 0 }));
    ctx.matrix.clear();
    engine.render(&mut ctx);
    drop(ctx);
    assert!(bbox(&m, (0, 255, 0)).is_some(), "values page drawn");
    // An empty payload (retained message deleted) clears the page.
    shared.set_page(1, parse_page(""));
    let mut ctx = EngineContext {
        matrix: &mut m,
        config: &config,
    };
    engine.update(&mut ctx);
    engine.render(&mut ctx);
    drop(ctx);
    assert!(bbox(&m, (0, 255, 0)).is_none());
    drop(shared);
    engine.deactivate();
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// Preview renders: `HA_PREVIEW_DIR=out cargo test --test test_homeassistant -- --ignored`
// ---------------------------------------------------------------------------

#[test]
#[ignore]
fn render_notice_previews() {
    let Ok(dir) = std::env::var("HA_PREVIEW_DIR") else {
        return;
    };
    std::fs::create_dir_all(&dir).unwrap();
    for (name, notice) in [
        ("ha_connecting", Notice::Connecting),
        ("ha_no_connection", Notice::NoConnection),
        ("ha_no_data", Notice::NoData),
    ] {
        for (w, h) in [(128u32, 32u32), (256, 64)] {
            let mut m = MockMatrix::new(w, h);
            draw_notice(&mut m, notice, Lang::En);
            let big = image::imageops::resize(&m.canvas, w * 8, h * 8, image::imageops::Nearest);
            big.save(format!("{}/{}_{}x{}.png", dir, name, w, h))
                .unwrap();
        }
    }
}

/// Runs the engine against a real broker and saves every page it shows:
/// `HA_BROKER=host HA_USER=u HA_PASS=p HA_TOPICS=a,b HA_PREVIEW_DIR=out
///  cargo test --test test_homeassistant live_broker -- --ignored`
#[test]
#[ignore]
fn live_broker_pages() {
    let (Ok(broker), Ok(dir)) = (std::env::var("HA_BROKER"), std::env::var("HA_PREVIEW_DIR"))
    else {
        return;
    };
    std::fs::create_dir_all(&dir).unwrap();
    let topics = std::env::var("HA_TOPICS").unwrap_or_default();
    for (w, h) in [(128u32, 32u32), (256, 64)] {
        let (config, tmp) = test_config(&broker);
        {
            let mut s = config.settings.write();
            s.data_mqtt.user = std::env::var("HA_USER").unwrap_or_default();
            s.data_mqtt.pass = std::env::var("HA_PASS").unwrap_or_default();
            s.wifi.hostname = "arcadematrix-mactest".into();
        }
        let cfg = instance(&topics, "3");
        let mut engine = HomeAssistantEngine::new();
        let mut m = MockMatrix::new(w, h);
        let t0 = Instant::now();
        {
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
        }
        let save = |m: &MockMatrix, name: &str| {
            let big = image::imageops::resize(&m.canvas, w * 8, h * 8, image::imageops::Nearest);
            big.save(format!("{}/{}_{}x{}.png", dir, name, w, h))
                .unwrap();
        };
        save(&m, "live_00_first_frame");
        // Wait for the retained snapshot, then step through every page.
        std::thread::sleep(Duration::from_millis(1500));
        let mut seen = Vec::new();
        for _ in 0..80 {
            let mut ctx = EngineContext {
                matrix: &mut m,
                config: &config,
            };
            engine.update(&mut ctx);
            let page = engine.current_page();
            ctx.matrix.clear();
            engine.render(&mut ctx);
            drop(ctx);
            if let Some(p) = page {
                if !seen.contains(&p) {
                    seen.push(p);
                    save(&m, &format!("live_t{}_p{}", p.topic, p.sub));
                }
                if seen.len() > 1 && p == seen[0] {
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        eprintln!("{}x{}: {} page(s) in {:?}", w, h, seen.len(), t0.elapsed());
        engine.deactivate();
        assert!(!engine.has_session());
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
