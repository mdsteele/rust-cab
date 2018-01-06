// ========================================================================= //

pub const FILE_SIGNATURE: u32 = 0x4643534d; // "MSCF" stored little-endian

pub const VERSION_MAJOR: u8 = 1;
pub const VERSION_MINOR: u8 = 3;

pub const MAX_HEADER_RESERVE_SIZE: usize = 60_000;
pub const MAX_FOLDER_RESERVE_SIZE: usize = 255;

// Header flags:
pub const FLAG_PREV_CABINET: u16 = 0x1;
pub const FLAG_NEXT_CABINET: u16 = 0x2;
pub const FLAG_RESERVE_PRESENT: u16 = 0x4;

// File attributes:
pub const ATTR_READ_ONLY: u16 = 0x01;
pub const ATTR_HIDDEN: u16 = 0x02;
pub const ATTR_SYSTEM: u16 = 0x04;

// ========================================================================= //
