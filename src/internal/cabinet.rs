use byteorder::{LittleEndian, ReadBytesExt};
use chrono::NaiveDateTime;
use crate::internal::checksum::Checksum;
use crate::internal::consts;
use crate::internal::ctype::CompressionType;
use crate::internal::datetime::datetime_from_bits;
use crate::internal::mszip::MsZipDecompressor;
use lzxd::Lzxd;
use std::io::{self, Read, Seek, SeekFrom};
use std::slice;

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
        let total_size = reader.read_u32::<LittleEndian>()?;
        if total_size > consts::MAX_TOTAL_CAB_SIZE {
            invalid_data!("Cabinet total size field is too large \
                           ({} bytes; max is {} bytes)",
                          total_size,
                          consts::MAX_TOTAL_CAB_SIZE);
        }
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
            let cab_name = read_null_terminated_string(&mut reader, false)?;
            let disk_name = read_null_terminated_string(&mut reader, false)?;
            Some((cab_name, disk_name))
        } else {
            None
        };
        let _next_cabinet = if (flags & consts::FLAG_NEXT_CABINET) != 0 {
            let cab_name = read_null_terminated_string(&mut reader, false)?;
            let disk_name = read_null_terminated_string(&mut reader, false)?;
            Some((cab_name, disk_name))
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
            let date = reader.read_u16::<LittleEndian>()?;
            let time = reader.read_u16::<LittleEndian>()?;
            let datetime = datetime_from_bits(date, time);
            let attributes = reader.read_u16::<LittleEndian>()?;
            let is_utf8 = (attributes & consts::ATTR_NAME_IS_UTF) != 0;
            let name = read_null_terminated_string(&mut reader, is_utf8)?;
            let entry = FileEntry {
                name: name,
                datetime: datetime,
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

    /// Returns the entry for the file with the given name, if any..
    pub fn get_file_entry(&self, name: &str) -> Option<&FileEntry> {
        for folder in self.folder_entries() {
            for file in folder.file_entries() {
                if file.name() == name {
                    return Some(file);
                }
            }
        }
        None
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
    fn read_folder(&mut self, index: usize) -> io::Result<FolderReader<R>> {
        if index >= self.folders.len() {
            invalid_input!("Folder index {} is out of range (cabinet has {} \
                            folders)",
                           index,
                           self.folders.len());
        }
        FolderReader::new(&mut self.reader,
                          &self.folders[index],
                          self.data_reserve_size)
    }
}

// ========================================================================= //

/// An iterator over the folder entries in a cabinet.
#[derive(Clone)]
pub struct FolderEntries<'a> {
    iter: slice::Iter<'a, FolderEntry>,
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
    iter: slice::Iter<'a, FileEntry>,
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
    datetime: Option<NaiveDateTime>,
    uncompressed_size: u32,
    uncompressed_offset: u32,
    attributes: u16,
}

impl FileEntry {
    /// Returns the name of file.
    pub fn name(&self) -> &str { &self.name }

    /// Returns the datetime for this file.  According to the CAB spec, this
    /// "is typically considered the 'last modified' time in local time, but
    /// the actual definition is application-defined".
    /// Note that this will return [None] if the datetime in the cabinet file
    /// was not a valid date/time.
    pub fn datetime(&self) -> Option<NaiveDateTime> { self.datetime }

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
    pub fn is_system(&self) -> bool {
        (self.attributes & consts::ATTR_SYSTEM) != 0
    }

    /// Returns true if this file has the "archive" (modified since last
    /// backup) attribute set.
    pub fn is_archive(&self) -> bool {
        (self.attributes & consts::ATTR_ARCH) != 0
    }

    /// Returns true if this file has the "execute after extraction" attribute
    /// set.
    pub fn is_exec(&self) -> bool {
        (self.attributes & consts::ATTR_EXEC) != 0
    }

    /// Returns true if this file has the "name is UTF" attribute set.
    pub fn is_name_utf(&self) -> bool {
        (self.attributes & consts::ATTR_NAME_IS_UTF) != 0
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
struct FolderReader<'a, R: 'a> {
    reader: &'a mut R,
    decompressor: FolderDecompressor,
    data_reserve_size: usize,
    data_blocks: Vec<(u64, u64)>,
    current_block_index: usize,
    current_block_data: Vec<u8>,
    current_offset_within_block: usize,
    current_offset_within_folder: u64,
}

enum FolderDecompressor {
    Uncompressed,
    MsZip(MsZipDecompressor),
    Lzx(Lzxd),
    // TODO: add options for other compression types
}

impl<'a, R: 'a + Read + Seek> FolderReader<'a, R> {
    fn new(reader: &'a mut R, entry: &FolderEntry, data_reserve_size: u8)
           -> io::Result<FolderReader<'a, R>> {
        let num_data_blocks = entry.num_data_blocks() as usize;
        let mut data_blocks =
            Vec::<(u64, u64)>::with_capacity(num_data_blocks);
        let mut cumulative_size: u64 = 0;
        let mut offset = entry.first_data_block_offset as u64;
        for _ in 0..num_data_blocks {
            reader.seek(SeekFrom::Start(offset + 4))?;
            let compressed_size = reader.read_u16::<LittleEndian>()? as u64;
            let uncompressed_size = reader.read_u16::<LittleEndian>()? as u64;
            cumulative_size += uncompressed_size;
            data_blocks.push((cumulative_size, offset));
            offset += 8 + data_reserve_size as u64 + compressed_size;
        }
        let decompressor = match entry.compression_type() {
            CompressionType::None => FolderDecompressor::Uncompressed,
            CompressionType::MsZip => {
                FolderDecompressor::MsZip(MsZipDecompressor::new())
            }
            CompressionType::Quantum(_, _) => {
                invalid_data!("Quantum decompression is not yet supported.");
            }
            CompressionType::Lzx(window_size) => {
                FolderDecompressor::Lzx(Lzxd::new(match window_size {
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

                    _ => invalid_data!("LZX given with invalid window size")
                }))
            }
        };
        let mut folder_reader = FolderReader {
            reader: reader,
            decompressor: decompressor,
            data_reserve_size: data_reserve_size as usize,
            data_blocks: data_blocks,
            current_block_index: 0,
            current_block_data: Vec::new(),
            current_offset_within_block: 0,
            current_offset_within_folder: 0,
        };
        folder_reader.load_block()?;
        Ok(folder_reader)
    }

    fn total_size(&self) -> u64 {
        match self.data_blocks.last() {
            Some(&(cumulative_size, _)) => cumulative_size,
            None => 0,
        }
    }

    fn current_block_start(&self) -> u64 {
        if self.current_block_index == 0 {
            0
        } else {
            self.data_blocks[self.current_block_index - 1].0
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
        let offset = self.data_blocks[self.current_block_index].1;
        self.reader.seek(SeekFrom::Start(offset))?;
        let expected_checksum = self.reader.read_u32::<LittleEndian>()?;
        let compressed_size = self.reader.read_u16::<LittleEndian>()?;
        let uncompressed_size = self.reader.read_u16::<LittleEndian>()?;
        let mut reserve_data = vec![0u8; self.data_reserve_size];
        self.reader.read_exact(&mut reserve_data)?;
        let mut compressed_data = vec![0u8; compressed_size as usize];
        self.reader.read_exact(&mut compressed_data)?;
        if expected_checksum != 0 {
            let mut checksum = Checksum::new();
            checksum.append(&reserve_data);
            checksum.append(&compressed_data);
            let actual_checksum = checksum.value() ^
                ((compressed_size as u32) |
                    ((uncompressed_size as u32) << 16));
            if actual_checksum != expected_checksum {
                invalid_data!("Checksum error in data block {} \
                               (expected {:08x}, actual {:08x})",
                              self.current_block_index,
                              expected_checksum,
                              actual_checksum);
            }
        }
        self.current_block_data = match self.decompressor {
            FolderDecompressor::Uncompressed => compressed_data,
            FolderDecompressor::MsZip(ref mut decompressor) => {
                decompressor
                    .decompress_block(&compressed_data, uncompressed_size)?
            }
            FolderDecompressor::Lzx(ref mut decompressor) => {
                decompressor.decompress_next(&compressed_data)
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?.to_vec()
            }
        };
        Ok(())
    }
}

impl<'a, R: Read + Seek> Read for FolderReader<'a, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() ||
            self.current_block_index >= self.data_blocks.len()
        {
            return Ok(0);
        }
        if self.current_offset_within_block == self.current_block_data.len() {
            self.current_block_index += 1;
            self.current_offset_within_block = 0;
            self.load_block()?;
        }
        let max_bytes =
            buf.len().min(self.current_block_data.len() -
                              self.current_offset_within_block);
        buf[..max_bytes]
            .copy_from_slice(&self.current_block_data
                                 [self.current_offset_within_block..]
                                 [..max_bytes]);
        self.current_offset_within_block += max_bytes;
        self.current_offset_within_folder += max_bytes as u64;
        Ok(max_bytes)
    }
}

impl<'a, R: Read + Seek> Seek for FolderReader<'a, R> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let total_size = self.total_size();
        let new_offset = match pos {
            SeekFrom::Start(offset) => offset as i64,
            SeekFrom::Current(delta) => {
                self.current_offset_within_folder as i64 + delta
            }
            SeekFrom::End(delta) => total_size as i64 + delta,
        };
        if new_offset < 0 || (new_offset as u64) > total_size {
            invalid_input!("Cannot seek to {}, data length is {}",
                           new_offset,
                           total_size);
        }
        let new_offset = new_offset as u64;
        if new_offset < self.current_block_start() {
            self.rewind()?;
        }
        // TODO: If folder is uncompressed, we should just jump straight to the
        // correct block without "decompressing" those in between.
        while self.data_blocks[self.current_block_index].0 < new_offset {
            self.current_block_index += 1;
            self.load_block()?;
        }
        debug_assert!(new_offset >= self.current_block_start());
        self.current_offset_within_block =
            (new_offset - self.current_block_start()) as usize;
        self.current_offset_within_folder = new_offset;
        Ok(new_offset)
    }
}

// ========================================================================= //

fn read_null_terminated_string<R: Read>(reader: &mut R, _is_utf8: bool)
                                        -> io::Result<String> {
    let mut bytes = Vec::<u8>::with_capacity(consts::MAX_STRING_SIZE);
    loop {
        let byte = reader.read_u8()?;
        if byte == 0 {
            break;
        } else if bytes.len() == consts::MAX_STRING_SIZE {
            invalid_data!("String longer than maximum of {} bytes",
                          consts::MAX_STRING_SIZE);
        }
        bytes.push(byte);
    }
    // TODO: Handle decoding differently depending on `_is_utf8`.
    Ok(String::from_utf8_lossy(&bytes).to_string())
}

// ========================================================================= //

#[cfg(test)]
mod tests {
    use super::Cabinet;
    use chrono::{Datelike, Timelike};
    use std::io::{Cursor, Read};

    #[test]
    fn read_uncompressed_cabinet_with_one_file() {
        let binary: &[u8] = b"MSCF\0\0\0\0\x59\0\0\0\0\0\0\0\
            \x2c\0\0\0\0\0\0\0\x03\x01\x01\0\x01\0\0\0\x34\x12\0\0\
            \x43\0\0\0\x01\0\0\0\
            \x0e\0\0\0\0\0\0\0\0\0\x6c\x22\xba\x59\x01\0hi.txt\0\
            \x4c\x1a\x2e\x7f\x0e\0\x0e\0Hello, world!\n";
        assert_eq!(binary.len(), 0x59);
        let mut cabinet = Cabinet::new(Cursor::new(binary)).unwrap();
        assert_eq!(cabinet.cabinet_set_id(), 0x1234);
        assert_eq!(cabinet.cabinet_set_index(), 0);
        assert_eq!(cabinet.reserve_data(), &[]);
        assert_eq!(cabinet.folder_entries().len(), 1);
        {
            let file = cabinet.get_file_entry("hi.txt").unwrap();
            assert_eq!(file.name(), "hi.txt");
            assert!(!file.is_name_utf());
            let dt = file.datetime().unwrap();

            assert_eq!(dt.year(), 1997);
            assert_eq!(dt.month(), 3);
            assert_eq!(dt.day(), 12);
            assert_eq!(dt.hour(), 11);
            assert_eq!(dt.minute(), 13);
            assert_eq!(dt.second(), 52);
        }

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
    fn read_uncompressed_cabinet_with_two_data_blocks() {
        let binary: &[u8] = b"MSCF\0\0\0\0\x61\0\0\0\0\0\0\0\
            \x2c\0\0\0\0\0\0\0\x03\x01\x01\0\x01\0\0\0\x34\x12\0\0\
            \x43\0\0\0\x02\0\0\0\
            \x0e\0\0\0\0\0\0\0\0\0\x6c\x22\xba\x59\x01\0hi.txt\0\
            \0\0\0\0\x06\0\x06\0Hello,\
            \0\0\0\0\x08\0\x08\0 world!\n";
        assert_eq!(binary.len(), 0x61);
        let mut cabinet = Cabinet::new(Cursor::new(binary)).unwrap();
        assert_eq!(cabinet.folder_entries().len(), 1);
        assert_eq!(cabinet.folder_entries().nth(0).unwrap().num_data_blocks(),
                   2);

        let mut data = Vec::new();
        cabinet.read_folder(0).unwrap().read_to_end(&mut data).unwrap();
        assert_eq!(data, b"Hello, world!\n");

        let mut data = Vec::new();
        cabinet.read_file("hi.txt").unwrap().read_to_end(&mut data).unwrap();
        assert_eq!(data, b"Hello, world!\n");
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

    #[test]
    fn read_lzx_cabinet_with_two_files() {
        let binary: &[u8] =
            b"\x4d\x53\x43\x46\x00\x00\x00\x00\x97\x00\x00\x00\x00\x00\x00\
            \x00\x2c\x00\x00\x00\x00\x00\x00\x00\x03\x01\x01\x00\x02\x00\
            \x00\x00\x2d\x05\x00\x00\x5b\x00\x00\x00\x01\x00\x03\x13\x0f\
            \x00\x00\x00\x00\x00\x00\x00\x00\x00\x21\x53\x0d\xb2\x20\x00\
            \x68\x69\x2e\x74\x78\x74\x00\x10\x00\x00\x00\x0f\x00\x00\x00\
            \x00\x00\x21\x53\x0b\xb2\x20\x00\x62\x79\x65\x2e\x74\x78\x74\
            \x00\x5c\xef\x2a\xc7\x34\x00\x1f\x00\x5b\x80\x80\x8d\x00\x30\
            \xf0\x01\x10\x00\x00\x00\x01\x00\x00\x00\x01\x00\x00\x00\x48\
            \x65\x6c\x6c\x6f\x2c\x20\x77\x6f\x72\x6c\x64\x21\x0d\x0a\x53\
            \x65\x65\x20\x79\x6f\x75\x20\x6c\x61\x74\x65\x72\x21\x0d\x0a\
            \x00";
        assert_eq!(binary.len(), 0x97);
        let mut cabinet = Cabinet::new(Cursor::new(binary)).unwrap();

        let mut data = Vec::new();
        cabinet.read_folder(0).unwrap().read_to_end(&mut data).unwrap();
        assert_eq!(data, b"Hello, world!\r\nSee you later!\r\n");

        let mut data = Vec::new();
        cabinet.read_file("hi.txt").unwrap().read_to_end(&mut data).unwrap();
        assert_eq!(data, b"Hello, world!\r\n");

        let mut data = Vec::new();
        cabinet.read_file("bye.txt").unwrap().read_to_end(&mut data).unwrap();
        assert_eq!(data, b"See you later!\r\n");
    }

    #[test]
    fn read_uncompressed_cabinet_with_non_ascii_filename() {
        let binary: &[u8] = b"MSCF\0\0\0\0\x55\0\0\0\0\0\0\0\
            \x2c\0\0\0\0\0\0\0\x03\x01\x01\0\x01\0\0\0\0\0\0\0\
            \x44\0\0\0\x01\0\0\0\
            \x09\0\0\0\0\0\0\0\0\0\x6c\x22\xba\x59\xa0\0\xe2\x98\x83.txt\0\
            \x3d\x0f\x08\x56\x09\0\x09\0Snowman!\n";
        assert_eq!(binary.len(), 0x55);
        let mut cabinet = Cabinet::new(Cursor::new(binary)).unwrap();
        {
            let file_entry = cabinet.get_file_entry("\u{2603}.txt").unwrap();
            assert_eq!(file_entry.name(), "\u{2603}.txt");
            assert!(file_entry.is_name_utf());
        }
        {
            let mut file_reader = cabinet.read_file("\u{2603}.txt").unwrap();
            let mut data = Vec::new();
            file_reader.read_to_end(&mut data).unwrap();
            assert_eq!(data, b"Snowman!\n");
        }
    }
}

// ========================================================================= //
