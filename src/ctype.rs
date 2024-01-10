use std::io;

use lzxd::Lzxd;

use crate::mszip::MsZipDecompressor;

const CTYPE_NONE: u16 = 0;
const CTYPE_MSZIP: u16 = 1;
const CTYPE_QUANTUM: u16 = 2;
const CTYPE_LZX: u16 = 3;

const QUANTUM_LEVEL_MIN: u16 = 1;
const QUANTUM_LEVEL_MAX: u16 = 7;
const QUANTUM_MEMORY_MIN: u16 = 10;
const QUANTUM_MEMORY_MAX: u16 = 21;

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
    Lzx(lzxd::WindowSize),
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
            let window = match window {
                15 => lzxd::WindowSize::KB32,
                16 => lzxd::WindowSize::KB64,
                17 => lzxd::WindowSize::KB128,
                18 => lzxd::WindowSize::KB256,
                19 => lzxd::WindowSize::KB512,
                20 => lzxd::WindowSize::MB1,
                21 => lzxd::WindowSize::MB2,
                22 => lzxd::WindowSize::MB4,
                23 => lzxd::WindowSize::MB8,
                24 => lzxd::WindowSize::MB16,
                25 => lzxd::WindowSize::MB32,
                _ => invalid_data!("Invalid LZX window: 0x{:02x}", window),
            };
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
            CompressionType::Lzx(window_size) => {
                let window = match window_size {
                    lzxd::WindowSize::KB32 => 15,
                    lzxd::WindowSize::KB64 => 16,
                    lzxd::WindowSize::KB128 => 17,
                    lzxd::WindowSize::KB256 => 18,
                    lzxd::WindowSize::KB512 => 19,
                    lzxd::WindowSize::MB1 => 20,
                    lzxd::WindowSize::MB2 => 21,
                    lzxd::WindowSize::MB4 => 22,
                    lzxd::WindowSize::MB8 => 23,
                    lzxd::WindowSize::MB16 => 24,
                    lzxd::WindowSize::MB32 => 25,
                };
                CTYPE_LZX | (window << 8)
            }
        }
    }

    pub(crate) fn into_decompressor(self) -> io::Result<Decompressor> {
        match self {
            CompressionType::None => Ok(Decompressor::Uncompressed),
            CompressionType::MsZip => {
                Ok(Decompressor::MsZip(Box::new(MsZipDecompressor::new())))
            }
            CompressionType::Quantum(_, _) => {
                invalid_data!("Quantum decompression is not yet supported.")
            }
            CompressionType::Lzx(window_size) => {
                Ok(Decompressor::Lzx(Box::new(Lzxd::new(window_size))))
            }
        }
    }
}

pub enum Decompressor {
    Uncompressed,
    MsZip(Box<MsZipDecompressor>),
    Lzx(Box<Lzxd>),
}

impl Decompressor {
    pub(crate) fn reset(&mut self) {
        match self {
            Self::Uncompressed => {}
            Self::MsZip(d) => d.reset(),
            Self::Lzx(d) => d.reset(),
        }
    }

    pub(crate) fn decompress(
        &mut self,
        data: Vec<u8>,
        uncompressed_size: usize,
    ) -> io::Result<Vec<u8>> {
        let data = match self {
            Decompressor::Uncompressed => data,
            Decompressor::MsZip(decompressor) => decompressor
                .decompress_block(&data, uncompressed_size)
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?
                .to_vec(),
            Decompressor::Lzx(decompressor) => decompressor
                .decompress_next(&data, uncompressed_size)
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?
                .to_vec(),
        };
        Ok(data)
    }
}

#[cfg(test)]
mod tests {
    use super::CompressionType;

    #[test]
    fn compression_type_to_bitfield() {
        assert_eq!(CompressionType::None.to_bitfield(), 0x0);
        assert_eq!(CompressionType::MsZip.to_bitfield(), 0x1);
        assert_eq!(CompressionType::Quantum(7, 20).to_bitfield(), 0x1472);
        assert_eq!(
            CompressionType::Lzx(lzxd::WindowSize::MB2).to_bitfield(),
            0x1503
        );
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
            CompressionType::Lzx(lzxd::WindowSize::MB2)
        );
    }
}
