//! The weather page drawn like the ESP32 `drawForecast`.

use arcadematrix::api::DayForecast;
use arcadematrix::engines::renderers::weather_page::draw_weather_page;

const HIGH: [u8; 4] = [255, 150, 50, 255];
const LOW: [u8; 4] = [120, 200, 255, 255];
const LABEL: [u8; 4] = [180, 180, 255, 255];
const DESC: [u8; 4] = [210, 210, 210, 255];
const WHITE: [u8; 4] = [255, 255, 255, 255];

fn day(min: &str, max: &str, cond: &str, cond_long: &str) -> DayForecast {
    DayForecast {
        label: "TODAY".into(),
        label_long: "TODAY".into(),
        temp: max.into(),
        temp_min: min.into(),
        temp_max: max.into(),
        condition: cond.into(),
        condition_long: cond_long.into(),
        icon: "01d".into(),
        is_now: false,
        now_line2_short: String::new(),
    }
}

fn render(d: &DayForecast, w: u32, h: u32) -> image::RgbaImage {
    let mut img = image::RgbaImage::new(w, h);
    draw_weather_page(&mut img, d, 0, w, h, 0, 0);
    img
}

/// (x0, y0, x1, y1) of the pixels painted in colour `c`.
fn bbox(img: &image::RgbaImage, c: [u8; 4]) -> Option<(u32, u32, u32, u32)> {
    let mut b: Option<(u32, u32, u32, u32)> = None;
    for (x, y, p) in img.enumerate_pixels() {
        if p.0 == c {
            b = Some(match b {
                None => (x, y, x, y),
                Some((a, bb, cc, dd)) => (a.min(x), bb.min(y), cc.max(x), dd.max(y)),
            });
        }
    }
    b
}

#[test]
fn fahrenheit_256x64_three_columns() {
    let img = render(&day("70°F", "89°F", "Sunny", "Sunny"), 256, 64);
    let (_, hy0, hx1, hy1) = bbox(&img, HIGH).unwrap();
    assert_eq!((hx1, hy0, hy1), (247, 10, 23)); // high on top, right-aligned to W-8
    let (_, ly0, lx1, _) = bbox(&img, LOW).unwrap();
    assert_eq!((lx1, ly0), (247, 38));
    let (lab_x0, lab_y0, _, _) = bbox(&img, LABEL).unwrap();
    assert_eq!((lab_x0, lab_y0), (64, 10));
    assert_eq!(img.get_pixel(32, 32).0, [255, 255, 0, 255]); // sun at 2x
}

#[test]
fn fahrenheit_128x32_temps_right() {
    let img = render(&day("70°F", "89°F", "Pt cloudy", "Partly cloudy"), 128, 32);
    let (_, hy0, hx1, _) = bbox(&img, HIGH).unwrap();
    assert_eq!((hx1, hy0), (123, 4));
    let (_, ly0, lx1, _) = bbox(&img, LOW).unwrap();
    assert_eq!((lx1, ly0), (123, 18));
    let (lab_x0, lab_y0, _, _) = bbox(&img, LABEL).unwrap();
    assert_eq!((lab_x0, lab_y0), (32, 4));
    // "Partly cloudy" (77) doesn't fit 65 px -> "Pt cloudy" (53).
    let (cx0, _, cx1, _) = bbox(&img, DESC).unwrap();
    assert_eq!((cx0, cx1), (32, 84));
}

#[test]
fn celsius_keeps_low_on_top() {
    let img = render(&day("21°C", "31°C", "Sunny", "Sunny"), 128, 32);
    let (_, hy0, _, _) = bbox(&img, HIGH).unwrap();
    let (_, ly0, _, _) = bbox(&img, LOW).unwrap();
    assert!(ly0 < hy0);
}

#[test]
fn now_page_drops_wind_direction_when_too_wide() {
    let mut now = day("100°F", "100%  NW 100km/h", "", "");
    now.is_now = true;
    now.label = "NOW".into();
    now.label_long = String::new();
    now.now_line2_short = "100%  100km/h".into();
    let img = render(&now, 128, 32);
    let (wx0, wy0, wx1, _) = bbox(&img, WHITE).unwrap();
    assert_eq!((wy0, wx1), (4, 123));
    let (gx0, gy0, gx1, _) = bbox(&img, DESC).unwrap();
    // "100%  100km/h" (77 px) right-aligned: x 47..123 ('1' has a blank first column).
    assert_eq!((gx0, gy0, gx1), (48, 18, 123));
    let (lx0, _, lx1, _) = bbox(&img, LABEL).unwrap();
    assert!(lx0 == 32 && lx1 < wx0);
}
