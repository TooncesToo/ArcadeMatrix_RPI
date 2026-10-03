//! Mario clock, adapted from the 64x64 "Clockwise" clockface cw-cf-0x01.
//!
//! The square scene keeps its place in the middle of the panel and the ground and clouds carry on
//! to both edges, so a wide sign reads as more of the same level. The hill and the bush are each cut
//! down one side to sit against a frame edge, so there is one of each, at the edge it was drawn for.
//! Once a minute Mario runs in from the left, jumps to bump a block, which flips its digits, and
//! runs off right.
//! Laid out for 256x64; smaller panels get the notice instead. Matches the ESP32 face.

use crate::core::matrix::MatrixBackend;
use crate::engines::clocks::cwassets::mario::{
    BLOCK, BUSH, CLOUD1, CLOUD2, GROUND, HILL, MARIO_IDLE, MARIO_JUMP, SKY_COLOR,
};
use crate::engines::clocks::cwscene;
use crate::engines::renderers::base_renderer::ArcadeFont;
use crate::engines::renderers::BaseRenderer;
use std::time::Instant;

const BLOCK_W: i32 = 19;
const GROUND_W: i32 = 8;
const GROUND_H: i32 = 8;
const HILL_W: i32 = 20;
const HILL_H: i32 = 22;
const BUSH_W: i32 = 21;
const BUSH_H: i32 = 9;
const CLOUD_W: i32 = 13;
const CLOUD_H: i32 = 12;
const MARIO_W: i32 = 13;
const MARIO_H: i32 = 16;
const MARIO_JUMP_W: i32 = 17;
const SCENE: i32 = 64;

#[derive(PartialEq, Clone, Copy)]
enum Phase {
    Waiting,
    RunIn,
    Jump,
    RunOut,
}

pub struct MarioClock {
    phase: Phase,
    runner_x: f32,
    jump_t: f32,
    jump_target: usize,
    block_bounce: [f32; 2],
    pending_digits: bool,
    shown: [String; 2],
    last_minute: u32,
    last_frame: Instant,
}

impl MarioClock {
    pub fn new() -> Self {
        Self {
            phase: Phase::Waiting,
            runner_x: -(MARIO_JUMP_W as f32),
            jump_t: 0.0,
            jump_target: 1,
            block_bounce: [0.0, 0.0],
            pending_digits: false,
            shown: [String::from("--"), String::from("--")],
            last_minute: 99,
            last_frame: Instant::now(),
        }
    }

    fn blit(
        matrix: &mut dyn MatrixBackend,
        data: &[u16],
        w: i32,
        h: i32,
        x: i32,
        y: i32,
        transparent: bool,
    ) {
        for row in 0..h {
            for col in 0..w {
                let c = data[(row * w + col) as usize];
                if transparent && c == SKY_COLOR {
                    continue;
                }
                cwscene::put(matrix, x + col, y + row, c);
            }
        }
    }

    fn draw_scene(matrix: &mut dyn MatrixBackend, w: i32, h: i32) {
        for y in 0..h {
            for x in 0..w {
                cwscene::put(matrix, x, y, SKY_COLOR);
            }
        }
        let scene_left = (w - SCENE) / 2;
        let ground_top = h - GROUND_H;
        let mut x = 0;
        while x < w {
            Self::blit(matrix, &GROUND, GROUND_W, GROUND_H, x, ground_top, false);
            x += GROUND_W;
        }
        // The hill is half a hill: its left side is a sheer vertical cut, drawn to sit flush against
        // the frame edge so it reads as a slope running on past it. Tiled across a wide panel that
        // cut lands in open sky and looks like a hill sliced off, so there is one, against the left
        // edge, as in the original.
        Self::blit(
            matrix,
            &HILL,
            HILL_W,
            HILL_H,
            0,
            ground_top - HILL_H + 2,
            true,
        );

        // The bush is the hill's mirror: cut down its right side, drawn to sit flush against the
        // other frame edge (x 43 of 64, so its right edge lands exactly on it). One, on the right.
        Self::blit(
            matrix,
            &BUSH,
            BUSH_W,
            BUSH_H,
            w - BUSH_W,
            ground_top - BUSH_H,
            true,
        );
        let mut cx = scene_left % 64 - 64;
        while cx < w {
            Self::blit(matrix, &CLOUD1, CLOUD_W, CLOUD_H, cx, 8, true);
            Self::blit(matrix, &CLOUD2, CLOUD_W, CLOUD_H, cx + 25, 2, true);
            cx += 51;
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        matrix: &mut dyn MatrixBackend,
        hours: u32,
        minutes: u32,
        font: &ArcadeFont<'_>,
        scale: u32,
    ) {
        let w = matrix.width() as i32;
        let h = matrix.height() as i32;
        if w < 192 || h < 64 {
            super::wide_only_notice(matrix, font, scale);
            return;
        }

        let dt = self.last_frame.elapsed().as_secs_f32().min(0.2);
        self.last_frame = Instant::now();

        let scene_left = (w - SCENE) / 2;
        let ground_top = h - GROUND_H;
        let hour_x = scene_left + 13;
        let minute_x = scene_left + 32;
        let block_y = 8;

        let hh = format!("{}", hours);
        let mm = format!("{:02}", minutes);
        if self.shown[0] == "--" {
            self.shown = [hh.clone(), mm.clone()];
        }

        if minutes != self.last_minute {
            let new_hour = self.last_minute != 99 && minutes == 0;
            self.last_minute = minutes;
            self.jump_target = if new_hour { 0 } else { 1 };
            self.pending_digits = true;
            if self.phase == Phase::Waiting {
                self.phase = Phase::RunIn;
                self.runner_x = -(MARIO_JUMP_W as f32);
            }
        }

        let target_x = (if self.jump_target == 0 {
            hour_x
        } else {
            minute_x
        }) + BLOCK_W / 2
            - MARIO_W / 2;
        let pace = 34.0;
        match self.phase {
            Phase::Waiting => {}
            Phase::RunIn => {
                self.runner_x += pace * dt;
                if self.runner_x >= target_x as f32 {
                    self.runner_x = target_x as f32;
                    self.phase = Phase::Jump;
                    self.jump_t = 0.0;
                }
            }
            Phase::Jump => {
                self.jump_t += dt * 1.6;
                if self.jump_t >= 0.5 && self.pending_digits {
                    self.pending_digits = false;
                    self.block_bounce[self.jump_target] = 0.001;
                    self.shown = [hh.clone(), mm.clone()];
                }
                if self.jump_t >= 1.0 {
                    self.jump_t = 0.0;
                    self.phase = Phase::RunOut;
                }
            }
            Phase::RunOut => {
                self.runner_x += pace * dt;
                if self.runner_x > w as f32 {
                    self.phase = Phase::Waiting;
                }
            }
        }
        for b in self.block_bounce.iter_mut() {
            if *b > 0.0 {
                *b += dt * 3.2;
                if *b >= 1.0 {
                    *b = 0.0;
                }
            }
        }

        Self::draw_scene(matrix, w, h);

        for i in 0..2 {
            let b = self.block_bounce[i];
            let lift = if b > 0.0 {
                (std::f32::consts::PI * b).sin() * 4.0
            } else {
                0.0
            } as i32;
            let bx = if i == 0 { hour_x } else { minute_x };
            Self::blit(matrix, &BLOCK, BLOCK_W, BLOCK_W, bx, block_y - lift, false);
            let text = &self.shown[i];
            let offset = if text.chars().count() == 1 { 6 } else { 2 };
            BaseRenderer::draw_text_at(
                matrix,
                text,
                font,
                scale.max(1) as f32,
                bx + offset,
                block_y - lift + 4,
                (0, 0, 0),
                (0, 0, 0),
            );
        }

        if self.phase != Phase::Waiting {
            let airborne = self.phase == Phase::Jump;
            let lift = if airborne {
                ((std::f32::consts::PI * self.jump_t).sin()
                    * (ground_top - block_y - BLOCK_W - 2) as f32) as i32
            } else {
                0
            };
            let y = ground_top - MARIO_H - lift;
            if airborne {
                Self::blit(
                    matrix,
                    &MARIO_JUMP,
                    MARIO_JUMP_W,
                    MARIO_H,
                    self.runner_x as i32,
                    y,
                    true,
                );
            } else {
                Self::blit(
                    matrix,
                    &MARIO_IDLE,
                    MARIO_W,
                    MARIO_H,
                    self.runner_x as i32,
                    y,
                    true,
                );
            }
        }
    }
}

impl Default for MarioClock {
    fn default() -> Self {
        Self::new()
    }
}
