pub const FILE_SIGNATURE: u32 = 0x4643534d; // "MSCF" stored little-endian

pub const VERSION_MAJOR: u8 = 1;
pub const VERSION_MINOR: u8 = 3;

pub const MAX_TOTAL_CAB_SIZE: u32 = 0x7fffffff;
pub const MAX_HEADER_RESERVE_SIZE: usize = 60_000;
pub const MAX_FOLDER_RESERVE_SIZE: usize = 255;
pub const MAX_STRING_SIZE: usize = 255;
pub const MAX_NUM_FILES: usize = 0xffff;
pub const MAX_NUM_FOLDERS: usize = 0xffff;
pub const MAX_FILE_SIZE: u32 = 0x7fff8000;

// Header flags:
pub const FLAG_PREV_CABINET: u16 = 0x1;
pub const FLAG_NEXT_CABINET: u16 = 0x2;
pub const FLAG_RESERVE_PRESENT: u16 = 0x4;

// File attributes:
pub const ATTR_READ_ONLY: u16 = 0x01;
pub const ATTR_HIDDEN: u16 = 0x02;
pub const ATTR_SYSTEM: u16 = 0x04;
pub const ATTR_ARCH: u16 = 0x20;
pub const ATTR_EXEC: u16 = 0x40;
pub const ATTR_NAME_IS_UTF: u16 = 0x80;
