use std::io::{self, Read, Seek, SeekFrom};
use std::marker::PhantomData;
use std::slice;

use byteorder::{LittleEndian, ReadBytesExt};
use lzxd::Lzxd;

use crate::cabinet::{Cabinet, ReadSeek};
use crate::checksum::Checksum;
use crate::ctype::CompressionType;
use crate::file::{FileEntries, FileEntry};
use crate::mszip::MsZipDecompressor;

/// An iterator over the folder entries in a cabinet.
#[derive(Clone)]
pub struct FolderEntries<'a> {
    pub(crate) iter: slice::Iter<'a, _FolderEntry>,
    pub(crate) files: &'a [FileEntry],
}

/// Metadata about one folder in a cabinet.
pub struct FolderEntry<'a> {
    entry: &'a _FolderEntry,
    files: &'a [FileEntry],
}

pub(crate) struct _FolderEntry {
    first_data_block_offset: u32,
    num_data_blocks: u16,
    compression_type: CompressionType,
    reserve_data: Vec<u8>,
    pub(crate) file_idx_start: usize,
    pub(crate) files_count: usize,
}

#[derive(Debug, Clone)]
struct DataBlockEntry {
    checksum: u32,
    compressed_size: u16,
    uncompressed_size: u16,
    reserve_data: Vec<u8>,
    data_offset: u64,
    cumulative_size: u64,
}

/// A reader for reading decompressed data from a cabinet folder.
pub(crate) struct FolderReader<'a, R> {
    reader: &'a Cabinet<dyn ReadSeek + 'a>,
    decompressor: FolderDecompressor,
    total_size: u64,
    data_blocks: Vec<DataBlockEntry>,
    current_block_index: usize,
    current_block_data: Vec<u8>,
    current_offset_within_block: usize,
    current_offset_within_folder: u64,
    _p: PhantomData<R>,
}

enum FolderDecompressor {
    Uncompressed,
    MsZip(Box<MsZipDecompressor>),
    Lzx(Box<Lzxd>),
    // TODO: add options for other compression types
}

impl<'a> Iterator for FolderEntries<'a> {
    type Item = FolderEntry<'a>;

    fn next(&mut self) -> Option<FolderEntry<'a>> {
        let entry = self.iter.next()?;
        let files = &self.files
            [entry.file_idx_start..entry.file_idx_start + entry.files_count];
        Some(FolderEntry { entry, files })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.iter.size_hint()
    }
}

impl<'a> ExactSizeIterator for FolderEntries<'a> {}

impl<'a> FolderEntry<'a> {
    /// Returns the scheme used to compress this folder's data.
    pub fn compression_type(&self) -> CompressionType {
        self.entry.compression_type
    }

    /// Returns the number of data blocks used to store this folder's data.
    pub fn num_data_blocks(&self) -> u16 {
        self.entry.num_data_blocks
    }

    /// Returns the application-defined reserve data for this folder.
    pub fn reserve_data(&self) -> &[u8] {
        &self.entry.reserve_data
    }

    /// Returns an iterator over the file entries in this folder.
    pub fn file_entries(&self) -> FileEntries {
        FileEntries { iter: self.files.iter() }
    }
}

impl<'a, R: Read + Seek> FolderReader<'a, R> {
    pub(crate) fn new(
        reader: &'a Cabinet<dyn ReadSeek + 'a>,
        entry: &_FolderEntry,
        data_reserve_size: u8,
    ) -> io::Result<FolderReader<'a, R>> {
        let num_data_blocks = entry.num_data_blocks as usize;
        let mut data_blocks = Vec::with_capacity(num_data_blocks);
        let mut total_size: u64 = 0;

        let r = &mut &reader.inner;
        r.seek(SeekFrom::Start(entry.first_data_block_offset as u64))?;
        for _ in 0..num_data_blocks {
            let r = &mut &reader.inner;
            let block =
                parse_block_entry(r, total_size, data_reserve_size as usize)?;
            total_size += block.uncompressed_size as u64;
            data_blocks.push(block);
        }
        let decompressor = match entry.compression_type {
            CompressionType::None => FolderDecompressor::Uncompressed,
            CompressionType::MsZip => {
                FolderDecompressor::MsZip(Box::new(MsZipDecompressor::new()))
            }
            CompressionType::Quantum(_, _) => {
                invalid_data!("Quantum decompression is not yet supported.");
            }
            CompressionType::Lzx(window_size) => {
                let lzxd = Lzxd::new(match window_size {
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

                    _ => invalid_data!("LZX given with invalid window size"),
                });
                FolderDecompressor::Lzx(Box::new(lzxd))
            }
        };
        let mut folder_reader = FolderReader {
            reader,
            decompressor,
            total_size,
            data_blocks,
            current_block_index: 0,
            current_block_data: Vec::new(),
            current_offset_within_block: 0,
            current_offset_within_folder: 0,
            _p: PhantomData,
        };
        folder_reader.load_block()?;
        Ok(folder_reader)
    }

    fn current_block_start(&self) -> u64 {
        if self.current_block_index == 0 {
            0
        } else {
            self.data_blocks[self.current_block_index - 1].cumulative_size
        }
    }

    fn rewind(&mut self) -> io::Result<()> {
        self.current_offset_within_block = 0;
        self.current_offset_within_folder = 0;
        if self.current_block_index != 0 {
            self.current_block_index = 0;
            self.load_block()?;
        }
        Ok(())
    }

    fn load_block(&mut self) -> io::Result<()> {
        if self.current_block_index >= self.data_blocks.len() {
            self.current_block_data = Vec::new();
            return Ok(());
        }
        let block = &self.data_blocks[self.current_block_index];
        let reader = &mut &self.reader.inner;

        reader.seek(SeekFrom::Start(block.data_offset))?;
        let mut compressed_data = vec![0u8; block.compressed_size as usize];
        reader.read_exact(&mut compressed_data)?;
        if block.checksum != 0 {
            let mut checksum = Checksum::new();
            checksum.update(&block.reserve_data);
            checksum.update(&compressed_data);
            let actual_checksum = checksum.value()
                ^ ((block.compressed_size as u32)
                    | ((block.uncompressed_size as u32) << 16));
            if actual_checksum != block.checksum {
                invalid_data!(
                    "Checksum error in data block {} \
                     (expected {:08x}, actual {:08x})",
                    self.current_block_index,
                    block.checksum,
                    actual_checksum
                );
            }
        }
        self.current_block_data = match self.decompressor {
            FolderDecompressor::Uncompressed => compressed_data,
            FolderDecompressor::MsZip(ref mut decompressor) => decompressor
                .decompress_block(&compressed_data, block.uncompressed_size)?,
            FolderDecompressor::Lzx(ref mut decompressor) => decompressor
                .decompress_next(
                    &compressed_data,
                    block.uncompressed_size as usize,
                )
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?
                .to_vec(),
        };
        Ok(())
    }
}

impl<'a, R: Read + Seek + 'a> Read for FolderReader<'a, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() || self.current_block_index >= self.data_blocks.len()
        {
            return Ok(0);
        }
        if self.current_offset_within_block == self.current_block_data.len() {
            self.current_block_index += 1;
            self.current_offset_within_block = 0;
            self.load_block()?;
        }
        let max_bytes = buf.len().min(
            self.current_block_data.len() - self.current_offset_within_block,
        );
        buf[..max_bytes].copy_from_slice(
            &self.current_block_data[self.current_offset_within_block..]
                [..max_bytes],
        );
        self.current_offset_within_block += max_bytes;
        self.current_offset_within_folder += max_bytes as u64;
        Ok(max_bytes)
    }
}

impl<'a, R: Read + Seek> Seek for FolderReader<'a, R> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let new_offset = match pos {
            SeekFrom::Start(offset) => offset as i64,
            SeekFrom::Current(delta) => {
                self.current_offset_within_folder as i64 + delta
            }
            SeekFrom::End(delta) => self.total_size as i64 + delta,
        };
        if new_offset < 0 || (new_offset as u64) > self.total_size {
            invalid_input!(
                "Cannot seek to {}, data length is {}",
                new_offset,
                self.total_size
            );
        }
        let new_offset = new_offset as u64;
        if new_offset < self.current_block_start() {
            self.rewind()?;
        }
        if new_offset > 0 {
            // TODO: If folder is uncompressed, we should just jump straight to
            // the correct block without "decompressing" those in between.
            while self.data_blocks[self.current_block_index].cumulative_size
                < new_offset
            {
                self.current_block_index += 1;
                self.load_block()?;
            }
        }
        debug_assert!(new_offset >= self.current_block_start());
        self.current_offset_within_block =
            (new_offset - self.current_block_start()) as usize;
        self.current_offset_within_folder = new_offset;
        Ok(new_offset)
    }
}

pub(crate) fn parse_folder_entry<R: Read>(
    mut reader: R,
    reserve_size: usize,
) -> io::Result<_FolderEntry> {
    let first_data_offset = reader.read_u32::<LittleEndian>()?;
    let num_data_blocks = reader.read_u16::<LittleEndian>()?;
    let compression_bits = reader.read_u16::<LittleEndian>()?;
    let compression_type = CompressionType::from_bitfield(compression_bits)?;
    let mut folder_reserve_data = vec![0u8; reserve_size];
    if reserve_size > 0 {
        reader.read_exact(&mut folder_reserve_data)?;
    }
    let entry = _FolderEntry {
        first_data_block_offset: first_data_offset,
        num_data_blocks,
        compression_type,
        reserve_data: folder_reserve_data,
        file_idx_start: 0,
        files_count: 0,
    };
    Ok(entry)
}

fn parse_block_entry<R: ReadSeek>(
    mut reader: R,
    cumulative_size: u64,
    data_reserve_size: usize,
) -> io::Result<DataBlockEntry> {
    let checksum = reader.read_u32::<LittleEndian>()?;
    let compressed_size = reader.read_u16::<LittleEndian>()?;
    let uncompressed_size = reader.read_u16::<LittleEndian>()?;
    let mut reserve_data = vec![0u8; data_reserve_size];
    reader.read_exact(&mut reserve_data)?;
    let data_offset = reader.stream_position()?;
    reader.seek(SeekFrom::Current(compressed_size as i64))?;
    let cumulative_size = cumulative_size + uncompressed_size as u64;

    Ok(DataBlockEntry {
        checksum,
        compressed_size,
        uncompressed_size,
        reserve_data,
        cumulative_size,
        data_offset,
    })
}
