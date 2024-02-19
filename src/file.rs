use std::io::{self, Read, Seek, SeekFrom};
use std::slice;

use byteorder::{LittleEndian, ReadBytesExt};
use time::PrimitiveDateTime;

use crate::consts;
use crate::datetime::datetime_from_bits;
use crate::folder::FolderReader;
use crate::string::read_null_terminated_string;

/// An iterator over the file entries in a folder.
#[derive(Clone)]
pub struct FileEntries<'a> {
    pub(crate) iter: slice::Iter<'a, FileEntry>,
}

/// Metadata about one file stored in a cabinet.
#[derive(Debug, Clone)]
pub struct FileEntry {
    name: String,
    datetime: Option<PrimitiveDateTime>,
    uncompressed_size: u32,
    attributes: u16,
    pub(crate) folder_index: u16,
    pub(crate) uncompressed_offset: u32,
}

/// A reader for reading decompressed data from a cabinet file.
pub struct FileReader<'a, R: 'a> {
    pub(crate) reader: FolderReader<'a, R>,
    pub(crate) file_start_in_folder: u64,
    pub(crate) offset: u64,
    pub(crate) size: u64,
}

impl<'a> Iterator for FileEntries<'a> {
    type Item = &'a FileEntry;

    fn next(&mut self) -> Option<&'a FileEntry> {
        self.iter.next()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.iter.size_hint()
    }
}

impl<'a> ExactSizeIterator for FileEntries<'a> {}

impl FileEntry {
    /// Returns the name of file.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the datetime for this file.  According to the CAB spec, this
    /// "is typically considered the 'last modified' time in local time, but
    /// the actual definition is application-defined."
    ///
    /// Note that this will return [`None`] if the datetime in the cabinet file
    /// was not a valid date/time.
    pub fn datetime(&self) -> Option<PrimitiveDateTime> {
        self.datetime
    }

    /// Returns the total size of the file when decompressed, in bytes.
    pub fn uncompressed_size(&self) -> u32 {
        self.uncompressed_size
    }

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

impl<'a, R: Read + Seek> Read for FileReader<'a, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        debug_assert!(self.offset <= self.size);
        let bytes_remaining = self.size - self.offset;
        let max_bytes = bytes_remaining.min(buf.len() as u64) as usize;
        if max_bytes == 0 {
            return Ok(0);
        }
        let bytes_read = self.reader.read(&mut buf[..max_bytes])?;
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
            invalid_input!(
                "Cannot seek to {}, file length is {}",
                new_offset,
                self.size
            );
        }
        let new_offset = new_offset as u64;
        self.reader.seek_to_uncompressed_offset(
            self.file_start_in_folder + new_offset,
        )?;
        self.offset = new_offset;
        Ok(new_offset)
    }
}

pub(crate) fn parse_file_entry<R: Read>(
    mut reader: R,
) -> io::Result<FileEntry> {
    let uncompressed_size = reader.read_u32::<LittleEndian>()?;
    let uncompressed_offset = reader.read_u32::<LittleEndian>()?;
    let folder_index = reader.read_u16::<LittleEndian>()?;
    let date = reader.read_u16::<LittleEndian>()?;
    let time = reader.read_u16::<LittleEndian>()?;
    let datetime = datetime_from_bits(date, time);
    let attributes = reader.read_u16::<LittleEndian>()?;
    let is_utf8 = (attributes & consts::ATTR_NAME_IS_UTF) != 0;
    let name = read_null_terminated_string(&mut reader, is_utf8)?;
    let entry = FileEntry {
        name,
        folder_index,
        datetime,
        uncompressed_size,
        uncompressed_offset,
        attributes,
    };
    Ok(entry)
}
