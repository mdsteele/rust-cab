use byteorder::{LittleEndian, WriteBytesExt};
use flate2;
use flate2::Compression;
use std::io;

// ========================================================================= //

const MSZIP_SIGNATURE: u16 = 0x4B43; // "CK" stored little-endian
const MSZIP_SIGNATURE_LEN: usize = 2;
const MSZIP_BLOCK_TERMINATOR: u16 = 0x0003;
const DEFLATE_MAX_DICT_LEN: usize = 0x8000;

// ========================================================================= //

pub struct MsZipCompressor {
    compressor: flate2::Compress,
}

impl MsZipCompressor {
    pub fn new() -> MsZipCompressor {
        MsZipCompressor {
            compressor: flate2::Compress::new(Compression::best(), false),
        }
    }

    pub fn compress_block(&mut self, data: &[u8], is_last_block: bool)
                          -> io::Result<Vec<u8>> {
        debug_assert!(data.len() <= 0x8000);
        let mut out = Vec::<u8>::with_capacity(0xffff);
        out.write_u16::<LittleEndian>(MSZIP_SIGNATURE)?;
        let flush = if is_last_block {
            flate2::FlushCompress::Finish
        } else {
            flate2::FlushCompress::Sync
        };
        match self.compressor.compress_vec(data, &mut out, flush) {
            Ok(_) => {}
            Err(error) => invalid_data!("MSZIP compression failed: {}", error),
        }
        if !is_last_block {
            out.write_u16::<LittleEndian>(MSZIP_BLOCK_TERMINATOR)?;
        }
        let max_out_len = data.len() + 7;
        if out.len() > max_out_len {
            out = Vec::with_capacity(max_out_len);
            out.write_u16::<LittleEndian>(MSZIP_SIGNATURE)?;
            out.push(1);
            out.write_u16::<LittleEndian>(data.len() as u16)?;
            out.write_u16::<LittleEndian>(!(data.len() as u16))?;
            out.extend_from_slice(data);
            debug_assert_eq!(out.len(), max_out_len);
            debug_assert_eq!(out.capacity(), max_out_len);
        }
        Ok(out)
    }
}

// ========================================================================= //

pub struct MsZipDecompressor {
    decompressor: flate2::Decompress,
    dictionary: Vec<u8>,
}

impl MsZipDecompressor {
    pub fn new() -> MsZipDecompressor {
        MsZipDecompressor {
            decompressor: flate2::Decompress::new(false),
            dictionary: Vec::with_capacity(DEFLATE_MAX_DICT_LEN),
        }
    }

    pub fn decompress_block(&mut self, data: &[u8], uncompressed_size: u16)
                            -> io::Result<Vec<u8>> {
        // Check signature:
        if data.len() < (MSZIP_SIGNATURE_LEN as usize) ||
            ((data[0] as u16) | ((data[1] as u16) << 8)) != MSZIP_SIGNATURE
        {
            invalid_data!("MSZIP decompression failed: \
                           Invalid block signature");
        }
        let data = &data[MSZIP_SIGNATURE_LEN..];
        // Reset decompressor with appropriate dictionary:
        self.decompressor.reset(false);
        if !self.dictionary.is_empty() {
            // TODO: Avoid doing extra allocations/copies here.
            debug_assert!(self.dictionary.len() <= DEFLATE_MAX_DICT_LEN);
            let length = self.dictionary.len() as u16;
            let mut chunk: Vec<u8> = vec![0];
            chunk.write_u16::<LittleEndian>(length)?;
            chunk.write_u16::<LittleEndian>(!length)?;
            chunk.extend_from_slice(&self.dictionary);
            let mut out = Vec::with_capacity(self.dictionary.len());
            let flush = flate2::FlushDecompress::Sync;
            match self.decompressor.decompress_vec(&chunk, &mut out, flush) {
                Ok(flate2::Status::Ok) => {}
                _ => unreachable!(),
            }
        }
        // Decompress data:
        let mut out = Vec::<u8>::with_capacity(uncompressed_size as usize);
        let flush = flate2::FlushDecompress::Finish;
        match self.decompressor.decompress_vec(data, &mut out, flush) {
            Ok(_) => {}
            Err(error) => {
                invalid_data!("MSZIP decompression failed: {}", error);
            }
        }
        if out.len() != uncompressed_size as usize {
            invalid_data!("MSZIP decompression failed: Incorrect uncompressed \
                           size (expected {}, was actually {})",
                          uncompressed_size,
                          out.len());
        }
        // Update dictionary for next block:
        if out.len() >= DEFLATE_MAX_DICT_LEN {
            let start = out.len() - DEFLATE_MAX_DICT_LEN;
            self.dictionary = out[start..].to_vec();
        } else {
            let total = self.dictionary.len() + out.len();
            if total > DEFLATE_MAX_DICT_LEN {
                self.dictionary.drain(..(total - DEFLATE_MAX_DICT_LEN));
            }
            self.dictionary.extend_from_slice(&out);
        }
        debug_assert_eq!(self.dictionary.capacity(), DEFLATE_MAX_DICT_LEN);
        Ok(out)
    }
}

// ========================================================================= //

#[cfg(test)]
mod tests {
    extern crate rand;
    use self::rand::Rng;
    use super::{DEFLATE_MAX_DICT_LEN, MsZipCompressor, MsZipDecompressor};

    #[test]
    fn read_compressed_data() {
        let input: &[u8] = b"CK%\xcc\xd1\t\x031\x0c\x04\xd1V\xb6\x80#\x95\xa4\
              \t\xc5\x12\xc7\x82e\xfb,\xa9\xff\x18\xee{x\xf3\x9d\xdb\x1c\\Q\
              \x0e\x9d}n\x04\x13\xe2\x96\x17\xda\x1ca--kC\x94\x8b\xd18nX\xe7\
              \x89az\x00\x8c\x15>\x15i\xbe\x0e\xe6hTj\x8dD%\xba\xfc\xce\x1e\
              \x96\xef\xda\xe0r\x0f\x81t>%\x9f?\x12]-\x87";
        let expected: &[u8] =
            b"Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed \
              do eiusmod tempor incididunt ut labore et dolore magna aliqua.";
        assert!(input.len() < expected.len());
        let mut decompressor = MsZipDecompressor::new();
        let output = decompressor
            .decompress_block(&input, expected.len() as u16)
            .unwrap();
        assert_eq!(output, expected);
    }

    fn repeating_data(size: usize) -> Vec<u8> {
        let modulus = 251; // a prime number no bigger than u8::MAX
        (0..size).map(|index| (index % modulus) as u8).collect::<Vec<u8>>()
    }

    fn random_data(size: usize) -> Vec<u8> {
        rand::thread_rng().gen_iter::<u8>().take(size).collect()
    }

    fn do_lib_compress(mut data: &[u8]) -> Vec<(usize, Vec<u8>)> {
        let mut blocks = Vec::<(usize, Vec<u8>)>::new();
        let mut compressor = MsZipCompressor::new();
        while data.len() > DEFLATE_MAX_DICT_LEN {
            let slice = &data[0..DEFLATE_MAX_DICT_LEN];
            let compressed = compressor.compress_block(slice, false).unwrap();
            blocks.push((slice.len(), compressed));
            data = &data[slice.len()..];
        }
        let compressed = compressor.compress_block(data, true).unwrap();
        blocks.push((data.len(), compressed));
        blocks
    }

    fn do_lib_decompress(blocks: Vec<(usize, Vec<u8>)>) -> Vec<u8> {
        let mut output = Vec::<u8>::new();
        let mut decompressor = MsZipDecompressor::new();
        for (size, compressed) in blocks.into_iter() {
            output.append(&mut decompressor
                              .decompress_block(&compressed, size as u16)
                              .unwrap());
        }
        output
    }

    fn test_lib_round_trip(data: &[u8]) {
        assert_eq!(do_lib_decompress(do_lib_compress(data)).as_slice(), data);
    }

    #[test]
    fn lorem_ipsum_lib_round_trip() {
        test_lib_round_trip(
            b"Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed \
              do eiusmod tempor incididunt ut labore et dolore magna aliqua.");
    }

    #[test]
    fn one_block_exactly_lib_round_trip() {
        test_lib_round_trip(&repeating_data(DEFLATE_MAX_DICT_LEN));
    }

    #[test]
    fn one_block_less_a_byte_lib_round_trip() {
        test_lib_round_trip(&repeating_data(DEFLATE_MAX_DICT_LEN - 1));
    }

    #[test]
    fn one_block_plus_a_byte_lib_round_trip() {
        test_lib_round_trip(&repeating_data(DEFLATE_MAX_DICT_LEN + 1));
    }

    #[test]
    fn zeros_one_block_lib_round_trip() { test_lib_round_trip(&[0u8; 1000]); }

    #[test]
    fn zeros_two_blocks_lib_round_trip() {
        test_lib_round_trip(&[0u8; DEFLATE_MAX_DICT_LEN + 1000]);
    }

    #[test]
    fn zeros_many_blocks_lib_round_trip() {
        test_lib_round_trip(&[0u8; DEFLATE_MAX_DICT_LEN * 10]);
    }

    #[test]
    fn repeating_one_block_lib_round_trip() {
        test_lib_round_trip(&repeating_data(1000));
    }

    #[test]
    fn repeating_two_blocks_lib_round_trip() {
        test_lib_round_trip(&repeating_data(DEFLATE_MAX_DICT_LEN + 1000));
    }

    #[test]
    fn repeating_many_blocks_lib_round_trip() {
        test_lib_round_trip(&repeating_data(DEFLATE_MAX_DICT_LEN * 10));
    }

    #[test]
    fn random_one_block_lib_round_trip() {
        test_lib_round_trip(&random_data(1000));
    }

    #[test]
    fn random_two_blocks_lib_round_trip() {
        test_lib_round_trip(&random_data(DEFLATE_MAX_DICT_LEN + 1000));
    }

    #[test]
    fn random_many_blocks_lib_round_trip() {
        test_lib_round_trip(&random_data(DEFLATE_MAX_DICT_LEN * 10));
    }
}

// ========================================================================= //
