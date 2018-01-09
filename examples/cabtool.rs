extern crate cab;
extern crate clap;

use clap::{App, Arg, SubCommand};
use std::fs::File;
use std::io;

// ========================================================================= //

fn main() {
    let matches = App::new("cabtool")
        .version("0.1")
        .author("Matthew D. Steele <mdsteele@alum.mit.edu>")
        .about("Manipulates CAB files")
        .subcommand(SubCommand::with_name("cat")
                        .about("Concatenates and prints streams")
                        .arg(Arg::with_name("cab").required(true))
                        .arg(Arg::with_name("file").multiple(true)))
        .subcommand(SubCommand::with_name("ls")
                        .about("Lists files in the cabinet")
                        .arg(Arg::with_name("long")
                                 .short("l")
                                 .help("Lists in long format"))
                        .arg(Arg::with_name("cab").required(true)))
        .get_matches();
    if let Some(submatches) = matches.subcommand_matches("cat") {
        let mut cabinet = open_cab(submatches.value_of("cab").unwrap())
            .unwrap();
        if let Some(filenames) = submatches.values_of("file") {
            for filename in filenames {
                let mut file_reader = cabinet.read_file(filename).unwrap();
                io::copy(&mut file_reader, &mut io::stdout()).unwrap();
            }
        }
    } else if let Some(submatches) = matches.subcommand_matches("ls") {
        let long = submatches.is_present("long");
        let cabinet = open_cab(submatches.value_of("cab").unwrap()).unwrap();
        for (index, folder) in cabinet.folder_entries().enumerate() {
            for file in folder.file_entries() {
                list_file(index, folder, file, long);
            }
        }
    }
}

// ========================================================================= //

fn list_file(folder_index: usize, folder: &cab::FolderEntry,
             file: &cab::FileEntry, long: bool) {
    if !long {
        println!("{}", file.name());
        return;
    }
    let file_size = if file.uncompressed_size() >= 100_000_000 {
        format!("{} MB", file.uncompressed_size() / (1 << 20))
    } else if file.uncompressed_size() >= 1_000_000 {
        format!("{} kB", file.uncompressed_size() / (1 << 10))
    } else {
        format!("{} B ", file.uncompressed_size())
    };
    println!("{}{}{} {:>2} {:?} {:>10} {} {}",
             if file.is_read_only() { 'R' } else { '-' },
             if file.is_hidden() { 'H' } else { '-' },
             if file.is_system_file() { 'S' } else { '-' },
             folder_index,
             folder.compression_type(),
             file_size,
             file.datetime(),
             file.name());
}

fn open_cab(path: &str) -> io::Result<cab::Cabinet<File>> {
    cab::Cabinet::new(File::open(path)?)
}

// ========================================================================= //
