extern crate cab;
extern crate chrono;
extern crate clap;

use cab::{Cabinet, CabinetBuilder, CompressionType, FileEntry, FolderEntry};
use chrono::NaiveDateTime;
use clap::{App, Arg, SubCommand};
use std::fs::{self, File};
use std::io;
use std::path::PathBuf;
use std::time::UNIX_EPOCH;

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
        .subcommand(SubCommand::with_name("create")
                        .about("Creates a new cabinet")
                        .arg(Arg::with_name("compress")
                                 .takes_value(true)
                                 .value_name("TYPE")
                                 .short("c")
                                 .long("compress")
                                 .help("Sets compression type"))
                        .arg(Arg::with_name("output")
                                 .takes_value(true)
                                 .value_name("PATH")
                                 .short("o")
                                 .long("output")
                                 .help("Sets output path"))
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
    } else if let Some(submatches) = matches.subcommand_matches("create") {
        let ctype = if let Some(string) = submatches.value_of("compress") {
            let string = string.to_lowercase();
            match string.as_str() {
                "none" => CompressionType::None,
                "mszip" => CompressionType::MsZip,
                _ => panic!("Invalid compression type: {}", string),
            }
        } else {
            CompressionType::MsZip
        };
        let out_path = if let Some(path) = submatches.value_of("output") {
            PathBuf::from(path)
        } else {
            let mut path = PathBuf::from("out.cab");
            let mut index: i32 = 0;
            while path.exists() {
                index += 1;
                path = PathBuf::from(format!("out{}.cab", index));
            }
            path
        };
        let mut builder = CabinetBuilder::new();
        if let Some(filenames) = submatches.values_of("file") {
            let filenames: Vec<&str> = filenames.collect();
            let mut file_index: usize = 0;
            while file_index < filenames.len() {
                let folder = builder.add_folder(ctype);
                let mut folder_size: u64 = 0;
                while file_index < filenames.len() && folder_size < 0x8000 {
                    let filename = filenames[file_index];
                    let metadata = fs::metadata(filename).unwrap();
                    folder_size += metadata.len();
                    let file = folder.add_file(filename);
                    if let Ok(time) = metadata.modified() {
                        if let Ok(dur) = time.duration_since(UNIX_EPOCH) {
                            let secs = dur.as_secs() as i64;
                            let ndt = NaiveDateTime::from_timestamp(secs, 0);
                            file.set_datetime(ndt);
                        }
                    }
                    file_index += 1;
                }
            }
        }
        let file = File::create(&out_path).unwrap();
        let mut cabinet = builder.build(file).unwrap();
        while let Some(mut writer) = cabinet.next_file().unwrap() {
            let mut file = File::open(writer.file_name()).unwrap();
            io::copy(&mut file, &mut writer).unwrap();
        }
        cabinet.finish().unwrap();
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

fn list_file(folder_index: usize, folder: &FolderEntry, file: &FileEntry,
             long: bool) {
    if !long {
        println!("{}", file.name());
        return;
    }
    let ctype = match folder.compression_type() {
        CompressionType::None => "None".to_string(),
        CompressionType::MsZip => "MsZip".to_string(),
        CompressionType::Quantum(v, m) => format!("Q{}/{}", v, m),
        CompressionType::Lzx(w) => format!("Lzx{}", w),
    };
    let file_size = if file.uncompressed_size() >= 100_000_000 {
        format!("{} MB", file.uncompressed_size() / (1 << 20))
    } else if file.uncompressed_size() >= 1_000_000 {
        format!("{} kB", file.uncompressed_size() / (1 << 10))
    } else {
        format!("{} B ", file.uncompressed_size())
    };
    println!("{}{}{}{}{}{} {:>2} {:<5} {:>10} {} {}",
             if file.is_read_only() { 'R' } else { '-' },
             if file.is_hidden() { 'H' } else { '-' },
             if file.is_system() { 'S' } else { '-' },
             if file.is_archive() { 'A' } else { '-' },
             if file.is_exec() { 'E' } else { '-' },
             if file.is_name_utf() { 'U' } else { '-' },
             folder_index,
             ctype,
             file_size,
             file.datetime().map(|dt| dt.to_string()).unwrap_or("invalid datetime".to_string()),
             file.name());
}

fn open_cab(path: &str) -> io::Result<Cabinet<File>> {
    Cabinet::new(File::open(path)?)
}

// ========================================================================= //
