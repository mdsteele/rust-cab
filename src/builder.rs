use crate::checksum::Checksum;
use crate::consts;
use crate::ctype::CompressionType;
use crate::datetime::datetime_to_bits;
use crate::mszip::MsZipCompressor;
use byteorder::{LittleEndian, WriteBytesExt};
use std::io::{self, Seek, SeekFrom, Write};
use std::mem;
use time::PrimitiveDateTime;

const MAX_UNCOMPRESSED_BLOCK_SIZE: usize = 0x8000;

/// A structure for building a file within a new cabinet.
pub struct FileBuilder {
    name: String,
    attributes: u16,
    datetime: PrimitiveDateTime,
    entry_offset: u64,
    uncompressed_size: u32,
    offset_within_folder: u32,
}

impl FileBuilder {
    fn new(name: String) -> FileBuilder {
        let name_is_utf = name.bytes().any(|byte| byte > 0x7f);
        let now = time::OffsetDateTime::now_utc();

        let mut builder = FileBuilder {
            name,
            attributes: consts::ATTR_ARCH,
            datetime: time::PrimitiveDateTime::new(now.date(), now.time()),
            entry_offset: 0, // filled in later by CabinetWriter
            uncompressed_size: 0, // filled in later by FileWriter
            offset_within_folder: 0, // filled in later by CabinetWriter
        };
        builder.set_attribute(consts::ATTR_NAME_IS_UTF, name_is_utf);
        builder
    }

    /// Sets the datetime for this file.  According to the CAB spec, this "is
    /// typically considered the 'last modified' time in local time, but the
    /// actual definition is application-defined".
    ///
    /// The CAB file format only supports storing datetimes with years from
    /// 1980 to 2107 (inclusive), with a resolution of two seconds.  If the
    /// given datetime is outside this range/resolution, it will be
    /// clamped/rounded to the nearest legal value.
    ///
    /// By default, the datetime of a new `FileBuilder` is the current UTC
    /// date/time.
    pub fn set_datetime(&mut self, datetime: PrimitiveDateTime) {
        self.datetime = datetime;
    }

    /// Sets whether this file has the "read-only" attribute set.  This
    /// attribute is false by default.
    pub fn set_is_read_only(&mut self, is_read_only: bool) {
        self.set_attribute(consts::ATTR_READ_ONLY, is_read_only);
    }

    /// Sets whether this file has the "hidden" attribute set.  This attribute
    /// is false by default.
    pub fn set_is_hidden(&mut self, is_hidden: bool) {
        self.set_attribute(consts::ATTR_HIDDEN, is_hidden);
    }

    /// Sets whether this file has the "system file" attribute set.  This
    /// attribute is false by default.
    pub fn set_is_system(&mut self, is_system_file: bool) {
        self.set_attribute(consts::ATTR_SYSTEM, is_system_file);
    }

    /// Sets whether this file has the "archive" (modified since last backup)
    /// attribute set.  This attribute is true by default.
    pub fn set_is_archive(&mut self, is_archive: bool) {
        self.set_attribute(consts::ATTR_ARCH, is_archive);
    }

    /// Returns true if this file has the "execute after extraction" attribute
    /// set.  This attribute is false by default.
    pub fn set_is_exec(&mut self, is_exec: bool) {
        self.set_attribute(consts::ATTR_EXEC, is_exec);
    }

    fn set_attribute(&mut self, bit: u16, enable: bool) {
        if enable {
            self.attributes |= bit;
        } else {
            self.attributes &= !bit;
        }
    }
}

/// A structure for building a folder within a new cabinet.
pub struct FolderBuilder {
    compression_type: CompressionType,
    files: Vec<FileBuilder>,
    reserve_data: Vec<u8>,
    entry_offset: u32,
}

impl FolderBuilder {
    fn new(ctype: CompressionType) -> FolderBuilder {
        FolderBuilder {
            compression_type: ctype,
            files: Vec::new(),
            reserve_data: Vec::new(),
            entry_offset: 0, // filled in later by CabinetWriter
        }
    }

    /// Adds a new file to the folder.  You can optionally use the returned
    /// `FileBuilder` to change settings on the file.
    pub fn add_file<S: Into<String>>(&mut self, name: S) -> &mut FileBuilder {
        self.files.push(FileBuilder::new(name.into()));
        self.files.last_mut().unwrap()
    }

    /// Sets the folder's reserve data.  The meaning of this data is
    /// application-defined.  The data must be no more than 255 bytes long.
    pub fn set_reserve_data(&mut self, data: Vec<u8>) {
        self.reserve_data = data;
    }
}

/// A structure for building a new cabinet.
pub struct CabinetBuilder {
    folders: Vec<FolderBuilder>,
    reserve_data: Vec<u8>,
}

impl CabinetBuilder {
    /// Creates a new, empty `CabinetBuilder`.
    pub fn new() -> CabinetBuilder {
        CabinetBuilder { folders: Vec::new(), reserve_data: Vec::new() }
    }

    /// Adds a new folder to the cabinet.  Use the returned `FolderBuilder` to
    /// add files to the folder or to change other settings on the folder.
    pub fn add_folder(
        &mut self,
        ctype: CompressionType,
    ) -> &mut FolderBuilder {
        self.folders.push(FolderBuilder::new(ctype));
        self.folders.last_mut().unwrap()
    }

    /// Sets the cabinet file's header reserve data.  The meaning of this data
    /// is application-defined.  The data must be no more than 60,000 bytes
    /// long.
    pub fn set_reserve_data(&mut self, data: Vec<u8>) {
        self.reserve_data = data;
    }

    /// Locks in the cabinet settings and returns a `CabinetWriter` object that
    /// will write the cabinet file into the given writer.
    pub fn build<W: Write + Seek>(
        self,
        writer: W,
    ) -> io::Result<CabinetWriter<W>> {
        CabinetWriter::start(writer, self)
    }
}

impl Default for CabinetBuilder {
    fn default() -> Self {
        CabinetBuilder::new()
    }
}

/// A structure for writing file data into a new cabinet file.
pub struct CabinetWriter<W: Write + Seek> {
    writer: InnerCabinetWriter<W>,
    builder: CabinetBuilder,
    current_folder_index: usize,
    next_file_index: usize,
    offset_within_folder: u64,
}

enum InnerCabinetWriter<W: Write + Seek> {
    Raw(W),
    Folder(FolderWriter<W>),
    None,
}

impl<W: Write + Seek> InnerCabinetWriter<W> {
    fn is_none(&self) -> bool {
        matches!(*self, InnerCabinetWriter::None)
    }

    fn take(&mut self) -> InnerCabinetWriter<W> {
        mem::replace(self, InnerCabinetWriter::None)
    }
}

impl<W: Write + Seek> CabinetWriter<W> {
    fn start(
        mut writer: W,
        mut builder: CabinetBuilder,
    ) -> io::Result<CabinetWriter<W>> {
        let num_folders = builder.folders.len();
        if num_folders > consts::MAX_NUM_FOLDERS {
            invalid_input!(
                "Cabinet has too many folders ({}; max is {})",
                num_folders,
                consts::MAX_NUM_FOLDERS
            );
        }

        let num_files: usize =
            builder.folders.iter().map(|folder| folder.files.len()).sum();
        if num_files > consts::MAX_NUM_FILES {
            invalid_input!(
                "Cabinet has too many files ({}; max is {})",
                num_files,
                consts::MAX_NUM_FILES
            );
        }

        let header_reserve_size = builder.reserve_data.len();
        if header_reserve_size > consts::MAX_HEADER_RESERVE_SIZE {
            invalid_input!(
                "Cabinet header reserve data is too large \
                 ({} bytes; max is {} bytes)",
                header_reserve_size,
                consts::MAX_HEADER_RESERVE_SIZE
            );
        }

        let folder_reserve_size = builder
            .folders
            .iter()
            .map(|folder| folder.reserve_data.len())
            .max()
            .unwrap_or(0);
        if folder_reserve_size > consts::MAX_FOLDER_RESERVE_SIZE {
            invalid_input!(
                "Cabinet folder reserve data is too large \
                 ({} bytes; max is {} bytes)",
                folder_reserve_size,
                consts::MAX_FOLDER_RESERVE_SIZE
            );
        }

        let mut flags: u16 = 0;
        if header_reserve_size > 0 || folder_reserve_size > 0 {
            flags |= consts::FLAG_RESERVE_PRESENT;
        }

        let mut first_folder_offset = 36;
        if (flags & consts::FLAG_RESERVE_PRESENT) != 0 {
            first_folder_offset += 4 + header_reserve_size as u32;
        }
        let folder_entry_size = 8 + folder_reserve_size as u32;
        let first_file_offset =
            first_folder_offset + (num_folders as u32) * folder_entry_size;

        // Write cabinet header:
        writer.write_u32::<LittleEndian>(consts::FILE_SIGNATURE)?;
        writer.write_u32::<LittleEndian>(0)?; // reserved1
        writer.write_u32::<LittleEndian>(0)?; // total size, filled later
        writer.write_u32::<LittleEndian>(0)?; // reserved2
        writer.write_u32::<LittleEndian>(first_file_offset)?;
        writer.write_u32::<LittleEndian>(0)?; // reserved3
        writer.write_u8(consts::VERSION_MINOR)?;
        writer.write_u8(consts::VERSION_MAJOR)?;
        writer.write_u16::<LittleEndian>(num_folders as u16)?;
        writer.write_u16::<LittleEndian>(num_files as u16)?;
        writer.write_u16::<LittleEndian>(flags)?;
        writer.write_u16::<LittleEndian>(0)?; // cabinet set ID
        writer.write_u16::<LittleEndian>(0)?; // cabinet set index
        if (flags & consts::FLAG_RESERVE_PRESENT) != 0 {
            writer.write_u16::<LittleEndian>(header_reserve_size as u16)?;
            writer.write_u8(folder_reserve_size as u8)?;
            writer.write_u8(0)?; // data reserve size
            writer.write_all(&builder.reserve_data)?;
        }
        if (flags & consts::FLAG_PREV_CABINET) != 0 {
            invalid_input!("Prev-cabinet feature not yet supported");
        }
        if (flags & consts::FLAG_NEXT_CABINET) != 0 {
            invalid_input!("Next-cabinet feature not yet supported");
        }

        // Write structs for folders:
        for (index, folder) in builder.folders.iter_mut().enumerate() {
            folder.entry_offset =
                first_folder_offset + (index as u32) * folder_entry_size;
            writer.write_u32::<LittleEndian>(0)?; // first data, filled later
            writer.write_u16::<LittleEndian>(0)?; // num data, filled later
            let ctype_bits = folder.compression_type.to_bitfield();
            writer.write_u16::<LittleEndian>(ctype_bits)?;
            debug_assert!(folder.reserve_data.len() <= folder_reserve_size);
            if folder_reserve_size > 0 {
                writer.write_all(&folder.reserve_data)?;
                let padding = folder_reserve_size - folder.reserve_data.len();
                if padding > 0 {
                    writer.write_all(&vec![0; padding])?;
                }
            }
        }

        // Write structs for files:
        let mut current_offset = first_file_offset as u64;
        for (folder_index, folder) in builder.folders.iter_mut().enumerate() {
            for file in folder.files.iter_mut() {
                file.entry_offset = current_offset;
                writer.write_u32::<LittleEndian>(0)?; // size, filled later
                writer.write_u32::<LittleEndian>(0)?; // offset, filled later
                writer.write_u16::<LittleEndian>(folder_index as u16)?;
                let (date, time) = datetime_to_bits(file.datetime);
                writer.write_u16::<LittleEndian>(date)?;
                writer.write_u16::<LittleEndian>(time)?;
                writer.write_u16::<LittleEndian>(file.attributes)?;
                writer.write_all(file.name.as_bytes())?;
                writer.write_u8(0)?;
                current_offset += 17 + file.name.len() as u64;
            }
        }

        Ok(CabinetWriter {
            writer: InnerCabinetWriter::Raw(writer),
            builder,
            current_folder_index: 0,
            next_file_index: 0,
            offset_within_folder: 0,
        })
    }

    /// Returns a `FileWriter` for the next file within that cabinet that needs
    /// data to be written, or `None` if all files are now complete.
    pub fn next_file(&mut self) -> io::Result<Option<FileWriter<W>>> {
        let num_folders = self.builder.folders.len();
        while self.current_folder_index < num_folders {
            if self.next_file_index > 0 {
                // End previous file:
                let folder = &self.builder.folders[self.current_folder_index];
                let file = &folder.files[self.next_file_index - 1];
                self.offset_within_folder += file.uncompressed_size as u64;
            }
            let num_files =
                self.builder.folders[self.current_folder_index].files.len();
            if self.next_file_index < num_files {
                let folder =
                    &mut self.builder.folders[self.current_folder_index];
                if self.next_file_index == 0 {
                    // Begin folder:
                    match self.writer.take() {
                        InnerCabinetWriter::Raw(writer) => {
                            let folder_writer = FolderWriter::new(
                                writer,
                                folder.compression_type,
                                folder.entry_offset,
                            )?;
                            self.writer =
                                InnerCabinetWriter::Folder(folder_writer);
                        }
                        _ => unreachable!(),
                    }
                }
                // Begin next file:
                let file = &mut folder.files[self.next_file_index];
                if self.offset_within_folder > (u32::MAX as u64) {
                    invalid_data!(
                        "Folder is overfull \
                         (file offset of {} bytes, max is {} bytes)",
                        self.offset_within_folder,
                        u32::MAX
                    );
                }
                file.offset_within_folder = self.offset_within_folder as u32;
                let file_writer = match self.writer {
                    InnerCabinetWriter::Folder(ref mut folder_writer) => {
                        FileWriter::new(folder_writer, file)
                    }
                    _ => unreachable!(),
                };
                self.next_file_index += 1;
                return Ok(Some(file_writer));
            }
            // End folder:
            match self.writer.take() {
                InnerCabinetWriter::Folder(folder_writer) => {
                    let folder =
                        &self.builder.folders[self.current_folder_index];
                    let writer = folder_writer.finish(&folder.files)?;
                    self.writer = InnerCabinetWriter::Raw(writer);
                }
                _ => unreachable!(),
            }
            self.current_folder_index += 1;
            self.next_file_index = 0;
            self.offset_within_folder = 0;
        }
        Ok(None)
    }

    /// Finishes writing the cabinet file, and returns the underlying writer.
    pub fn finish(mut self) -> io::Result<W> {
        self.shutdown()?;
        match self.writer.take() {
            InnerCabinetWriter::Raw(writer) => Ok(writer),
            _ => unreachable!(),
        }
    }

    fn shutdown(&mut self) -> io::Result<()> {
        while (self.next_file()?).is_some() {}
        match self.writer {
            InnerCabinetWriter::Raw(ref mut writer) => {
                let cabinet_file_size = writer.stream_position()?;
                if cabinet_file_size > (consts::MAX_TOTAL_CAB_SIZE as u64) {
                    invalid_data!(
                        "Cabinet file is too large \
                         ({} bytes; max is {} bytes)",
                        cabinet_file_size,
                        consts::MAX_TOTAL_CAB_SIZE
                    );
                }
                writer.seek(SeekFrom::Start(8))?;
                writer.write_u32::<LittleEndian>(cabinet_file_size as u32)?;
                writer.seek(SeekFrom::End(0))?;
                writer.flush()?;
            }
            _ => unreachable!(),
        };
        Ok(())
    }
}

impl<W: Write + Seek> Drop for CabinetWriter<W> {
    fn drop(&mut self) {
        if !self.writer.is_none() {
            let _ = self.shutdown();
        }
    }
}

/// Allows writing data for a single file within a new cabinet.
pub struct FileWriter<'a, W: 'a + Write + Seek> {
    folder_writer: &'a mut FolderWriter<W>,
    file_builder: &'a mut FileBuilder,
}

impl<'a, W: Write + Seek> FileWriter<'a, W> {
    fn new(
        folder_writer: &'a mut FolderWriter<W>,
        file_builder: &'a mut FileBuilder,
    ) -> FileWriter<'a, W> {
        FileWriter { folder_writer, file_builder }
    }

    /// Returns the name of the file being written.
    pub fn file_name(&self) -> &str {
        &self.file_builder.name
    }
}

impl<'a, W: Write + Seek> Write for FileWriter<'a, W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if self.file_builder.uncompressed_size == consts::MAX_FILE_SIZE {
            invalid_input!(
                "File is already at maximum size of {} bytes",
                consts::MAX_FILE_SIZE
            );
        }
        let remaining =
            consts::MAX_FILE_SIZE - self.file_builder.uncompressed_size;
        let max_bytes = (buf.len() as u64).min(remaining as u64) as usize;
        let bytes_written = self.folder_writer.write(&buf[0..max_bytes])?;
        self.file_builder.uncompressed_size += bytes_written as u32;
        Ok(bytes_written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.folder_writer.flush()
    }
}

/// A writer for writer data into a cabinet folder.
struct FolderWriter<W: Write + Seek> {
    writer: W,
    compressor: FolderCompressor,
    folder_entry_offset: u32,
    first_data_block_offset: u32,
    next_data_block_offset: u64,
    num_data_blocks: u16,
    data_block_buffer: Vec<u8>,
}

enum FolderCompressor {
    Uncompressed,
    MsZip(MsZipCompressor),
    // TODO: add options for other compression types
}

impl<W: Write + Seek> FolderWriter<W> {
    fn new(
        mut writer: W,
        compression_type: CompressionType,
        folder_entry_offset: u32,
    ) -> io::Result<FolderWriter<W>> {
        let current_offset = writer.stream_position()?;
        if current_offset > (consts::MAX_TOTAL_CAB_SIZE as u64) {
            invalid_data!(
                "Cabinet file is too large \
                 (already {} bytes; max is {} bytes)",
                current_offset,
                consts::MAX_TOTAL_CAB_SIZE
            );
        }
        let compressor = match compression_type {
            CompressionType::None => FolderCompressor::Uncompressed,
            CompressionType::MsZip => {
                FolderCompressor::MsZip(MsZipCompressor::new())
            }
            CompressionType::Quantum(_, _) => {
                invalid_data!("Quantum compression is not yet supported.");
            }
            CompressionType::Lzx(_) => {
                invalid_data!("LZX compression is not yet supported.");
            }
        };
        Ok(FolderWriter {
            writer,
            compressor,
            folder_entry_offset,
            first_data_block_offset: current_offset as u32,
            next_data_block_offset: current_offset,
            num_data_blocks: 0,
            data_block_buffer: Vec::with_capacity(MAX_UNCOMPRESSED_BLOCK_SIZE),
        })
    }

    fn finish(mut self, files: &[FileBuilder]) -> io::Result<W> {
        if !self.data_block_buffer.is_empty() {
            self.write_data_block(true)?;
        }
        let mut writer = self.writer;
        let offset = writer.stream_position()?;
        writer.seek(SeekFrom::Start(self.folder_entry_offset as u64))?;
        writer.write_u32::<LittleEndian>(self.first_data_block_offset)?;
        writer.write_u16::<LittleEndian>(self.num_data_blocks)?;
        for file in files.iter() {
            writer.seek(SeekFrom::Start(file.entry_offset))?;
            writer.write_u32::<LittleEndian>(file.uncompressed_size)?;
            writer.write_u32::<LittleEndian>(file.offset_within_folder)?;
        }
        writer.seek(SeekFrom::Start(offset))?;
        Ok(writer)
    }

    fn write_data_block(&mut self, is_last_block: bool) -> io::Result<()> {
        debug_assert!(!self.data_block_buffer.is_empty());
        let uncompressed_size = self.data_block_buffer.len() as u16;
        let compressed = match self.compressor {
            FolderCompressor::Uncompressed => {
                let empty = Vec::with_capacity(MAX_UNCOMPRESSED_BLOCK_SIZE);
                mem::replace(&mut self.data_block_buffer, empty)
            }
            FolderCompressor::MsZip(ref mut compressor) => {
                let compressed = compressor
                    .compress_block(&self.data_block_buffer, is_last_block)?;
                self.data_block_buffer.clear();
                compressed
            }
        };
        let compressed_size = compressed.len() as u16;
        let mut checksum = Checksum::new();
        checksum.update(&compressed);
        let checksum_value = checksum.value()
            ^ ((compressed_size as u32) | ((uncompressed_size as u32) << 16));
        let total_data_block_size = 8 + compressed_size as u64;
        self.writer.seek(SeekFrom::Start(self.next_data_block_offset))?;
        self.writer.write_u32::<LittleEndian>(checksum_value)?;
        self.writer.write_u16::<LittleEndian>(compressed_size)?;
        self.writer.write_u16::<LittleEndian>(uncompressed_size)?;
        self.writer.write_all(&compressed)?;
        self.next_data_block_offset += total_data_block_size;
        self.num_data_blocks += 1;
        Ok(())
    }
}

impl<W: Write + Seek> Write for FolderWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let capacity = self.data_block_buffer.capacity();
        debug_assert_eq!(capacity, MAX_UNCOMPRESSED_BLOCK_SIZE);
        if buf.is_empty() {
            return Ok(0);
        }
        if self.data_block_buffer.len() == capacity {
            self.write_data_block(false)?;
        }
        let max_bytes = buf.len().min(capacity - self.data_block_buffer.len());
        debug_assert!(max_bytes > 0);
        self.data_block_buffer.extend_from_slice(&buf[..max_bytes]);
        debug_assert_eq!(self.data_block_buffer.capacity(), capacity);
        Ok(max_bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::CabinetBuilder;
    use crate::ctype::CompressionType;
    use std::io::{Cursor, Write};
    use time::macros::datetime;

    #[test]
    fn write_uncompressed_cabinet_with_one_file() {
        let mut builder = CabinetBuilder::new();
        let dt = datetime!(1997-03-12 11:13:52);
        builder
            .add_folder(CompressionType::None)
            .add_file("hi.txt")
            .set_datetime(dt);
        let mut cab_writer = builder.build(Cursor::new(Vec::new())).unwrap();
        while let Some(mut file_writer) = cab_writer.next_file().unwrap() {
            file_writer.write_all(b"Hello, world!\n").unwrap();
        }
        let output = cab_writer.finish().unwrap().into_inner();
        let expected: &[u8] = b"MSCF\0\0\0\0\x59\0\0\0\0\0\0\0\
            \x2c\0\0\0\0\0\0\0\x03\x01\x01\0\x01\0\0\0\0\0\0\0\
            \x43\0\0\0\x01\0\0\0\
            \x0e\0\0\0\0\0\0\0\0\0\x6c\x22\xba\x59\x20\0hi.txt\0\
            \x4c\x1a\x2e\x7f\x0e\0\x0e\0Hello, world!\n";
        assert_eq!(output.as_slice(), expected);
    }

    #[test]
    fn write_uncompressed_cabinet_with_two_files() {
        let mut builder = CabinetBuilder::new();
        let dt = datetime!(2018-01-06 15:19:42);
        {
            let folder_builder = builder.add_folder(CompressionType::None);
            folder_builder.add_file("hi.txt").set_datetime(dt);
            folder_builder.add_file("bye.txt").set_datetime(dt);
        }
        let mut cab_writer = builder.build(Cursor::new(Vec::new())).unwrap();
        while let Some(mut file_writer) = cab_writer.next_file().unwrap() {
            let data = if file_writer.file_name() == "hi.txt" {
                "Hello, world!\n".as_bytes()
            } else {
                "See you later!\n".as_bytes()
            };
            file_writer.write_all(data).unwrap();
        }
        let output = cab_writer.finish().unwrap().into_inner();
        let expected: &[u8] = b"MSCF\0\0\0\0\x80\0\0\0\0\0\0\0\
            \x2c\0\0\0\0\0\0\0\x03\x01\x01\0\x02\0\0\0\0\0\0\0\
            \x5b\0\0\0\x01\0\0\0\
            \x0e\0\0\0\0\0\0\0\0\0\x26\x4c\x75\x7a\x20\0hi.txt\0\
            \x0f\0\0\0\x0e\0\0\0\0\0\x26\x4c\x75\x7a\x20\0bye.txt\0\
            \x1a\x54\x09\x35\x1d\0\x1d\0Hello, world!\nSee you later!\n";
        assert_eq!(output.as_slice(), expected);
    }

    #[test]
    fn write_uncompressed_cabinet_with_non_ascii_filename() {
        let mut builder = CabinetBuilder::new();
        let dt = datetime!(1997-03-12 11:13:52);
        builder
            .add_folder(CompressionType::None)
            .add_file("\u{2603}.txt")
            .set_datetime(dt);
        let mut cab_writer = builder.build(Cursor::new(Vec::new())).unwrap();
        while let Some(mut file_writer) = cab_writer.next_file().unwrap() {
            file_writer.write_all(b"Snowman!\n").unwrap();
        }
        let output = cab_writer.finish().unwrap().into_inner();
        let expected: &[u8] = b"MSCF\0\0\0\0\x55\0\0\0\0\0\0\0\
            \x2c\0\0\0\0\0\0\0\x03\x01\x01\0\x01\0\0\0\0\0\0\0\
            \x44\0\0\0\x01\0\0\0\
            \x09\0\0\0\0\0\0\0\0\0\x6c\x22\xba\x59\xa0\0\xe2\x98\x83.txt\0\
            \x3d\x0f\x08\x56\x09\0\x09\0Snowman!\n";
        assert_eq!(output.as_slice(), expected);
    }
}
