use std::io;

// ========================================================================= //

const CTYPE_NONE: u16 = 0;
const CTYPE_MSZIP: u16 = 1;
const CTYPE_QUANTUM: u16 = 2;
const CTYPE_LZX: u16 = 3;

const QUANTUM_LEVEL_MIN: u16 = 1;
const QUANTUM_LEVEL_MAX: u16 = 7;
const QUANTUM_MEMORY_MIN: u16 = 10;
const QUANTUM_MEMORY_MAX: u16 = 21;

const LZX_WINDOW_MIN: u16 = 15;
const LZX_WINDOW_MAX: u16 = 21;

// ========================================================================= //

/// A scheme for compressing data within the cabinet.
#[derive(Clone, Copy, Debug, Hash, Eq, PartialEq)]
pub enum CompressionType {
    /// No compression.
    None,
    /// MSZIP compression.  MSZIP is described further in
    /// [MS-MCI](https://msdn.microsoft.com/en-us/library/cc483131.aspx).
    MsZip,
    /// Quantum compression with the given level and memory.
    Quantum(u16, u16),
    /// LZX compression with the given window size.  The LZX compression scheme
    /// is described further in
    /// [MS-PATCH](https://msdn.microsoft.com/en-us/library/cc483133.aspx).
    Lzx(u16),
}

impl CompressionType {
    pub(crate) fn from_bitfield(bits: u16) -> io::Result<CompressionType> {
        let ctype = bits & 0x000f;
        if ctype == CTYPE_NONE {
            Ok(CompressionType::None)
        } else if ctype == CTYPE_MSZIP {
            Ok(CompressionType::MsZip)
        } else if ctype == CTYPE_QUANTUM {
            let level = (bits & 0x00f0) >> 4;
            if !(QUANTUM_LEVEL_MIN..=QUANTUM_LEVEL_MAX).contains(&level) {
                invalid_data!("Invalid Quantum level: 0x{:02x}", level);
            }
            let memory = (bits & 0x1f00) >> 8;
            if !(QUANTUM_MEMORY_MIN..=QUANTUM_MEMORY_MAX).contains(&memory) {
                invalid_data!("Invalid Quantum memory: 0x{:02x}", memory);
            }
            Ok(CompressionType::Quantum(level, memory))
        } else if ctype == CTYPE_LZX {
            let window = (bits & 0x1f00) >> 8;
            if !(LZX_WINDOW_MIN..=LZX_WINDOW_MAX).contains(&window) {
                invalid_data!("Invalid LZX window: 0x{:02x}", window);
            }
            Ok(CompressionType::Lzx(window))
        } else {
            invalid_data!("Invalid compression type: 0x{:04x}", bits);
        }
    }

    pub(crate) fn to_bitfield(self) -> u16 {
        match self {
            CompressionType::None => CTYPE_NONE,
            CompressionType::MsZip => CTYPE_MSZIP,
            CompressionType::Quantum(level, memory) => {
                CTYPE_QUANTUM
                    | (level.max(QUANTUM_LEVEL_MIN).min(QUANTUM_LEVEL_MAX)
                        << 4)
                    | (memory.max(QUANTUM_MEMORY_MIN).min(QUANTUM_MEMORY_MAX)
                        << 8)
            }
            CompressionType::Lzx(window) => {
                CTYPE_LZX
                    | (window.max(LZX_WINDOW_MIN).min(LZX_WINDOW_MAX) << 8)
            }
        }
    }
}

// ========================================================================= //

#[cfg(test)]
mod tests {
    use super::CompressionType;

    #[test]
    fn compression_type_to_bitfield() {
        assert_eq!(CompressionType::None.to_bitfield(), 0x0);
        assert_eq!(CompressionType::MsZip.to_bitfield(), 0x1);
        assert_eq!(CompressionType::Quantum(7, 20).to_bitfield(), 0x1472);
        assert_eq!(CompressionType::Lzx(21).to_bitfield(), 0x1503);
    }

    #[test]
    fn compression_type_from_bitfield() {
        assert_eq!(
            CompressionType::from_bitfield(0x0).unwrap(),
            CompressionType::None
        );
        assert_eq!(
            CompressionType::from_bitfield(0x1).unwrap(),
            CompressionType::MsZip
        );
        assert_eq!(
            CompressionType::from_bitfield(0x1472).unwrap(),
            CompressionType::Quantum(7, 20)
        );
        assert_eq!(
            CompressionType::from_bitfield(0x1503).unwrap(),
            CompressionType::Lzx(21)
        );
    }
}

// ========================================================================= //
