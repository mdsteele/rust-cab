extern crate cab;

use anyhow::Context;

use std::env;
use std::fs::File;
use std::path::Path;

fn main() -> anyhow::Result<()> {
    let num_args = env::args().count();
    if num_args != 2 {
        println!("Usage: readcab <path/to/archive.cab>");
        return Ok(());
    }

    let input_path = env::args().nth(1).unwrap();
    let input_path = Path::new(&input_path);
    let input_file = File::open(input_path)?;
    let cabinet = cab::Cabinet::new(input_file).context("Failed to open cabinet file")?;
    for (index, folder) in cabinet.folder_entries().enumerate() {
        println!("Folder #{}:", index);
        println!("  compression_type = {:?}", folder.compression_type());
        println!("  reserve_data = {:?}", folder.reserve_data());
        println!("  num_data_blocks = {}", folder.num_data_blocks());
        let mut total_size = 0;
        for file in folder.file_entries() {
            let size = file.uncompressed_size();
            println!("  {:?} ({} bytes)", file.name(), size);
            total_size += size;
        }
        println!("  {} bytes total", total_size);
    }

    Ok(())
}
