//! Pixel sprites for the Pac-Man clock face, ported verbatim from the ESP32 firmware's
//! PacmanSprites.h and MsPacSprites.h so both signs draw the same parade.
//!
//! One u16 row mask per line, MSB = leftmost column. Pac-Man frames are 13x13; ghosts are 14
//! wide (12-row body + 2-row skirt). EYES/PUPIL_* overlay the body from rows 3 / 5;
//! FRIGHT_FACE (eyes + wavy mouth of a frightened ghost) overlays from row 5.

pub const PAC_FRAME_CLOSED_ROWS: i32 = 13;
pub const PAC_FRAME_CLOSED_COLS: i32 = 13;
pub const PAC_FRAME_CLOSED: [u16; 13] = [
    0x01F0, 0x07FC, 0x0FFE, 0x0FFE, 0x1FFF, 0x1FFF, 0x1FFF, 0x1FFF, 0x1FFF, 0x0FFE, 0x0FFE, 0x07FC,
    0x01F0,
];

pub const PAC_FRAME_HALF_ROWS: i32 = 13;
pub const PAC_FRAME_HALF_COLS: i32 = 13;
pub const PAC_FRAME_HALF: [u16; 13] = [
    0x01F0, 0x07FC, 0x0FFE, 0x0FFC, 0x1FF8, 0x1FF0, 0x1FC0, 0x1FF0, 0x1FF8, 0x0FFC, 0x0FFE, 0x07FC,
    0x01F0,
];

pub const PAC_FRAME_OPEN_ROWS: i32 = 13;
pub const PAC_FRAME_OPEN_COLS: i32 = 13;
pub const PAC_FRAME_OPEN: [u16; 13] = [
    0x01F0, 0x07F8, 0x0FF0, 0x0FE0, 0x1FC0, 0x1F80, 0x1F00, 0x1F80, 0x1FC0, 0x0FE0, 0x0FF0, 0x07F8,
    0x01F0,
];

pub const GHOST_BODY_ROWS: i32 = 12;
pub const GHOST_BODY_COLS: i32 = 14;
pub const GHOST_BODY: [u16; 12] = [
    0x01E0, 0x07F8, 0x0FFC, 0x1FFE, 0x1FFE, 0x3FFF, 0x3FFF, 0x3FFF, 0x3FFF, 0x3FFF, 0x3FFF, 0x3FFF,
];

pub const SKIRT_A_ROWS: i32 = 2;
pub const SKIRT_A_COLS: i32 = 14;
pub const SKIRT_A: [u16; 2] = [0x39E7, 0x30C3];

pub const SKIRT_B_ROWS: i32 = 2;
pub const SKIRT_B_COLS: i32 = 14;
pub const SKIRT_B: [u16; 2] = [0x2F3D, 0x0618];

pub const EYES_ROWS: i32 = 5;
pub const EYES_COLS: i32 = 14;
pub const EYES: [u16; 5] = [0x0618, 0x0F3C, 0x0F3C, 0x0F3C, 0x0618];

pub const PUPIL_R_ROWS: i32 = 2;
pub const PUPIL_R_COLS: i32 = 14;
pub const PUPIL_R: [u16; 2] = [0x030C, 0x030C];

pub const PUPIL_L_ROWS: i32 = 2;
pub const PUPIL_L_COLS: i32 = 14;
pub const PUPIL_L: [u16; 2] = [0x0C30, 0x0C30];

pub const FRIGHT_FACE_ROWS: i32 = 5;
pub const FRIGHT_FACE_COLS: i32 = 14;
pub const FRIGHT_FACE: [u16; 5] = [0x0618, 0x0618, 0x0000, 0x1332, 0x2CCD];

/// Ms. Pac-Man, 13x13 to match the Pac-Man frames. Colour-indexed rather than a plain mask,
/// because she is not one colour: the body is yellow, the bow red with a lighter centre, the
/// eye dark and the lips pink at the mouth.
///   y body   r bow   p bow highlight   k eye   l lips   . transparent
pub const MSPAC_ROWS: i32 = 13;
pub const MSPAC_COLS: i32 = 13;
pub const MSPAC_CLOSED: [&str; 13] = [
    ".rr.rryy.....",
    ".rprpryyyy...",
    ".rryrryyyyy..",
    ".yyyyyykkyyy.",
    ".yyyyyykyyyy.",
    "yyyyyyyyyyyyy",
    "yyyyyyyyyyyyy",
    "yyyyyyyyyyyyy",
    ".yyyyyyyyyyy.",
    ".yyyyyyyyyyy.",
    "..yyyyyyyyy..",
    "...yyyyyyy...",
    ".....yyy.....",
];

pub const MSPAC_HALF: [&str; 13] = [
    ".rr.rryy.....",
    ".rprpryyyy...",
    ".rryrryyyyy..",
    ".yyyyyykkyll.",
    ".yyyyyykll...",
    "yyyyyyyy.....",
    "yyyyyy.......",
    "yyyyyyyy.....",
    ".yyyyyyyyy...",
    ".yyyyyyyyyyy.",
    "..yyyyyyyyy..",
    "...yyyyyyy...",
    ".....yyy.....",
];

pub const MSPAC_OPEN: [&str; 13] = [
    ".rr.rryy.....",
    ".rprpryyyy...",
    ".rryrryyyl...",
    ".yyyyyykk....",
    ".yyyyyyk.....",
    "yyyyyyy......",
    "yyyyyy.......",
    "yyyyyyy......",
    ".yyyyyyy.....",
    ".yyyyyyyy....",
    "..yyyyyyyy...",
    "...yyyyyyy...",
    ".....yyy.....",
];
