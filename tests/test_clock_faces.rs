//! Clock face behaviour that can be checked without a panel: the wording and the date order behind
//! the words face. Mirrors the ESP32 suite, so both firmwares are held to the same wording.

use arcadematrix::engines::clocks::words_clock::{date_line, spoken_time};

#[test]
fn spoken_time_reads_as_words() {
    let (h, m) = spoken_time(12, 0, "en");
    assert_eq!(h, "noon");
    assert_eq!(m, "");

    let (h, _) = spoken_time(0, 0, "en");
    assert_eq!(h, "midnight");

    let (h, m) = spoken_time(8, 5, "en");
    assert_eq!(h, "eight");
    assert_eq!(m, "oh five");

    let (_, m) = spoken_time(8, 30, "en");
    assert_eq!(m, "a half");

    // Each language gets its own phrasing rather than a word-for-word translation.
    let (h, m) = spoken_time(8, 30, "fr");
    assert_eq!(h, "huit");
    assert_eq!(m, "heures et demie");

    let (h, m) = spoken_time(8, 30, "es");
    assert_eq!(h, "ocho");
    assert_eq!(m, "y media");

    let (_, m) = spoken_time(8, 21, "fr");
    assert_eq!(m, "heures vingt et une");

    let (_, m) = spoken_time(8, 21, "es");
    assert_eq!(m, "y veintiuno");
}

#[test]
fn date_line_follows_the_language() {
    // Month before the day in English, day before the month elsewhere.
    assert_eq!(date_line(1, 8, 28, "en"), "MON SEP 28");
    assert_eq!(date_line(1, 8, 28, "fr"), "LUN 28 SEPT");
    assert_eq!(date_line(1, 8, 28, "es"), "LUN 28 SEP");
}

#[test]
fn scene_places_a_square_face_in_the_middle() {
    use arcadematrix::engines::clocks::cwscene;
    // On a wide panel the 64 px scene keeps its size and sits centred.
    assert_eq!(cwscene::divisor(64), 1);
    assert_eq!(cwscene::origin_x(256, 64), 96);
    assert_eq!(cwscene::map_x(32, 256, 64), 128);
    // On a short one it is halved so the whole picture still fits.
    assert_eq!(cwscene::divisor(32), 2);
    assert_eq!(cwscene::scaled_size(32), 32);
    assert_eq!(cwscene::origin_x(128, 32), 48);
}
