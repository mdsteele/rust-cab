use std::fs::File;
use std::path::PathBuf;

use anyhow::Context;
use clap::Parser;

#[derive(Parser, Debug)]
#[command(author, about)]
struct Cli {
    path: PathBuf,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let input_file = File::open(cli.path)?;
    let cabinet = cab::Cabinet::new(input_file)
        .context("Failed to open cabinet file")?;
    let mut index = 0;
    while let Some(folder) = cabinet.folder_entries().next() {
        let folder = folder.context("Failed to get folders")?;
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

        index += 1;
        println!("  {} bytes total", total_size);
    }

    Ok(())
}
