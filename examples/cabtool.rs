use std::fs::{self, File};
use std::io;
use std::path::PathBuf;
use std::time::UNIX_EPOCH;

use clap::{Parser, Subcommand};
use time::{OffsetDateTime, PrimitiveDateTime};

use cab::{Cabinet, CabinetBuilder, CompressionType, FileEntry, FolderEntry};

#[derive(Parser, Debug)]
#[command(author, about, version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Concatenates and prints streams
    Cat { path: PathBuf, files: Vec<String> },
    /// Creates a new cabinet
    Create {
        /// Sets compression type
        #[clap(short, long, default_value_t = String::from("mszip"))]
        compress: String,
        /// Sets output path
        #[clap(short, long)]
        output: Option<PathBuf>,
        files: Vec<String>,
    },
    /// Lists files in the cabinet
    Ls {
        /// Lists in long format
        #[clap(short, long)]
        long: bool,
        path: PathBuf,
    },
}

fn main() {
    let cli = Cli::parse();
    match cli.command {
        Command::Cat { path, files } => {
            let mut cabinet = Cabinet::new(File::open(path).unwrap()).unwrap();
            for filename in files {
                let mut file_reader = cabinet.read_file(&filename).unwrap();
                io::copy(&mut file_reader, &mut io::stdout()).unwrap();
            }
        }
        Command::Create { compress, output, files } => {
            let compress = match compress.as_str() {
                "none" => CompressionType::None,
                "mszip" => CompressionType::MsZip,
                _ => panic!("Invalid compression type: {}", compress),
            };

            let output = output.unwrap_or_else(|| {
                let mut path = PathBuf::from("out.cab");
                let mut index: i32 = 0;
                while path.exists() {
                    index += 1;
                    path = PathBuf::from(format!("out{}.cab", index));
                }
                path
            });
            let mut builder = CabinetBuilder::new();
            let mut file_index: usize = 0;
            while file_index < files.len() {
                let folder = builder.add_folder(compress);
                let mut folder_size: u64 = 0;
                while file_index < files.len() && folder_size < 0x8000 {
                    let filename = files[file_index].as_str();
                    let metadata = fs::metadata(filename).unwrap();
                    folder_size += metadata.len();
                    let file = folder.add_file(filename);
                    if let Ok(time) = metadata.modified() {
                        if let Ok(dur) = time.duration_since(UNIX_EPOCH) {
                            let dt = OffsetDateTime::from_unix_timestamp(
                                dur.as_secs() as i64,
                            )
                            .unwrap();
                            file.set_datetime(PrimitiveDateTime::new(
                                dt.date(),
                                dt.time(),
                            ));
                        }
                    }
                    file_index += 1;
                }
            }
            let file = File::create(&output).unwrap();
            let mut cabinet = builder.build(file).unwrap();
            while let Some(mut writer) = cabinet.next_file().unwrap() {
                let mut file = File::open(writer.file_name()).unwrap();
                io::copy(&mut file, &mut writer).unwrap();
            }
            cabinet.finish().unwrap();
        }
        Command::Ls { path, long } => {
            let cabinet = Cabinet::new(File::open(path).unwrap()).unwrap();
            for (index, folder) in cabinet.folder_entries().enumerate() {
                for file in folder.file_entries() {
                    list_file(index, &folder, file, long);
                }
            }
        }
    }
}

fn list_file(
    folder_index: usize,
    folder: &FolderEntry,
    file: &FileEntry,
    long: bool,
) {
    if !long {
        println!("{}", file.name());
        return;
    }
    let ctype = match folder.compression_type() {
        CompressionType::None => "None".to_string(),
        CompressionType::MsZip => "MsZip".to_string(),
        CompressionType::Quantum(v, m) => format!("Q{}/{}", v, m),
        CompressionType::Lzx(w) => format!("Lzx{:?}", w),
    };
    let file_size = if file.uncompressed_size() >= 100_000_000 {
        format!("{} MB", file.uncompressed_size() / (1 << 20))
    } else if file.uncompressed_size() >= 1_000_000 {
        format!("{} kB", file.uncompressed_size() / (1 << 10))
    } else {
        format!("{} B ", file.uncompressed_size())
    };
    println!(
        "{}{}{}{}{}{} {:>2} {:<5} {:>10} {} {}",
        if file.is_read_only() { 'R' } else { '-' },
        if file.is_hidden() { 'H' } else { '-' },
        if file.is_system() { 'S' } else { '-' },
        if file.is_archive() { 'A' } else { '-' },
        if file.is_exec() { 'E' } else { '-' },
        if file.is_name_utf() { 'U' } else { '-' },
        folder_index,
        ctype,
        file_size,
        file.datetime()
            .map(|dt| dt.to_string())
            .unwrap_or("invalid datetime".to_string()),
        file.name()
    );
}
