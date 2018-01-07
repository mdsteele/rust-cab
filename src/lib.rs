//! A library for reading/writing [Windows
//! cabinet](https://en.wikipedia.org/wiki/Cabinet_(file_format)) (CAB) files.

#![warn(missing_docs)]

extern crate byteorder;
extern crate chrono;
extern crate flate2;

mod internal;

pub use internal::builder::{CabinetBuilder, CabinetWriter, FileBuilder,
                            FileWriter, FolderBuilder};
pub use internal::cabinet::{Cabinet, FileEntries, FileEntry, FileReader,
                            FolderEntries, FolderEntry};
pub use internal::ctype::CompressionType;

// ========================================================================= //
