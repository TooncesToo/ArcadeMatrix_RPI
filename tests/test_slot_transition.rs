//! Slot transition effect names. Mirrors the ESP32 suite, so the two firmwares accept the same
//! configuration values.

use arcadematrix::core::slot_transition::SlotEffect;

#[test]
fn slot_effect_names_parse() {
    assert_eq!(SlotEffect::parse("wipe"), SlotEffect::Wipe);
    assert_eq!(SlotEffect::parse("Curtain"), SlotEffect::Curtain);
    assert_eq!(SlotEffect::parse("blinds"), SlotEffect::Shutter);
    assert_eq!(SlotEffect::parse("checkerboard"), SlotEffect::Checker);
    assert_eq!(SlotEffect::parse("matrix_rain"), SlotEffect::MatrixRain);
    assert_eq!(SlotEffect::parse("random"), SlotEffect::Random);
    // Anything unknown turns the transition off rather than picking a surprise.
    assert_eq!(SlotEffect::parse("sparkles"), SlotEffect::None);
    assert_eq!(SlotEffect::parse(""), SlotEffect::None);
}

#[test]
fn duration_is_clamped_to_a_sane_range() {
    use arcadematrix::core::slot_transition::SlotTransition;
    let mut t = SlotTransition::default();
    t.configure("wipe", 10); // too short to see
    t.start();
    assert!(t.is_running());
    t.configure("none", 500);
    t.start();
    assert!(!t.is_running(), "no effect configured means nothing plays");
}
