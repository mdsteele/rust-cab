use std::io;

use byteorder::{LittleEndian, WriteBytesExt};
use flate2::Compression;

const MSZIP_SIGNATURE: u16 = 0x4B43; // "CK" stored little-endian
const MSZIP_SIGNATURE_LEN: usize = 2;
const MSZIP_BLOCK_TERMINATOR: u16 = 0x0003;
const DEFLATE_MAX_DICT_LEN: usize = 0x8000;

pub struct MsZipCompressor {
    compressor: flate2::Compress,
}

impl MsZipCompressor {
    pub fn new() -> MsZipCompressor {
        MsZipCompressor {
            compressor: flate2::Compress::new(Compression::best(), false),
        }
    }

    pub fn compress_block(
        &mut self,
        data: &[u8],
        is_last_block: bool,
    ) -> io::Result<Vec<u8>> {
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

    pub fn reset(&mut self) {
        self.decompressor.reset(true);
        self.dictionary = Vec::with_capacity(DEFLATE_MAX_DICT_LEN);
    }

    pub fn decompress_block(
        &mut self,
        data: &[u8],
        uncompressed_size: usize,
    ) -> io::Result<Vec<u8>> {
        // Check signature:
        if data.len() < MSZIP_SIGNATURE_LEN
            || ((data[0] as u16) | ((data[1] as u16) << 8)) != MSZIP_SIGNATURE
        {
            invalid_data!(
                "MSZIP decompression failed: Invalid block signature"
            );
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
        let mut out = Vec::<u8>::with_capacity(uncompressed_size);
        let flush = flate2::FlushDecompress::Finish;
        match self.decompressor.decompress_vec(data, &mut out, flush) {
            Ok(_) => {}
            Err(error) => {
                invalid_data!("MSZIP decompression failed: {}", error);
            }
        }
        if out.len() != uncompressed_size {
            invalid_data!(
                "MSZIP decompression failed: Incorrect uncompressed size \
                 (expected {}, was actually {})",
                uncompressed_size,
                out.len()
            );
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

#[cfg(test)]
mod tests {
    use rand::RngCore;

    use super::{MsZipCompressor, MsZipDecompressor, DEFLATE_MAX_DICT_LEN};

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
        let output =
            decompressor.decompress_block(&input, expected.len()).unwrap();
        assert_eq!(output, expected);
    }

    fn repeating_data(size: usize) -> Vec<u8> {
        let modulus = 251; // a prime number no bigger than u8::MAX
        (0..size).map(|index| (index % modulus) as u8).collect::<Vec<u8>>()
    }

    fn random_data(size: usize) -> Vec<u8> {
        use rand::SeedableRng;

        let mut rd = vec![0; size];
        rand::rngs::SmallRng::from_entropy().fill_bytes(&mut rd);
        rd
    }

    #[cfg(target_env = "msvc")]
    /// Wrappers for the Microsoft compression API so that on Windows we can
    /// test interop with the system implementation.  This code comes from
    /// https://github.com/luser/rust-makecab; thanks to Ted Mielczarek for
    /// sharing it.
    mod sys {
        #![allow(non_camel_case_types)]

        use std::mem;
        use std::ptr;

        use winapi::shared::basetsd::{PSIZE_T, SIZE_T};
        use winapi::shared::minwindef::{BOOL, DWORD, FALSE, LPVOID, TRUE};
        use winapi::um::winnt::{HANDLE, PVOID};

        use super::super::DEFLATE_MAX_DICT_LEN;

        const COMPRESS_ALGORITHM_MSZIP: DWORD = 2;
        const COMPRESS_RAW: DWORD = 1 << 29;
        type PCOMPRESS_ALLOCATION_ROUTINES = LPVOID;
        type COMPRESSOR_HANDLE = HANDLE;
        type DECOMPRESSOR_HANDLE = HANDLE;
        type PCOMPRESSOR_HANDLE = *mut COMPRESSOR_HANDLE;
        type PDECOMPRESSOR_HANDLE = *mut DECOMPRESSOR_HANDLE;

        #[rustfmt::skip]
        #[link(name = "cabinet")]
        extern "system" {
            fn CreateCompressor(
                Algorithm: DWORD,
                AllocationRoutines: LPVOID,
                CompressorHandle: PCOMPRESSOR_HANDLE)
                -> BOOL;
            fn CloseCompressor(
                CompressorHandle: COMPRESSOR_HANDLE)
                -> BOOL;
            fn Compress(
                CompressorHandle: COMPRESSOR_HANDLE,
                UncompressedData: PVOID,
                UncompressedDataSize: SIZE_T,
                CompressedBuffer: PVOID,
                CompressedBufferSize: SIZE_T,
                CompressedDataSize: PSIZE_T)
                -> BOOL;

            fn CreateDecompressor(
                Algorithm: DWORD,
                AllocationRoutines: PCOMPRESS_ALLOCATION_ROUTINES,
                DecompressorHandle: PDECOMPRESSOR_HANDLE)
                -> BOOL;
            fn CloseDecompressor(
                DecompressorHandle: DECOMPRESSOR_HANDLE)
                -> BOOL;
            fn Decompress(
                DecompressorHandle: DECOMPRESSOR_HANDLE,
                CompressedData: PVOID,
                CompressedDataSize: SIZE_T,
                UncompressedBuffer: PVOID,
                UncompressedBufferSize: SIZE_T,
                UncompressedDataSize: PSIZE_T)
                -> BOOL;
        }

        /// Compress `data` with the Microsoft compression API.
        pub fn do_system_compress(data: &[u8]) -> Vec<(usize, Vec<u8>)> {
            let handle = unsafe {
                let mut handle = mem::MaybeUninit::uninit();
                if CreateCompressor(
                    COMPRESS_ALGORITHM_MSZIP | COMPRESS_RAW,
                    ptr::null_mut(),
                    handle.as_mut_ptr(),
                ) != TRUE
                {
                    panic!("CreateCompressor failed");
                }
                handle.assume_init()
            };
            let mut blocks = Vec::<(usize, Vec<u8>)>::new();
            for slice in data.chunks(DEFLATE_MAX_DICT_LEN) {
                let mut buffer = vec![0; 0xffff];
                unsafe {
                    let mut compressed_size = mem::MaybeUninit::uninit();
                    if Compress(
                        handle,
                        slice.as_ptr() as PVOID,
                        slice.len() as SIZE_T,
                        buffer.as_ptr() as PVOID,
                        buffer.len() as SIZE_T,
                        compressed_size.as_mut_ptr(),
                    ) == FALSE
                    {
                        panic!("Compress failed");
                    }
                    buffer.resize(compressed_size.assume_init() as usize, 0);
                }
                blocks.push((slice.len(), buffer));
            }
            unsafe {
                CloseCompressor(handle);
            }
            blocks
        }

        pub fn do_system_decompress(blocks: Vec<(usize, Vec<u8>)>) -> Vec<u8> {
            let handle = unsafe {
                let mut handle = mem::MaybeUninit::uninit();
                if CreateDecompressor(
                    COMPRESS_ALGORITHM_MSZIP | COMPRESS_RAW,
                    ptr::null_mut(),
                    handle.as_mut_ptr(),
                ) != TRUE
                {
                    panic!("CreateDecompressor failed");
                }
                handle.assume_init()
            };
            let mut buffer = Vec::<u8>::new();
            // Decompress each chunk in turn.
            for (original_size, ref block) in blocks.into_iter() {
                assert!(original_size <= DEFLATE_MAX_DICT_LEN);
                // Make space in the output buffer.
                let last = buffer.len();
                buffer.resize(last + original_size, 0);
                unsafe {
                    if Decompress(
                        handle,
                        block.as_ptr() as PVOID,
                        block.len() as SIZE_T,
                        buffer[last..].as_mut_ptr() as PVOID,
                        original_size as SIZE_T,
                        ptr::null_mut(),
                    ) == FALSE
                    {
                        panic!("Decompress failed");
                    }
                }
            }
            unsafe {
                CloseDecompressor(handle);
            }
            buffer
        }
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
            output.append(
                &mut decompressor.decompress_block(&compressed, size).unwrap(),
            );
        }
        output
    }

    macro_rules! round_trip_tests {
        ($name:ident, $data:expr) => {
            mod $name {
                use super::*;

                #[test]
                fn lib_to_lib() {
                    let original: &[u8] = $data;
                    let compressed = do_lib_compress(original);
                    assert_eq!(
                        do_lib_decompress(compressed).as_slice(),
                        original
                    );
                }

                #[cfg(target_env = "msvc")]
                #[test]
                fn lib_to_sys() {
                    let original: &[u8] = $data;
                    let compressed = do_lib_compress(original);
                    assert_eq!(
                        sys::do_system_decompress(compressed).as_slice(),
                        original
                    );
                }

                #[cfg(target_env = "msvc")]
                #[test]
                fn sys_to_lib() {
                    let original: &[u8] = $data;
                    let compressed = sys::do_system_compress(original);
                    assert_eq!(
                        do_lib_decompress(compressed).as_slice(),
                        original
                    );
                }
            }
        };
    }

    round_trip_tests!(
        lorem_ipsum,
        b"Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed \
          do eiusmod tempor incididunt ut labore et dolore magna aliqua."
    );

    round_trip_tests!(
        one_block_exactly,
        &repeating_data(DEFLATE_MAX_DICT_LEN)
    );
    round_trip_tests!(
        one_block_less_a_byte,
        &repeating_data(DEFLATE_MAX_DICT_LEN - 1)
    );
    round_trip_tests!(
        one_block_plus_a_byte,
        &repeating_data(DEFLATE_MAX_DICT_LEN + 1)
    );

    round_trip_tests!(zeros_one_block, &[0u8; 1000]);
    round_trip_tests!(zeros_two_blocks, &[0u8; DEFLATE_MAX_DICT_LEN + 1000]);
    round_trip_tests!(zeros_many_blocks, &[0u8; DEFLATE_MAX_DICT_LEN * 10]);

    round_trip_tests!(repeating_one_block, &repeating_data(1000));
    round_trip_tests!(
        repeating_two_blocks,
        &repeating_data(DEFLATE_MAX_DICT_LEN + 1000)
    );
    round_trip_tests!(
        repeating_many_blocks,
        &repeating_data(DEFLATE_MAX_DICT_LEN * 10)
    );

    round_trip_tests!(random_one_block, &random_data(1000));
    round_trip_tests!(
        random_two_blocks,
        &random_data(DEFLATE_MAX_DICT_LEN + 1000)
    );
    round_trip_tests!(
        random_many_blocks,
        &random_data(DEFLATE_MAX_DICT_LEN * 10)
    );
}
