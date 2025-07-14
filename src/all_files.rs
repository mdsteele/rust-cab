use std::io::{Read, Seek};

use crate::{folder::FolderReader, Cabinet, FileEntry, FolderEntry};

pub struct SingleFileReader<'a, 'b, R>
where
    R: Read + Seek + 'a,
{
    parent: &'b mut FileReaderState<'a, R>,
}

impl<'a, 'b, R> Read for SingleFileReader<'a, 'b, R>
where
    R: Read + Seek + 'a,
{
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let limlen = buf.len().min(self.parent.remain as usize);
        let lim_buf = &mut buf[0..limlen];
        match self.parent.folder_reader.read(lim_buf) {
            Ok(n) => {
                self.parent.remain -= n as u64;
                Ok(n)
            }
            Err(e) => Err(e),
        }
    }
}

pub struct AllFiles<'a, R> {
    pub(crate) data_reserve_size: u8,
    pub(crate) cabinet: &'a Cabinet<R>,
    pub(crate) folder_stack: Vec<FolderEntry>,
    pub(crate) files_stack: Vec<FileEntry>,
    pub(crate) reader_state: Option<FileReaderState<'a, R>>,
}
pub(crate) struct FileReaderState<'a, R> {
    pub(crate) folder_reader: FolderReader<'a, R>,
    pub(crate) remain: u64,
}

/// this is the struct created by [`Cabinet::all_files`]
impl<'a, R> AllFiles<'a, R>
where
    R: Read + Seek + 'a,
{
    pub fn next_file<'b>(
        &'b mut self,
    ) -> Option<(FileEntry, SingleFileReader<'a, 'b, R>)> {
        if let Some(file_entry) = self.files_stack.pop() {
            if let Some(ref mut sub) = self.reader_state {
                if sub.remain > 0 {
                    let mut taker = (&mut sub.folder_reader).take(sub.remain);
                    let _ = std::io::copy(&mut taker, &mut std::io::sink());
                }
                sub.remain = file_entry.uncompressed_size() as u64;
                Some((file_entry, SingleFileReader { parent: sub }))
            } else {
                None
            }
        } else {
            match self.folder_stack.pop() {
                Some(folder_entry) => {
                    let folder_reader = FolderReader::new(
                        self.cabinet,
                        &folder_entry,
                        self.data_reserve_size,
                    )
                    .ok()?;
                    self.reader_state =
                        Some(FileReaderState { folder_reader, remain: 0 });
                    let files = folder_entry.files;
                    let mut files_stack: Vec<_> = files.into_iter().collect();
                    files_stack.sort_by_key(|f| f.uncompressed_offset);
                    files_stack.reverse();
                    self.files_stack = files_stack;
                    self.next_file()
                }
                None => None,
            }
        }
    }
}
