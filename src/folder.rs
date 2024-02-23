use std::io::{self, Read, Seek, SeekFrom};
use std::marker::PhantomData;
use std::slice;

use byteorder::{LittleEndian, ReadBytesExt};

use crate::cabinet::{Cabinet, ReadSeek};
use crate::checksum::Checksum;
use crate::ctype::{CompressionType, Decompressor};
use crate::file::{FileEntries, FileEntry};

/// An iterator over the folder entries in a cabinet.
#[derive(Clone)]
pub struct FolderEntries<'a> {
    pub(crate) iter: slice::Iter<'a, FolderEntry>,
}

/// Metadata about one folder in a cabinet.
pub struct FolderEntry {
    first_data_block_offset: u32,
    num_data_blocks: u16,
    compression_type: CompressionType,
    reserve_data: Vec<u8>,
    pub(crate) files: Vec<FileEntry>,
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
    num_data_blocks: usize,
    data_reserve_size: u8,
    decompressor: Decompressor,
    /// The data blocks we've read so far.
    /// This always has len() <= num_data_blocks and grows once we encounter
    /// a new block in load_block().
    data_blocks: Vec<DataBlockEntry>,
    current_block_index: usize,
    current_block_data: Vec<u8>,
    current_offset_within_block: usize,
    current_offset_within_folder: u64,
    _p: PhantomData<R>,
}

impl<'a> Iterator for FolderEntries<'a> {
    type Item = &'a FolderEntry;

    fn next(&mut self) -> Option<Self::Item> {
        self.iter.next()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.iter.size_hint()
    }
}

impl<'a> ExactSizeIterator for FolderEntries<'a> {}

impl FolderEntry {
    /// Returns the scheme used to compress this folder's data.
    pub fn compression_type(&self) -> CompressionType {
        self.compression_type
    }

    /// Returns the number of data blocks used to store this folder's data.
    pub fn num_data_blocks(&self) -> u16 {
        self.num_data_blocks
    }

    /// Returns the application-defined reserve data for this folder.
    pub fn reserve_data(&self) -> &[u8] {
        &self.reserve_data
    }

    /// Returns an iterator over the file entries in this folder.
    pub fn file_entries(&self) -> FileEntries {
        FileEntries { iter: self.files.iter() }
    }
}

impl<'a, R: Read + Seek> FolderReader<'a, R> {
    pub(crate) fn new(
        reader: &'a Cabinet<dyn ReadSeek + 'a>,
        entry: &FolderEntry,
        data_reserve_size: u8,
    ) -> io::Result<FolderReader<'a, R>> {
        let num_data_blocks = entry.num_data_blocks as usize;
        let mut data_blocks = Vec::with_capacity(num_data_blocks);

        let r = &mut &reader.inner;
        r.seek(SeekFrom::Start(entry.first_data_block_offset as u64))?;
        if num_data_blocks != 0 {
            let first_block =
                parse_block_entry(*r, 0, data_reserve_size as usize)?;
            data_blocks.push(first_block);
        }

        let decompressor = entry.compression_type.into_decompressor()?;
        let mut folder_reader = FolderReader {
            reader,
            num_data_blocks,
            data_reserve_size,
            decompressor,
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

    pub fn seek_to_uncompressed_offset(
        &mut self,
        new_offset: u64,
    ) -> io::Result<()> {
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
        Ok(())
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
            self.decompressor.reset();
            self.load_block()?;
        }
        Ok(())
    }

    fn load_block(&mut self) -> io::Result<()> {
        if self.current_block_index >= self.num_data_blocks {
            self.current_block_data = Vec::new();
            return Ok(());
        }
        debug_assert!(self.current_block_index <= self.data_blocks.len());
        let block = if self.current_block_index == self.data_blocks.len() {
            let previous_block = self.data_blocks.last().unwrap();
            let reader = &mut &self.reader.inner;
            reader.seek(SeekFrom::Start(
                previous_block.data_offset
                    + previous_block.compressed_size as u64,
            ))?;
            let block = parse_block_entry(
                reader,
                previous_block.cumulative_size,
                self.data_reserve_size as usize,
            )?;
            self.data_blocks.push(block);
            &self.data_blocks[self.current_block_index]
        } else {
            let block = &self.data_blocks[self.current_block_index];
            let reader = &mut &self.reader.inner;
            reader.seek(SeekFrom::Start(block.data_offset))?;
            block
        };

        let mut compressed_data = vec![0u8; block.compressed_size as usize];
        let reader = &mut &self.reader.inner;
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
        self.current_block_data = self
            .decompressor
            .decompress(compressed_data, block.uncompressed_size as usize)?;
        Ok(())
    }
}

impl<'a, R: Read + Seek + 'a> Read for FolderReader<'a, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() || self.current_block_index >= self.num_data_blocks {
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

pub(crate) fn parse_folder_entry<R: Read>(
    mut reader: R,
    reserve_size: usize,
) -> io::Result<FolderEntry> {
    let first_data_offset = reader.read_u32::<LittleEndian>()?;
    let num_data_blocks = reader.read_u16::<LittleEndian>()?;
    let compression_bits = reader.read_u16::<LittleEndian>()?;
    let compression_type = CompressionType::from_bitfield(compression_bits)?;
    let mut folder_reserve_data = vec![0u8; reserve_size];
    if reserve_size > 0 {
        reader.read_exact(&mut folder_reserve_data)?;
    }
    let entry = FolderEntry {
        first_data_block_offset: first_data_offset,
        num_data_blocks,
        compression_type,
        reserve_data: folder_reserve_data,
        files: vec![],
    };
    Ok(entry)
}

/// Parse a data block entry from a reader.
///
/// The reader must be positioned at the start of the data block,
/// which is either at the start first data block or immediately after
/// the previous data block, i.e. at position
/// `previous_data_block.data_offset + previous_data_block.compressed_size`.
///
/// Once this function returns, the reader will be positioned at the current
/// block's `data_offset`.
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
