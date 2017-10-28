//! A library for reading/writing [Windows
//! cabinet](https://en.wikipedia.org/wiki/Cabinet_(file_format)) (CAB) files.

#![warn(missing_docs)]

extern crate byteorder;

use byteorder::{LittleEndian, ReadBytesExt};
use std::io::{self, Read, Seek, SeekFrom};

// ========================================================================= //

macro_rules! invalid_data {
    ($e:expr) => {
        return Err(::std::io::Error::new(::std::io::ErrorKind::InvalidData,
                                         $e))
    };
    ($fmt:expr, $($arg:tt)+) => {
        return Err(::std::io::Error::new(::std::io::ErrorKind::InvalidData,
                                         format!($fmt, $($arg)+)))
    };
}

// ========================================================================= //

const FILE_SIGNATURE: u32 = 0x4643534d; // "MSCF" stored little-endian

const VERSION_MAJOR: u8 = 1;
const VERSION_MINOR: u8 = 3;

const FLAG_PREV_CABINET: u16 = 0x1;
const FLAG_NEXT_CABINET: u16 = 0x2;
const FLAG_RESERVE_PRESENT: u16 = 0x4;

// Compression type constants:
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

// File attributes:
const ATTR_READ_ONLY: u16 = 0x01;
const ATTR_HIDDEN: u16 = 0x02;
const ATTR_SYSTEM: u16 = 0x04;

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
    fn from_bitfield(bits: u16) -> io::Result<CompressionType> {
        let ctype = bits & 0x000f;
        if ctype == CTYPE_NONE {
            Ok(CompressionType::None)
        } else if ctype == CTYPE_MSZIP {
            Ok(CompressionType::MsZip)
        } else if ctype == CTYPE_QUANTUM {
            let level = (bits & 0x00f0) >> 4;
            if level < QUANTUM_LEVEL_MIN || level > QUANTUM_LEVEL_MAX {
                invalid_data!("Invalid Quantum level: 0x{:02x}", level);
            }
            let memory = (bits & 0x1f00) >> 8;
            if memory < QUANTUM_MEMORY_MIN || memory > QUANTUM_MEMORY_MAX {
                invalid_data!("Invalid Quantum memory: 0x{:02x}", memory);
            }
            Ok(CompressionType::Quantum(level, memory))
        } else if ctype == CTYPE_LZX {
            let window = (bits & 0x1f00) >> 8;
            if window < LZX_WINDOW_MIN || window > LZX_WINDOW_MAX {
                invalid_data!("Invalid LZX window: 0x{:02x}", window);
            }
            Ok(CompressionType::Lzx(window))
        } else {
            invalid_data!("Invalid compression type: 0x{:04x}", bits);
        }
    }

    #[allow(dead_code)]
    fn to_bitfield(&self) -> u16 {
        match *self {
            CompressionType::None => CTYPE_NONE,
            CompressionType::MsZip => CTYPE_MSZIP,
            CompressionType::Quantum(level, memory) => {
                CTYPE_QUANTUM |
                    (level.max(QUANTUM_LEVEL_MIN).min(QUANTUM_LEVEL_MAX) <<
                        4) |
                    (memory.max(QUANTUM_MEMORY_MIN).min(QUANTUM_MEMORY_MAX) <<
                         8)
            }
            CompressionType::Lzx(window) => {
                CTYPE_LZX |
                    (window.max(LZX_WINDOW_MIN).min(LZX_WINDOW_MAX) << 8)
            }
        }
    }
}

// ========================================================================= //

/// A structure for reading a cabinet file.
#[allow(dead_code)]
pub struct Cabinet<R> {
    reader: R,
    cabinet_set_id: u16,
    cabinet_set_index: u16,
    reserve_data: Vec<u8>,
    folders: Vec<FolderEntry>,
}

impl<R: Read + Seek> Cabinet<R> {
    /// Open an existing cabinet file.
    pub fn new(mut reader: R) -> io::Result<Cabinet<R>> {
        let signature = reader.read_u32::<LittleEndian>()?;
        if signature != FILE_SIGNATURE {
            invalid_data!("Not a cabinet file (invalid file signature)");
        }
        let _reserved1 = reader.read_u32::<LittleEndian>()?;
        let _total_size = reader.read_u32::<LittleEndian>()?;
        let _reserved2 = reader.read_u32::<LittleEndian>()?;
        let first_file_offset = reader.read_u32::<LittleEndian>()?;
        let _reserved3 = reader.read_u32::<LittleEndian>()?;
        let minor_version = reader.read_u8()?;
        let major_version = reader.read_u8()?;
        if major_version > VERSION_MAJOR ||
            major_version == VERSION_MAJOR && minor_version > VERSION_MINOR
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
        let mut _data_reserve_size = 0u8;
        if (flags & FLAG_RESERVE_PRESENT) != 0 {
            header_reserve_size = reader.read_u16::<LittleEndian>()?;
            folder_reserve_size = reader.read_u8()?;
            _data_reserve_size = reader.read_u8()?;
        }
        let mut header_reserve_data = vec![0u8; header_reserve_size as usize];
        if header_reserve_size > 0 {
            reader.read_exact(&mut header_reserve_data)?;
        }
        let _prev_cabinet = if (flags & FLAG_PREV_CABINET) != 0 {
            let cabinet_name = read_null_terminated_string(&mut reader)?;
            let disk_name = read_null_terminated_string(&mut reader)?;
            Some((cabinet_name, disk_name))
        } else {
            None
        };
        let _next_cabinet = if (flags & FLAG_NEXT_CABINET) != 0 {
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
            let _uncompressed_offset = reader.read_u32::<LittleEndian>()?;
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
                attributes: attributes,
            };
            folders[folder_index].files.push(entry);
        }
        Ok(Cabinet {
               reader: reader,
               cabinet_set_id: cabinet_set_id,
               cabinet_set_index: cabinet_set_index,
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
#[allow(dead_code)]
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
    attributes: u16,
}

impl FileEntry {
    /// Returns the name of file.
    pub fn name(&self) -> &str { &self.name }

    /// Returns the total size of the file when decompressed, in bytes.
    pub fn uncompressed_size(&self) -> u32 { self.uncompressed_size }

    /// Returns true if this file has the "read-only" attribute set.
    pub fn is_read_only(&self) -> bool {
        (self.attributes & ATTR_READ_ONLY) != 0
    }

    /// Returns true if this file has the "hidden" attribute set.
    pub fn is_hidden(&self) -> bool { (self.attributes & ATTR_HIDDEN) != 0 }

    /// Returns true if this file has the "system file" attribute set.
    pub fn is_system_file(&self) -> bool {
        (self.attributes & ATTR_SYSTEM) != 0
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
        assert_eq!(CompressionType::from_bitfield(0x0).unwrap(),
                   CompressionType::None);
        assert_eq!(CompressionType::from_bitfield(0x1).unwrap(),
                   CompressionType::MsZip);
        assert_eq!(CompressionType::from_bitfield(0x1472).unwrap(),
                   CompressionType::Quantum(7, 20));
        assert_eq!(CompressionType::from_bitfield(0x1503).unwrap(),
                   CompressionType::Lzx(21));
    }
}

// ========================================================================= //
