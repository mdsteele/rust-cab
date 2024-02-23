use std::cell::RefCell;
use std::io::{self, Read, Seek, SeekFrom};

use byteorder::{LittleEndian, ReadBytesExt};

use crate::consts;
use crate::file::{parse_file_entry, FileEntry, FileReader};
use crate::folder::{
    parse_folder_entry, FolderEntries, FolderEntry, FolderReader,
};
use crate::string::read_null_terminated_string;

pub(crate) trait ReadSeek: Read + Seek {}
impl<R: Read + Seek> ReadSeek for R {}

/// A structure for reading a cabinet file.
pub struct Cabinet<R: ?Sized> {
    pub(crate) inner: CabinetInner<R>,
}

pub(crate) struct CabinetInner<R: ?Sized> {
    cabinet_set_id: u16,
    cabinet_set_index: u16,
    data_reserve_size: u8,
    reserve_data: Vec<u8>,
    folders: Vec<FolderEntry>,
    files: Vec<FileEntry>,
    reader: RefCell<R>,
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
            invalid_data!(
                "Cabinet total size field is too large \
                 ({} bytes; max is {} bytes)",
                total_size,
                consts::MAX_TOTAL_CAB_SIZE
            );
        }
        let _reserved2 = reader.read_u32::<LittleEndian>()?;
        let first_file_offset = reader.read_u32::<LittleEndian>()?;
        let _reserved3 = reader.read_u32::<LittleEndian>()?;
        let minor_version = reader.read_u8()?;
        let major_version = reader.read_u8()?;
        if major_version > consts::VERSION_MAJOR
            || major_version == consts::VERSION_MAJOR
                && minor_version > consts::VERSION_MINOR
        {
            invalid_data!(
                "Version {}.{} cabinet files are not supported",
                major_version,
                minor_version
            );
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
        let mut folders = Vec::with_capacity(num_folders);
        for _ in 0..num_folders {
            let entry =
                parse_folder_entry(&mut reader, folder_reserve_size as usize)?;
            folders.push(entry);
        }
        reader.seek(SeekFrom::Start(first_file_offset as u64))?;
        let mut files = Vec::with_capacity(num_files as usize);
        for _ in 0..num_files {
            let entry = parse_file_entry(&mut reader)?;
            let folder_index = entry.folder_index as usize;
            if folder_index >= folders.len() {
                invalid_data!("File entry folder index out of bounds");
            }
            let folder = &mut folders[folder_index];
            folder.files.push(entry.clone());
            files.push(entry);
        }
        Ok(Cabinet {
            inner: CabinetInner {
                cabinet_set_id,
                cabinet_set_index,
                data_reserve_size,
                reserve_data: header_reserve_data,
                folders,
                files,
                reader: RefCell::new(reader),
            },
        })
    }

    /// Returns the cabinet set ID for this cabinet (an arbitrary number used
    /// to group together a set of cabinets).
    pub fn cabinet_set_id(&self) -> u16 {
        self.inner.cabinet_set_id
    }

    /// Returns this cabinet's (zero-based) index within its cabinet set.
    pub fn cabinet_set_index(&self) -> u16 {
        self.inner.cabinet_set_index
    }

    /// Returns the application-defined reserve data stored in the cabinet
    /// header.
    pub fn reserve_data(&self) -> &[u8] {
        &self.inner.reserve_data
    }

    /// Returns an iterator over the folder entries in this cabinet.
    pub fn folder_entries(&self) -> FolderEntries {
        FolderEntries { iter: self.inner.folders.iter() }
    }

    /// Returns the entry for the file with the given name, if any..
    pub fn get_file_entry(&self, name: &str) -> Option<&FileEntry> {
        self.inner.files.iter().find(|&file| file.name() == name)
    }

    /// Returns a reader over the decompressed data for the file in the cabinet
    /// with the given name.
    pub fn read_file(&mut self, name: &str) -> io::Result<FileReader<R>> {
        match self.get_file_entry(name) {
            Some(file_entry) => {
                let folder_index = file_entry.folder_index as usize;
                let file_start_in_folder =
                    file_entry.uncompressed_offset as u64;
                let size = file_entry.uncompressed_size() as u64;
                let mut folder_reader = self.read_folder(folder_index)?;
                folder_reader
                    .seek_to_uncompressed_offset(file_start_in_folder)?;
                Ok(FileReader {
                    reader: folder_reader,
                    file_start_in_folder,
                    offset: 0,
                    size,
                })
            }

            None => not_found!("No such file in cabinet: {:?}", name),
        }
    }

    /// Returns a reader over the decompressed data in the specified folder.
    fn read_folder(&mut self, index: usize) -> io::Result<FolderReader<R>> {
        if index >= self.inner.folders.len() {
            invalid_input!(
                "Folder index {} is out of range (cabinet has {} folders)",
                index,
                self.inner.folders.len()
            );
        }

        let me: &Cabinet<dyn ReadSeek> = self;
        FolderReader::new(
            me,
            &self.inner.folders[index],
            self.inner.data_reserve_size,
        )
    }
}

impl<'a, R: ?Sized + Read> Read for &'a CabinetInner<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.reader.borrow_mut().read(buf)
    }
}

impl<'a, R: ?Sized + Seek> Seek for &'a CabinetInner<R> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.reader.borrow_mut().seek(pos)
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Read};

    use super::Cabinet;

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
            assert_eq!(dt.month(), time::Month::March);
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
        let binary: &[u8] = b"MSCF\0\0\0\0\x80\0\0\0\0\0\0\0\
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
        assert_eq!(
            cabinet.folder_entries().nth(0).unwrap().num_data_blocks(),
            2
        );

        let mut data = Vec::new();
        cabinet.read_folder(0).unwrap().read_to_end(&mut data).unwrap();
        assert_eq!(data, b"Hello, world!\n");

        let mut data = Vec::new();
        cabinet.read_file("hi.txt").unwrap().read_to_end(&mut data).unwrap();
        assert_eq!(data, b"Hello, world!\n");
    }

    #[test]
    fn read_mszip_cabinet_with_one_file() {
        let binary: &[u8] = b"MSCF\0\0\0\0\x61\0\0\0\0\0\0\0\
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
        let binary: &[u8] = b"MSCF\0\0\0\0\x88\0\0\0\0\0\0\0\
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
