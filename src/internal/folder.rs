use std::cell::RefCell;
use std::io::{self, Cursor, Read, Seek, SeekFrom};
use std::slice;

use byteorder::{LittleEndian, ReadBytesExt};
use lzxd::Lzxd;

use crate::internal::cabinet::{Cabinet, ReadSeek};
use crate::internal::checksum::Checksum;
use crate::internal::ctype::CompressionType;
use crate::internal::file::{FileEntries, FileEntry};
use crate::internal::mszip::MsZipDecompressor;

/// An iterator over the folder entries in a cabinet.
#[derive(Clone)]
pub struct FolderEntries<'a> {
    pub(crate) archive: &'a Cabinet<dyn ReadSeek + 'a>,
    pub(crate) iter: slice::Iter<'a, FolderEntry>,
    pub(crate) files: &'a [FileEntry],
    pub(crate) data_reserve_size: u8,
}

/// Metadata about one folder in a cabinet.
pub struct FolderEntry {
    first_data_block_offset: u32,
    num_data_blocks: u16,
    compression_type: CompressionType,
    reserve_data: Vec<u8>,
    pub(crate) file_idx_start: usize,
    pub(crate) files_count: usize,
}

enum FolderDecompressor {
    Uncompressed,
    MsZip(Box<MsZipDecompressor>),
    Lzx(Box<Lzxd>),
    // TODO: add options for other compression types
}

/// A reader for reading decompressed data from a cabinet folder.
pub struct FolderReader<'a> {
    archive: &'a Cabinet<dyn ReadSeek + 'a>,
    entry: &'a FolderEntry,
    data_reserve_size: u8,
    decompressor: RefCell<FolderDecompressor>,
    current_block: RefCell<Option<BlockEntry>>,
    current_block_index: RefCell<u16>,
    files: &'a [FileEntry],
}

struct BlockEntry {
    data: RefCell<Cursor<Vec<u8>>>,
}

impl<'a> Iterator for FolderEntries<'a> {
    type Item = io::Result<FolderReader<'a>>;

    fn next(&mut self) -> Option<io::Result<FolderReader<'a>>> {
        let entry = self.iter.next()?;
        let files = &self.files
            [entry.file_idx_start..entry.file_idx_start + entry.files_count];

        let decompressor = match entry.compression_type {
            CompressionType::None => FolderDecompressor::Uncompressed,
            CompressionType::MsZip => {
                FolderDecompressor::MsZip(Box::new(MsZipDecompressor::new()))
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

                    _ => {
                        return Some(invalid_data!(
                            "LZX given with invalid window size"
                        ))
                    }
                });
                FolderDecompressor::Lzx(Box::new(lzxd))
            }
            CompressionType::Quantum(_, _) => {
                return Some(invalid_data!(
                    "Quantum decompression is not yet supported."
                ))
            }
        };
        Some(Ok(FolderReader {
            archive: self.archive,
            entry,
            data_reserve_size: self.data_reserve_size,
            decompressor: RefCell::new(decompressor),
            current_block: RefCell::new(None),
            current_block_index: RefCell::new(0),
            files,
        }))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.iter.size_hint()
    }
}

impl<'a> ExactSizeIterator for FolderEntries<'a> {}

impl<'a> FolderReader<'a> {
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
    pub fn file_entries<'r>(&'r self) -> FileEntries<'r, 'a> {
        FileEntries { reader: self, iter: self.files.iter() }
    }

    fn load_block(&self) -> io::Result<Option<BlockEntry>> {
        let current_block_index = *self.current_block_index.borrow();
        if current_block_index == self.num_data_blocks() {
            return Ok(None);
        }

        let archive = &mut &self.archive.inner;
        if current_block_index == 0 {
            archive.seek(SeekFrom::Start(
                self.entry.first_data_block_offset as u64,
            ))?;
        }

        let expected_checksum = archive.read_u32::<LittleEndian>()?;
        let compressed_size = archive.read_u16::<LittleEndian>()?;
        let uncompressed_size = archive.read_u16::<LittleEndian>()?;
        let mut reserve_data = vec![0u8; self.data_reserve_size as usize];
        archive.read_exact(&mut reserve_data)?;
        let mut compressed_data = vec![0u8; compressed_size as usize];
        archive.read_exact(&mut compressed_data)?;

        if expected_checksum != 0 {
            let mut checksum = Checksum::new();
            checksum.update(&reserve_data);
            checksum.update(&compressed_data);
            let actual_checksum = checksum.value()
                ^ ((compressed_size as u32)
                    | ((uncompressed_size as u32) << 16));
            if actual_checksum != expected_checksum {
                return invalid_data!(
                    "Checksum error in data block {} \
                     (expected {:08x}, actual {:08x})",
                    current_block_index,
                    expected_checksum,
                    actual_checksum
                );
            }
        }

        let data = match &mut *self.decompressor.borrow_mut() {
            FolderDecompressor::Uncompressed => compressed_data,
            FolderDecompressor::MsZip(ref mut decompressor) => decompressor
                .decompress_block(
                    &compressed_data,
                    uncompressed_size as usize,
                )?,
            FolderDecompressor::Lzx(ref mut decompressor) => decompressor
                .decompress_next(&compressed_data)
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?
                .to_vec(),
        };

        *self.current_block_index.borrow_mut() += 1;
        Ok(Some(BlockEntry { data: RefCell::new(Cursor::new(data)) }))
    }
}

impl<'a> Read for &'a FolderReader<'a> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        loop {
            let is_none = self.current_block.borrow().is_none();
            if is_none {
                match self.load_block()? {
                    Some(block) => {
                        self.current_block.replace(Some(block));
                        continue;
                    }
                    None => break Ok(0),
                }
            } else {
                let size = {
                    let opt_block = &*self.current_block.borrow();
                    let block_reader = &mut opt_block.as_ref().unwrap();
                    block_reader.read(buf)?
                };
                if size == 0 {
                    self.current_block.replace(None);
                    continue;
                }
                break Ok(size);
            }
        }
    }
}

impl<'a> Read for &'a BlockEntry {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.data.borrow_mut().read(buf)
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
        file_idx_start: 0,
        files_count: 0,
    };
    Ok(entry)
}
