//! A library for reading/writing [Windows
//! cabinet](https://en.wikipedia.org/wiki/Cabinet_(file_format)) (CAB) files.

#![warn(missing_docs)]

extern crate byteorder;
extern crate flate2;

#[macro_use]
mod internal;

use byteorder::{LittleEndian, ReadBytesExt};
pub use internal::builder::{CabinetBuilder, CabinetWriter, FileWriter};
use internal::consts;
pub use internal::ctype::CompressionType;
use internal::mszip::MsZipReader;
use std::io::{self, Read, Seek, SeekFrom};

// ========================================================================= //

/// A structure for reading a cabinet file.
pub struct Cabinet<R> {
    reader: R,
    cabinet_set_id: u16,
    cabinet_set_index: u16,
    data_reserve_size: u8,
    reserve_data: Vec<u8>,
    folders: Vec<FolderEntry>,
}

impl<R: Read + Seek> Cabinet<R> {
    /// Open an existing cabinet file.
    pub fn new(mut reader: R) -> io::Result<Cabinet<R>> {
        let signature = reader.read_u32::<LittleEndian>()?;
        if signature != consts::FILE_SIGNATURE {
            invalid_data!("Not a cabinet file (invalid file signature)");
        }
        let _reserved1 = reader.read_u32::<LittleEndian>()?;
        let _total_size = reader.read_u32::<LittleEndian>()?;
        let _reserved2 = reader.read_u32::<LittleEndian>()?;
        let first_file_offset = reader.read_u32::<LittleEndian>()?;
        let _reserved3 = reader.read_u32::<LittleEndian>()?;
        let minor_version = reader.read_u8()?;
        let major_version = reader.read_u8()?;
        if major_version > consts::VERSION_MAJOR ||
            major_version == consts::VERSION_MAJOR &&
                minor_version > consts::VERSION_MINOR
        {
            invalid_data!("Version {}.{} cabinet files are not supported",
                          major_version,
                          minor_version);
        }
        let num_folders = reader.read_u16::<LittleEndian>()? as usize;
        let num_files = reader.read_u16::<LittleEndian>()?;
        let flags = reader.read_u16::<LittleEndian>()?;
        let cabinet_set_id = reader.read_u16::<LittleEndian>()?;
        let cabinet_set_index = reader.read_u16::<LittleEndian>()?;
        let mut header_reserve_size = 0u16;
        let mut folder_reserve_size = 0u8;
        let mut data_reserve_size = 0u8;
        if (flags & consts::FLAG_RESERVE_PRESENT) != 0 {
            header_reserve_size = reader.read_u16::<LittleEndian>()?;
            folder_reserve_size = reader.read_u8()?;
            data_reserve_size = reader.read_u8()?;
        }
        let mut header_reserve_data = vec![0u8; header_reserve_size as usize];
        if header_reserve_size > 0 {
            reader.read_exact(&mut header_reserve_data)?;
        }
        let _prev_cabinet = if (flags & consts::FLAG_PREV_CABINET) != 0 {
            let cabinet_name = read_null_terminated_string(&mut reader)?;
            let disk_name = read_null_terminated_string(&mut reader)?;
            Some((cabinet_name, disk_name))
        } else {
            None
        };
        let _next_cabinet = if (flags & consts::FLAG_NEXT_CABINET) != 0 {
            let cabinet_name = read_null_terminated_string(&mut reader)?;
            let disk_name = read_null_terminated_string(&mut reader)?;
            Some((cabinet_name, disk_name))
        } else {
            None
        };
        let mut folders = Vec::<FolderEntry>::with_capacity(num_folders);
        for _ in 0..num_folders {
            let first_data_offset = reader.read_u32::<LittleEndian>()?;
            let num_data_blocks = reader.read_u16::<LittleEndian>()?;
            let compression_bits = reader.read_u16::<LittleEndian>()?;
            let compression_type =
                CompressionType::from_bitfield(compression_bits)?;
            let mut folder_reserve_data =
                vec![0u8; folder_reserve_size as usize];
            if folder_reserve_size > 0 {
                reader.read_exact(&mut folder_reserve_data)?;
            }
            let entry = FolderEntry {
                first_data_block_offset: first_data_offset,
                num_data_blocks: num_data_blocks,
                compression_type: compression_type,
                reserve_data: folder_reserve_data,
                files: Vec::new(),
            };
            folders.push(entry);
        }
        reader.seek(SeekFrom::Start(first_file_offset as u64))?;
        for _ in 0..num_files {
            let uncompressed_size = reader.read_u32::<LittleEndian>()?;
            let uncompressed_offset = reader.read_u32::<LittleEndian>()?;
            let folder_index = reader.read_u16::<LittleEndian>()? as usize;
            if folder_index >= folders.len() {
                invalid_data!("File entry folder index out of bounds");
            }
            let _date = reader.read_u16::<LittleEndian>()?;
            let _time = reader.read_u16::<LittleEndian>()?;
            let attributes = reader.read_u16::<LittleEndian>()?;
            let name = read_null_terminated_string(&mut reader)?;
            let entry = FileEntry {
                name: name,
                uncompressed_size: uncompressed_size,
                uncompressed_offset: uncompressed_offset,
                attributes: attributes,
            };
            folders[folder_index].files.push(entry);
        }
        Ok(Cabinet {
               reader: reader,
               cabinet_set_id: cabinet_set_id,
               cabinet_set_index: cabinet_set_index,
               data_reserve_size: data_reserve_size,
               reserve_data: header_reserve_data,
               folders: folders,
           })
    }

    /// Returns the cabinet set ID for this cabinet (an arbitrary number used
    /// to group together a set of cabinets).
    pub fn cabinet_set_id(&self) -> u16 { self.cabinet_set_id }

    /// Returns this cabinet's (zero-based) index within its cabinet set.
    pub fn cabinet_set_index(&self) -> u16 { self.cabinet_set_index }

    /// Returns the application-defined reserve data stored in the cabinet
    /// header.
    pub fn reserve_data(&self) -> &[u8] { &self.reserve_data }

    /// Returns an iterator over the folder entries in this cabinet.
    pub fn folder_entries(&self) -> FolderEntries {
        FolderEntries { iter: self.folders.iter() }
    }

    /// Returns a reader over the decompressed data for the file in the cabinet
    /// with the given name.
    pub fn read_file(&mut self, name: &str) -> io::Result<FileReader<R>> {
        if let Some((folder_index, offset, size)) = self.find_file(name) {
            let mut folder_reader = self.read_folder(folder_index)?;
            folder_reader.seek(SeekFrom::Start(offset))?;
            Ok(FileReader {
                   reader: folder_reader,
                   offset: 0,
                   size: size,
               })
        } else {
            not_found!("No such file in cabinet: {:?}", name);
        }
    }

    fn find_file(&mut self, name: &str) -> Option<(usize, u64, u64)> {
        for (folder_index, folder_entry) in self.folder_entries().enumerate() {
            for file_entry in folder_entry.file_entries() {
                if file_entry.name() == name {
                    let offset = file_entry.uncompressed_offset as u64;
                    let size = file_entry.uncompressed_size() as u64;
                    return Some((folder_index, offset, size));
                }
            }
        }
        None
    }

    /// Returns a reader over the decompressed data in the specified folder.
    pub fn read_folder(&mut self, index: usize)
                       -> io::Result<FolderReader<R>> {
        if index >= self.folders.len() {
            invalid_input!("Folder index {} is out of range (cabinet has {} \
                            folders)",
                           index,
                           self.folders.len());
        }
        let compression_type = self.folders[index].compression_type();
        let data_reader = self.read_data(index)?;
        let decompressor = match compression_type {
            CompressionType::None => {
                FolderDecompressor::Uncompressed(data_reader)
            }
            CompressionType::MsZip => {
                let mszip_reader = MsZipReader::new(data_reader)?;
                FolderDecompressor::MsZip(mszip_reader)
            }
            CompressionType::Quantum(_, _) => {
                invalid_data!("Quantum decompression is not yet supported.");
            }
            CompressionType::Lzx(_) => {
                invalid_data!("LZX decompression is not yet supported.");
            }
        };
        Ok(FolderReader { decompressor: decompressor })
    }

    /// Returns a reader over the compressed data in the specified folder.
    pub fn read_data(&mut self, index: usize) -> io::Result<DataReader<R>> {
        if index >= self.folders.len() {
            invalid_input!("Folder index {} is out of range (cabinet has {} \
                            folders)",
                           index,
                           self.folders.len());
        }
        let num_data_blocks = self.folders[index].num_data_blocks() as usize;
        let mut data_blocks =
            Vec::<(u64, u64)>::with_capacity(num_data_blocks);
        let mut cumulative_size: u64 = 0;
        let mut offset = self.folders[index].first_data_block_offset as u64;
        for _ in 0..num_data_blocks {
            self.reader.seek(SeekFrom::Start(offset + 4))?;
            let compressed_size =
                self.reader.read_u16::<LittleEndian>().unwrap() as u64;
            cumulative_size += compressed_size;
            offset += 8 + self.data_reserve_size as u64;
            data_blocks.push((cumulative_size, offset));
            offset += compressed_size;
        }
        self.reader.seek(SeekFrom::Start(data_blocks[0].1))?;
        Ok(DataReader {
               reader: &mut self.reader,
               data_blocks: data_blocks,
               current_offset: 0,
               current_block: 0,
           })
    }
}

// ========================================================================= //

/// An iterator over the folder entries in a cabinet.
#[derive(Clone)]
pub struct FolderEntries<'a> {
    iter: std::slice::Iter<'a, FolderEntry>,
}

impl<'a> Iterator for FolderEntries<'a> {
    type Item = &'a FolderEntry;

    fn next(&mut self) -> Option<&'a FolderEntry> { self.iter.next() }

    fn size_hint(&self) -> (usize, Option<usize>) { self.iter.size_hint() }
}

impl<'a> ExactSizeIterator for FolderEntries<'a> {}

// ========================================================================= //

/// Metadata about one folder in a cabinet.
pub struct FolderEntry {
    first_data_block_offset: u32,
    num_data_blocks: u16,
    compression_type: CompressionType,
    reserve_data: Vec<u8>,
    files: Vec<FileEntry>,
}

impl FolderEntry {
    /// Returns the scheme used to compress this folder's data.
    pub fn compression_type(&self) -> CompressionType { self.compression_type }

    /// Returns the number of data blocks used to store this folder's data.
    pub fn num_data_blocks(&self) -> u16 { self.num_data_blocks }

    /// Returns the application-defined reserve data for this folder.
    pub fn reserve_data(&self) -> &[u8] { &self.reserve_data }

    /// Returns an iterator over the file entries in this folder.
    pub fn file_entries(&self) -> FileEntries {
        FileEntries { iter: self.files.iter() }
    }
}

// ========================================================================= //

/// An iterator over the file entries in a folder.
#[derive(Clone)]
pub struct FileEntries<'a> {
    iter: std::slice::Iter<'a, FileEntry>,
}

impl<'a> Iterator for FileEntries<'a> {
    type Item = &'a FileEntry;

    fn next(&mut self) -> Option<&'a FileEntry> { self.iter.next() }

    fn size_hint(&self) -> (usize, Option<usize>) { self.iter.size_hint() }
}

impl<'a> ExactSizeIterator for FileEntries<'a> {}

// ========================================================================= //

/// Metadata about one file stored in a cabinet.
pub struct FileEntry {
    name: String,
    uncompressed_size: u32,
    uncompressed_offset: u32,
    attributes: u16,
}

impl FileEntry {
    /// Returns the name of file.
    pub fn name(&self) -> &str { &self.name }

    /// Returns the total size of the file when decompressed, in bytes.
    pub fn uncompressed_size(&self) -> u32 { self.uncompressed_size }

    /// Returns true if this file has the "read-only" attribute set.
    pub fn is_read_only(&self) -> bool {
        (self.attributes & consts::ATTR_READ_ONLY) != 0
    }

    /// Returns true if this file has the "hidden" attribute set.
    pub fn is_hidden(&self) -> bool {
        (self.attributes & consts::ATTR_HIDDEN) != 0
    }

    /// Returns true if this file has the "system file" attribute set.
    pub fn is_system_file(&self) -> bool {
        (self.attributes & consts::ATTR_SYSTEM) != 0
    }
}

// ========================================================================= //

/// A reader for reading decompressed data from a cabinet file.
pub struct FileReader<'a, R: 'a> {
    reader: FolderReader<'a, R>,
    offset: u64,
    size: u64,
}

impl<'a, R: Read + Seek> Read for FileReader<'a, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        debug_assert!(self.offset <= self.size);
        let bytes_remaining = self.size - self.offset;
        let max_bytes = bytes_remaining.min(buf.len() as u64) as usize;
        if max_bytes == 0 {
            return Ok(0);
        }
        let bytes_read = self.reader.read(&mut buf[0..max_bytes])?;
        self.offset += bytes_read as u64;
        Ok(bytes_read)
    }
}

impl<'a, R: Read + Seek> Seek for FileReader<'a, R> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let new_offset = match pos {
            SeekFrom::Start(offset) => offset as i64,
            SeekFrom::Current(delta) => self.offset as i64 + delta,
            SeekFrom::End(delta) => self.size as i64 + delta,
        };
        if new_offset < 0 || (new_offset as u64) > self.size {
            invalid_input!("Cannot seek to {}, file length is {}",
                           new_offset,
                           self.size);
        }
        self.reader.seek(SeekFrom::Current(new_offset - self.offset as i64))?;
        self.offset = new_offset as u64;
        Ok(self.offset)
    }
}

// ========================================================================= //

/// A reader for reading decompressed data from a cabinet folder.
pub struct FolderReader<'a, R: 'a> {
    decompressor: FolderDecompressor<'a, R>,
}

enum FolderDecompressor<'a, R: 'a> {
    Uncompressed(DataReader<'a, R>),
    MsZip(MsZipReader<DataReader<'a, R>>),
    // TODO: add options for other compression types
}

impl<'a, R: Read + Seek> Read for FolderReader<'a, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self.decompressor {
            FolderDecompressor::Uncompressed(ref mut data_reader) => {
                data_reader.read(buf)
            }
            FolderDecompressor::MsZip(ref mut mszip_reader) => {
                mszip_reader.read(buf)
            }
        }
    }
}

impl<'a, R: Read + Seek> Seek for FolderReader<'a, R> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        match self.decompressor {
            FolderDecompressor::Uncompressed(ref mut data_reader) => {
                data_reader.seek(pos)
            }
            FolderDecompressor::MsZip(ref mut mszip_reader) => {
                mszip_reader.seek(pos)
            }
        }
    }
}

// ========================================================================= //

/// A reader for reading raw, compressed data from a cabinet folder.
pub struct DataReader<'a, R: 'a> {
    reader: &'a mut R,
    data_blocks: Vec<(u64, u64)>,
    current_offset: u64,
    current_block: usize,
}

impl<'a, R: Read + Seek> Read for DataReader<'a, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        debug_assert!(self.current_block <= self.data_blocks.len());
        let bytes_remaining_in_current_block =
            if self.current_block < self.data_blocks.len() {
                debug_assert!(self.current_offset <
                                  self.data_blocks[self.current_block].0);
                self.data_blocks[self.current_block].0 - self.current_offset
            } else {
                0
            };
        let max_bytes = bytes_remaining_in_current_block
            .min(buf.len() as u64) as usize;
        if max_bytes == 0 {
            return Ok(0);
        }
        let bytes_read = self.reader.read(&mut buf[0..max_bytes])?;
        self.current_offset += bytes_read as u64;
        debug_assert!(bytes_read as u64 <= bytes_remaining_in_current_block);
        if bytes_read as u64 == bytes_remaining_in_current_block {
            self.current_block += 1;
            if self.current_block < self.data_blocks.len() {
                let block_offset = self.data_blocks[self.current_block].1;
                self.reader.seek(SeekFrom::Start(block_offset))?;
            }
        }
        Ok(bytes_read)
    }
}

impl<'a, R: Read + Seek> Seek for DataReader<'a, R> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        debug_assert!(!self.data_blocks.is_empty());
        let total_size = self.data_blocks.last().unwrap().0;
        let new_offset = match pos {
            SeekFrom::Start(offset) => offset as i64,
            SeekFrom::Current(delta) => self.current_offset as i64 + delta,
            SeekFrom::End(delta) => total_size as i64 + delta,
        };
        if new_offset < 0 || (new_offset as u64) > total_size {
            invalid_input!("Cannot seek to {}, data length is {}",
                           new_offset,
                           total_size);
        }
        let new_offset = new_offset as u64;
        let new_block =
            match self.data_blocks
                .binary_search_by_key(&new_offset, |&entry| entry.0) {
                Ok(index) => index + 1,
                Err(index) => index,
            };
        if new_block < self.data_blocks.len() {
            let offset_at_block_start = self.data_blocks[new_block].1;
            let size_at_block_start = if new_block > 0 {
                self.data_blocks[new_block - 1].0
            } else {
                0
            };
            debug_assert!(size_at_block_start <= new_offset);
            let offset_within_block = new_offset - size_at_block_start;
            self.reader
                .seek(SeekFrom::Start(offset_at_block_start +
                                          offset_within_block))?;
        } else {
            debug_assert_eq!(new_offset, total_size);
            debug_assert_eq!(new_block, self.data_blocks.len());
        }
        self.current_offset = new_offset;
        self.current_block = new_block;
        Ok(self.current_offset)
    }
}
// ========================================================================= //

fn read_null_terminated_string<R: Read>(reader: &mut R) -> io::Result<String> {
    let mut bytes = Vec::<u8>::new();
    loop {
        let byte = reader.read_u8()?;
        if byte == 0 {
            break;
        }
        bytes.push(byte);
    }
    match String::from_utf8(bytes) {
        Ok(string) => Ok(string),
        Err(_) => invalid_data!("Invalid UTF-8 string"),
    }
}

// ========================================================================= //

#[cfg(test)]
mod tests {
    use super::Cabinet;
    use std::io::{Cursor, Read};

    #[test]
    fn read_uncompressed_cabinet_with_one_file() {
        let binary: &[u8] = b"MSCF\0\0\0\0\x59\0\0\0\0\0\0\0\
            \x2c\0\0\0\0\0\0\0\x03\x01\x01\0\x01\0\0\0\x34\x12\0\0\
            \x43\0\0\0\x01\0\0\0\
            \x0e\0\0\0\0\0\0\0\0\0\x6c\x22\xe7\x59\x01\0hi.txt\0\
            \0\0\0\0\x0e\0\x0e\0Hello, world!\n";
        assert_eq!(binary.len(), 0x59);
        let mut cabinet = Cabinet::new(Cursor::new(binary)).unwrap();
        assert_eq!(cabinet.cabinet_set_id(), 0x1234);
        assert_eq!(cabinet.cabinet_set_index(), 0);
        assert_eq!(cabinet.reserve_data(), &[]);
        assert_eq!(cabinet.folder_entries().len(), 1);

        let mut data = Vec::new();
        cabinet.read_data(0).unwrap().read_to_end(&mut data).unwrap();
        assert_eq!(data, b"Hello, world!\n");

        let mut data = Vec::new();
        cabinet.read_folder(0).unwrap().read_to_end(&mut data).unwrap();
        assert_eq!(data, b"Hello, world!\n");

        let mut data = Vec::new();
        cabinet.read_file("hi.txt").unwrap().read_to_end(&mut data).unwrap();
        assert_eq!(data, b"Hello, world!\n");
    }

    #[test]
    fn read_uncompressed_cabinet_with_two_files() {
        let binary: &[u8] =
            b"MSCF\0\0\0\0\x80\0\0\0\0\0\0\0\
            \x2c\0\0\0\0\0\0\0\x03\x01\x01\0\x02\0\0\0\x34\x12\0\0\
            \x5b\0\0\0\x01\0\0\0\
            \x0e\0\0\0\0\0\0\0\0\0\x6c\x22\xe7\x59\x01\0hi.txt\0\
            \x0f\0\0\0\x0e\0\0\0\0\0\x6c\x22\xe7\x59\x01\0bye.txt\0\
            \0\0\0\0\x1d\0\x1d\0Hello, world!\nSee you later!\n";
        assert_eq!(binary.len(), 0x80);
        let mut cabinet = Cabinet::new(Cursor::new(binary)).unwrap();

        let mut data = Vec::new();
        cabinet.read_folder(0).unwrap().read_to_end(&mut data).unwrap();
        assert_eq!(data, b"Hello, world!\nSee you later!\n");

        let mut data = Vec::new();
        cabinet.read_file("hi.txt").unwrap().read_to_end(&mut data).unwrap();
        assert_eq!(data, b"Hello, world!\n");

        let mut data = Vec::new();
        cabinet.read_file("bye.txt").unwrap().read_to_end(&mut data).unwrap();
        assert_eq!(data, b"See you later!\n");
    }

    #[test]
    fn read_mszip_cabinet_with_one_file() {
        let binary: &[u8] =
            b"MSCF\0\0\0\0\x61\0\0\0\0\0\0\0\
            \x2c\0\0\0\0\0\0\0\x03\x01\x01\0\x01\0\0\0\x34\x12\0\0\
            \x43\0\0\0\x01\0\x01\0\
            \x0e\0\0\0\0\0\0\0\0\0\x6c\x22\xe7\x59\x01\0hi.txt\0\
            \0\0\0\0\x16\0\x0e\0\
            CK\xf3H\xcd\xc9\xc9\xd7Q(\xcf/\xcaIQ\xe4\x02\x00$\xf2\x04\x94";
        assert_eq!(binary.len(), 0x61);
        let mut cabinet = Cabinet::new(Cursor::new(binary)).unwrap();
        assert_eq!(cabinet.cabinet_set_id(), 0x1234);
        assert_eq!(cabinet.cabinet_set_index(), 0);
        assert_eq!(cabinet.reserve_data(), &[]);
        assert_eq!(cabinet.folder_entries().len(), 1);

        let mut data = Vec::new();
        cabinet.read_data(0).unwrap().read_to_end(&mut data).unwrap();
        assert_eq!(data,
                   b"CK\xf3H\xcd\xc9\xc9\xd7Q(\xcf/\xcaIQ\
                     \xe4\x02\x00$\xf2\x04\x94");

        let mut data = Vec::new();
        cabinet.read_folder(0).unwrap().read_to_end(&mut data).unwrap();
        assert_eq!(data, b"Hello, world!\n");

        let mut data = Vec::new();
        cabinet.read_file("hi.txt").unwrap().read_to_end(&mut data).unwrap();
        assert_eq!(data, b"Hello, world!\n");
    }

    #[test]
    fn read_mszip_cabinet_with_two_files() {
        let binary: &[u8] =
            b"MSCF\0\0\0\0\x88\0\0\0\0\0\0\0\
            \x2c\0\0\0\0\0\0\0\x03\x01\x01\0\x02\0\0\0\x34\x12\0\0\
            \x5b\0\0\0\x01\0\x01\0\
            \x0e\0\0\0\0\0\0\0\0\0\x6c\x22\xe7\x59\x01\0hi.txt\0\
            \x0f\0\0\0\x0e\0\0\0\0\0\x6c\x22\xe7\x59\x01\0bye.txt\0\
            \0\0\0\0\x25\0\x1d\0CK\xf3H\xcd\xc9\xc9\xd7Q(\xcf/\xcaIQ\xe4\
            \nNMU\xa8\xcc/U\xc8I,I-R\xe4\x02\x00\x93\xfc\t\x91";
        assert_eq!(binary.len(), 0x88);
        let mut cabinet = Cabinet::new(Cursor::new(binary)).unwrap();

        let mut data = Vec::new();
        cabinet.read_folder(0).unwrap().read_to_end(&mut data).unwrap();
        assert_eq!(data, b"Hello, world!\nSee you later!\n");

        let mut data = Vec::new();
        cabinet.read_file("hi.txt").unwrap().read_to_end(&mut data).unwrap();
        assert_eq!(data, b"Hello, world!\n");

        let mut data = Vec::new();
        cabinet.read_file("bye.txt").unwrap().read_to_end(&mut data).unwrap();
        assert_eq!(data, b"See you later!\n");
    }
}

// ========================================================================= //
