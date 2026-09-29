//! Clock tower face, adapted from the 64x64 "Clockwise" clockface cw-cf-0x04.
//!
//! The tower sits in the middle of the panel at its own size with its edge columns carried out to
//! the sides, and the two hands sweep from the tower's clock face as they do in the original.
//! Matches the ESP32 firmware's face of the same name.

use crate::core::matrix::MatrixBackend;
use crate::engines::clocks::cwassets::castle::CLOCK_TOWER;
use crate::engines::clocks::cwscene;

const HAND_LEN: i32 = 10;
const PIVOT_X: i32 = 32;
const PIVOT_Y: i32 = 28;
const HAND_COLOR: u16 = 0xB58C;
const HOUR_OFFSET: f32 = -30.0;
const MIN_OFFSET: f32 = -6.0;

pub struct CastleClock;

impl CastleClock {
    pub fn new() -> Self {
        Self
    }

    pub fn render(&mut self, matrix: &mut dyn MatrixBackend, hours: u32, minutes: u32) {
        let w = matrix.width() as i32;
        let h = matrix.height() as i32;

        cwscene::draw_background(matrix, &CLOCK_TOWER, w, h);

        let extra = (HOUR_OFFSET * minutes as f32) / 60.0;
        let hour_angle = ((hours as f32 * HOUR_OFFSET) + 180.0 + extra).to_radians();
        let min_angle = ((minutes as f32 * MIN_OFFSET) + 180.0).to_radians();
        self.draw_hand(matrix, min_angle, HAND_LEN, w, h);
        self.draw_hand(matrix, hour_angle, HAND_LEN - 3, w, h);
    }

    fn draw_hand(&self, matrix: &mut dyn MatrixBackend, angle: f32, len: i32, w: i32, h: i32) {
        let d = cwscene::divisor(h);
        let x0 = cwscene::map_x(PIVOT_X, w, h);
        let y0 = cwscene::map_y(PIVOT_Y, h);
        let x1 = x0 + (angle.sin() * len as f32) as i32 / d;
        let y1 = y0 + (angle.cos() * len as f32) as i32 / d;
        // Straight line from the pivot, drawn a pixel at a time so it clips with the scene.
        let steps = (x1 - x0).abs().max((y1 - y0).abs()).max(1);
        for i in 0..=steps {
            let x = x0 + (x1 - x0) * i / steps;
            let y = y0 + (y1 - y0) * i / steps;
            cwscene::put(matrix, x, y, HAND_COLOR);
        }
    }
}

impl Default for CastleClock {
    fn default() -> Self {
        Self::new()
    }
}
