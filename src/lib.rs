//! A library for reading/writing [Windows
//! cabinet](https://en.wikipedia.org/wiki/Cabinet_(file_format)) (CAB) files.
//!
//! # Overview
//!
//! CAB is an archive file format used by Windows.  A cabinet file can contain
//! multiple compressed files, which are divided into "folders" (no relation to
//! filesystem folders/directories); files in the same folder are compressed
//! together, and each folder in the cabinet can potentially use a different
//! compression scheme.  The CAB file format supports multiple different
//! compression schemes; this library can recognize all of them when reading
//! metadata for an existing cabinet file, but currently only supports
//! encoding/decoding some of them, as shown:
//!
//! | Compression                | Supported         |
//! |----------------------------|-------------------|
//! | Uncompressed               | Yes               |
//! | MSZIP ([Deflate][deflate]) | Yes               |
//! | [Quantum][quantum]         | No                |
//! | [LZX][lzx]                 | Yes (decode only) |
//!
//! [deflate]: https://en.wikipedia.org/wiki/DEFLATE
//! [quantum]: https://en.wikipedia.org/wiki/Quantum_compression
//! [lzx]: https://en.wikipedia.org/wiki/LZX_(algorithm)
//!
//! # Example usage
//!
//! Use the `Cabinet` type to read an existing cabinet file:
//!
//! ```no_run
//! use cab;
//! use std::fs;
//! use std::io;
//!
//! let cab_file = fs::File::open("path/to/cabinet.cab").unwrap();
//! let mut cabinet = cab::Cabinet::new(cab_file).unwrap();
//! // List all files in the cabinet, with file sizes and compression types:
//! for folder in cabinet.folder_entries() {
//!     for file in folder.file_entries() {
//!         println!("File {} ({} B) is compressed with {:?}",
//!                  file.name(),
//!                  file.uncompressed_size(),
//!                  folder.compression_type());
//!     }
//! }
//! // Decompress a particular file in the cabinet and save it to disk:
//! let mut reader = cabinet.read_file("images/example.png").unwrap();
//! let mut writer = fs::File::create("out/example.png").unwrap();
//! io::copy(&mut reader, &mut writer).unwrap();
//! ```
//!
//! Creating a new cabinet file is a little more involved.  Because of how the
//! cabinet file is structured on disk, the library has to know the names of
//! all the files that will be in the cabinet up front, before it can start
//! writing anything to disk.  However, we don't want to have to hold all the
//! file **contents** in memory at once.  Therefore, cabinet creation happens
//! in two steps: first, create a `CabinetBuilder` and specify all filenames
//! and other metadata, and then second, stream each file's data into a
//! `CabinetWriter`, one at a time:
//!
//! ```no_run
//! use cab;
//! use std::fs;
//! use std::io;
//!
//! let mut cab_builder = cab::CabinetBuilder::new();
//! // Add a single file in its own folder:
//! cab_builder.add_folder(cab::CompressionType::None).add_file("img/foo.jpg");
//! // Add several more files, compressed together in a second folder:
//! {
//!     let folder = cab_builder.add_folder(cab::CompressionType::MsZip);
//!     folder.add_file("documents/README.txt");
//!     folder.add_file("documents/license.txt");
//!     // We can also specify metadata on individual files:
//!     {
//!         let file = folder.add_file("documents/hidden.txt");
//!         file.set_is_hidden(true);
//!         file.set_is_read_only(true);
//!     }
//! }
//! // Now, we'll actually construct the cabinet file on disk:
//! let cab_file = fs::File::create("path/to/cabinet.cab").unwrap();
//! let mut cab_writer = cab_builder.build(cab_file).unwrap();
//! while let Some(mut writer) = cab_writer.next_file().unwrap() {
//!     let mut reader = fs::File::open(writer.file_name()).unwrap();
//!     io::copy(&mut reader, &mut writer).unwrap();
//! }
//! // Print the file size of the cabinet file we just created:
//! let mut cab_file = cab_writer.finish().unwrap();
//! println!("Cabinet size: {} B", cab_file.metadata().unwrap().len());
//! ```

#![warn(missing_docs)]

pub use lzxd::WindowSize;

pub use builder::{
    CabinetBuilder, CabinetWriter, FileBuilder, FileWriter, FolderBuilder,
};
pub use cabinet::Cabinet;
pub use ctype::CompressionType;
pub use file::{FileEntries, FileEntry, FileReader};
pub use folder::{FolderEntries, FolderEntry};

#[macro_use]
mod macros;

mod builder;
mod cabinet;
mod checksum;
mod consts;
mod ctype;
mod datetime;
mod file;
mod folder;
mod mszip;
mod string;
