use std::io::Read;
use std::{io, slice};

use byteorder::{LittleEndian, ReadBytesExt};
use time::PrimitiveDateTime;

use crate::consts;
use crate::datetime::datetime_from_bits;
use crate::folder::FolderReader;
use crate::string::read_null_terminated_string;

/// An iterator over the file entries in a folder.
#[derive(Clone)]
pub struct FileEntries<'a> {
    pub(crate) reader: &'a FolderReader<'a>,
    pub(crate) iter: slice::Iter<'a, FileEntry>,
}

/// Metadata about one file stored in a cabinet.
#[derive(Clone)]
pub struct FileEntry {
    name: String,
    pub(crate) folder_index: u16,
    datetime: Option<PrimitiveDateTime>,
    uncompressed_size: u32,
    // uncompressed_offset: u32,
    attributes: u16,
}

/// A reader for reading decompressed data from a cabinet file.
#[derive(Clone, Copy)]
pub struct FileReader<'a> {
    reader: &'a FolderReader<'a>,
    entry: &'a FileEntry,
    offset: u64,
    size: u64,
}

impl<'a> Iterator for FileEntries<'a> {
    type Item = FileReader<'a>;

    fn next(&mut self) -> Option<FileReader<'a>> {
        let entry = self.iter.next()?;
        Some(FileReader {
            reader: self.reader,
            entry,
            offset: 0,
            size: entry.uncompressed_size as u64,
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.iter.size_hint()
    }
}

impl<'a> ExactSizeIterator for FileEntries<'a> {}

impl<'a> FileReader<'a> {
    /// Returns the name of file.
    pub fn name(&self) -> &str {
        &self.entry.name
    }

    /// Returns folder index.
    pub fn folder_index(&self) -> u16 {
        self.entry.folder_index
    }

    /// Returns the datetime for this file.  According to the CAB spec, this
    /// "is typically considered the 'last modified' time in local time, but
    /// the actual definition is application-defined."
    ///
    /// Note that this will return [`None`] if the datetime in the cabinet file
    /// was not a valid date/time.
    pub fn datetime(&self) -> Option<PrimitiveDateTime> {
        self.entry.datetime
    }

    /// Returns the total size of the file when decompressed, in bytes.
    pub fn uncompressed_size(&self) -> u32 {
        self.entry.uncompressed_size
    }

    /// Returns true if this file has the "read-only" attribute set.
    pub fn is_read_only(&self) -> bool {
        (self.entry.attributes & consts::ATTR_READ_ONLY) != 0
    }

    /// Returns true if this file has the "hidden" attribute set.
    pub fn is_hidden(&self) -> bool {
        (self.entry.attributes & consts::ATTR_HIDDEN) != 0
    }

    /// Returns true if this file has the "system file" attribute set.
    pub fn is_system(&self) -> bool {
        (self.entry.attributes & consts::ATTR_SYSTEM) != 0
    }

    /// Returns true if this file has the "archive" (modified since last
    /// backup) attribute set.
    pub fn is_archive(&self) -> bool {
        (self.entry.attributes & consts::ATTR_ARCH) != 0
    }

    /// Returns true if this file has the "execute after extraction" attribute
    /// set.
    pub fn is_exec(&self) -> bool {
        (self.entry.attributes & consts::ATTR_EXEC) != 0
    }

    /// Returns true if this file has the "name is UTF" attribute set.
    pub fn is_name_utf(&self) -> bool {
        (self.entry.attributes & consts::ATTR_NAME_IS_UTF) != 0
    }
}

impl<'a> Read for FileReader<'a> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let bytes_remaining = self.size - self.offset;
        let max_bytes = bytes_remaining.min(buf.len() as u64) as usize;
        if max_bytes == 0 {
            return Ok(0);
        }

        let reader = &mut &self.reader.inner;
        let bytes_read = reader.read(&mut buf[0..max_bytes])?;
        self.offset += bytes_read as u64;
        Ok(bytes_read)
    }
}

pub(crate) fn parse_file_entry<R: Read>(
    mut reader: R,
) -> io::Result<FileEntry> {
    let uncompressed_size = reader.read_u32::<LittleEndian>()?;
    let _uncompressed_offset = reader.read_u32::<LittleEndian>()?;
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
        // uncompressed_offset,
        attributes,
    };
    Ok(entry)
}
