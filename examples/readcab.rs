extern crate cab;

use std::env;
use std::fs::File;
use std::path::Path;

fn main() {
    let num_args = env::args().count();
    if num_args != 2 {
        println!("Usage: readcab <path/to/archive.cab>");
        return;
    }

    let input_path = env::args().nth(1).unwrap();
    let input_path = Path::new(&input_path);
    let input_file = File::open(input_path).unwrap();
    let cabinet = cab::Cabinet::new(input_file).unwrap();
    for (index, folder) in cabinet.folder_entries().enumerate() {
        println!("Folder #{}:", index);
        println!("  compression_type = {:?}", folder.compression_type());
        println!("  reserve_data = {:?}", folder.reserve_data());
        for file in folder.file_entries() {
            println!("  {:?} ({} bytes)",
                     file.name(),
                     file.uncompressed_size());
        }
    }
}
