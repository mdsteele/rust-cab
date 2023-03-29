use std::cell::RefCell;
use std::io::{self, Read, Seek, SeekFrom};

use byteorder::{LittleEndian, ReadBytesExt};

use crate::consts;
use crate::file::{parse_file_entry, FileEntry};
use crate::folder::{parse_folder_entry, FolderEntries, FolderEntry};
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
        let mut folders = Vec::<FolderEntry>::with_capacity(num_folders);
        for _ in 0..num_folders {
            let entry =
                parse_folder_entry(&mut reader, folder_reserve_size as usize)?;
            folders.push(entry);
        }
        reader.seek(SeekFrom::Start(first_file_offset as u64))?;
        let mut files = Vec::<FileEntry>::with_capacity(num_files as usize);
        let mut current_folder_idx = 0;
        for idx in 0..num_files {
            let entry = parse_file_entry(&mut reader)?;
            let folder_index = entry.folder_index as usize;
            if folder_index >= folders.len() {
                invalid_data!("File entry folder index out of bounds");
            }
            if folder_index != current_folder_idx {
                folders[folder_index].file_idx_start = idx as usize;
                current_folder_idx = folder_index;
            }
            folders[current_folder_idx].files_count += 1;
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
        let me: &Cabinet<dyn ReadSeek> = self;
        FolderEntries {
            archive: me,
            iter: self.inner.folders.iter(),
            data_reserve_size: self.inner.data_reserve_size,
            files: &self.inner.files,
        }
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

    use crate::Cabinet;

    #[test]
    fn read_uncompressed_cabinet_with_one_file() {
        let binary: &[u8] = b"MSCF\0\0\0\0\x59\0\0\0\0\0\0\0\
            \x2c\0\0\0\0\0\0\0\x03\x01\x01\0\x01\0\0\0\x34\x12\0\0\
            \x43\0\0\0\x01\0\0\0\
            \x0e\0\0\0\0\0\0\0\0\0\x6c\x22\xba\x59\x01\0hi.txt\0\
            \x4c\x1a\x2e\x7f\x0e\0\x0e\0Hello, world!\n";
        assert_eq!(binary.len(), 0x59);
        let cabinet = Cabinet::new(Cursor::new(binary)).unwrap();
        assert_eq!(cabinet.cabinet_set_id(), 0x1234);
        assert_eq!(cabinet.cabinet_set_index(), 0);
        assert_eq!(cabinet.reserve_data(), &[]);

        let mut folder_entries = cabinet.folder_entries();
        let folder_entry = folder_entries.next().unwrap().unwrap();
        let mut file_entries = folder_entry.file_entries();
        assert_eq!(file_entries.len(), 1);
        {
            let mut file = file_entries.next().unwrap();
            assert_eq!(file.name(), "hi.txt");
            assert!(!file.is_name_utf());
            let dt = file.datetime().unwrap();

            assert_eq!(dt.year(), 1997);
            assert_eq!(dt.month(), time::Month::March);
            assert_eq!(dt.day(), 12);
            assert_eq!(dt.hour(), 11);
            assert_eq!(dt.minute(), 13);
            assert_eq!(dt.second(), 52);

            let mut data = Vec::new();
            file.read_to_end(&mut data).unwrap();
            assert_eq!(data, b"Hello, world!\n");
        }
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
        let cabinet = Cabinet::new(Cursor::new(binary)).unwrap();

        for _ in 0..2 {
            let mut folder_entries = cabinet.folder_entries();
            let folder_entry = folder_entries.next().unwrap().unwrap();
            let mut file_entries = folder_entry.file_entries();
            assert_eq!(file_entries.len(), 2);
            {
                let mut file = file_entries.next().unwrap();
                assert_eq!(file.name(), "hi.txt");
                assert!(!file.is_name_utf());
                let mut data = Vec::new();
                file.read_to_end(&mut data).unwrap();
                assert_eq!(data, b"Hello, world!\n");
            }
            {
                let mut file = file_entries.next().unwrap();
                assert_eq!(file.name(), "bye.txt");
                assert!(!file.is_name_utf());
                let mut data = Vec::new();
                file.read_to_end(&mut data).unwrap();
                assert_eq!(data, b"See you later!\n");
            }
        }
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
        let cabinet = Cabinet::new(Cursor::new(binary)).unwrap();
        let mut folder_entries = cabinet.folder_entries();
        let folder_entry = folder_entries.next().unwrap().unwrap();
        assert_eq!(folder_entry.num_data_blocks(), 2);
        let mut file_entries = folder_entry.file_entries();
        {
            let mut file_entry = file_entries.next().unwrap();
            let mut data = Vec::new();
            file_entry.read_to_end(&mut data).unwrap();
            assert_eq!(data, b"Hello, world!\n");
        }
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
        let cabinet = Cabinet::new(Cursor::new(binary)).unwrap();
        assert_eq!(cabinet.cabinet_set_id(), 0x1234);
        assert_eq!(cabinet.cabinet_set_index(), 0);
        assert_eq!(cabinet.reserve_data(), &[]);

        let mut folder_entries = cabinet.folder_entries();

        let folder_entry = folder_entries.next().unwrap().unwrap();
        let mut file_entries = folder_entry.file_entries();
        assert_eq!(file_entries.len(), 1);
        {
            let mut file_entry = file_entries.next().unwrap();
            let mut data = Vec::new();
            file_entry.read_to_end(&mut data).unwrap();
            assert_eq!(data, b"Hello, world!\n");
        }
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
        let cabinet = Cabinet::new(Cursor::new(binary)).unwrap();

        let mut folder_entries = cabinet.folder_entries();

        let folder_entry = folder_entries.next().unwrap().unwrap();
        let mut file_entries = folder_entry.file_entries();
        assert_eq!(file_entries.len(), 2);
        {
            let mut file_entry = file_entries.next().unwrap();
            assert_eq!(file_entry.name(), "hi.txt");

            let mut data = Vec::new();
            file_entry.read_to_end(&mut data).unwrap();
            assert_eq!(data, b"Hello, world!\n");
        }
        {
            let mut file_entry = file_entries.next().unwrap();
            assert_eq!(file_entry.name(), "bye.txt");

            let mut data = Vec::new();
            file_entry.read_to_end(&mut data).unwrap();
            assert_eq!(data, b"See you later!\n");
        }
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
        let cabinet = Cabinet::new(Cursor::new(binary)).unwrap();

        let mut folder_entries = cabinet.folder_entries();

        let folder_entry = folder_entries.next().unwrap().unwrap();
        let mut file_entries = folder_entry.file_entries();
        assert_eq!(file_entries.len(), 2);
        {
            let mut file_entry = file_entries.next().unwrap();
            assert_eq!(file_entry.name(), "hi.txt");

            let mut data = Vec::new();
            file_entry.read_to_end(&mut data).unwrap();
            assert_eq!(data, b"Hello, world!\r\n");
        }
        {
            let mut file_entry = file_entries.next().unwrap();
            assert_eq!(file_entry.name(), "bye.txt");

            let mut data = Vec::new();
            file_entry.read_to_end(&mut data).unwrap();
            assert_eq!(data, b"See you later!\r\n");
        }
    }

    #[test]
    fn read_uncompressed_cabinet_with_non_ascii_filename() {
        let binary: &[u8] = b"MSCF\0\0\0\0\x55\0\0\0\0\0\0\0\
            \x2c\0\0\0\0\0\0\0\x03\x01\x01\0\x01\0\0\0\0\0\0\0\
            \x44\0\0\0\x01\0\0\0\
            \x09\0\0\0\0\0\0\0\0\0\x6c\x22\xba\x59\xa0\0\xe2\x98\x83.txt\0\
            \x3d\x0f\x08\x56\x09\0\x09\0Snowman!\n";
        assert_eq!(binary.len(), 0x55);
        let cabinet = Cabinet::new(Cursor::new(binary)).unwrap();
        let mut folder_entries = cabinet.folder_entries();

        let folder_entry = folder_entries.next().unwrap().unwrap();
        let mut file_entries = folder_entry.file_entries();
        {
            let mut file_entry = file_entries.next().unwrap();

            assert_eq!(file_entry.name(), "\u{2603}.txt");
            let mut data = Vec::new();
            file_entry.read_to_end(&mut data).unwrap();
            assert_eq!(data, b"Snowman!\n");
        }
    }
}
