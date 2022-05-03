use std::io::{Cursor, Read, Write};
use time::macros::datetime;

// ========================================================================= //

#[test]
fn cabinet_with_one_small_uncompressed_text_file() {
    let original = lipsum::lipsum(500);
    let datetime = datetime!(2063-04-05 23:14:38);

    let mut cab_builder = cab::CabinetBuilder::new();
    {
        let folder_builder =
            cab_builder.add_folder(cab::CompressionType::None);
        let file_builder = folder_builder.add_file("lorem_ipsum.txt");
        file_builder.set_datetime(datetime);
        file_builder.set_is_read_only(true);
        file_builder.set_is_system(true);
        file_builder.set_is_archive(false);
    }
    let mut cab_writer = cab_builder.build(Cursor::new(Vec::new())).unwrap();
    while let Some(mut file_writer) = cab_writer.next_file().unwrap() {
        file_writer.write_all(original.as_bytes()).unwrap();
    }
    let cab_file = cab_writer.finish().unwrap().into_inner();

    let mut cabinet = cab::Cabinet::new(Cursor::new(cab_file)).unwrap();
    {
        let file_entry = cabinet.get_file_entry("lorem_ipsum.txt").unwrap();
        assert_eq!(file_entry.datetime(), Some(datetime));
        assert!(file_entry.is_read_only());
        assert!(!file_entry.is_hidden());
        assert!(file_entry.is_system());
        assert!(!file_entry.is_archive());
    }
    let mut output = Vec::new();
    let mut file_reader = cabinet.read_file("lorem_ipsum.txt").unwrap();
    file_reader.read_to_end(&mut output).unwrap();
    assert_eq!(String::from_utf8_lossy(&output), original);
}

#[test]
fn cabinet_with_one_small_mszipped_text_file() {
    let original = lipsum::lipsum(500);

    let mut cab_builder = cab::CabinetBuilder::new();
    cab_builder
        .add_folder(cab::CompressionType::MsZip)
        .add_file("lorem_ipsum.txt");
    let mut cab_writer = cab_builder.build(Cursor::new(Vec::new())).unwrap();
    while let Some(mut file_writer) = cab_writer.next_file().unwrap() {
        file_writer.write_all(original.as_bytes()).unwrap();
    }
    let cab_file = cab_writer.finish().unwrap().into_inner();

    let mut cabinet = cab::Cabinet::new(Cursor::new(cab_file)).unwrap();
    assert_eq!(
        cabinet.folder_entries().nth(0).unwrap().compression_type(),
        cab::CompressionType::MsZip
    );
    let mut output = Vec::new();
    let mut file_reader = cabinet.read_file("lorem_ipsum.txt").unwrap();
    file_reader.read_to_end(&mut output).unwrap();
    assert_eq!(String::from_utf8_lossy(&output), original);
}

#[test]
fn cabinet_with_one_big_uncompressed_text_file() {
    let original = lipsum::lipsum(30000);

    let mut cab_builder = cab::CabinetBuilder::new();
    cab_builder
        .add_folder(cab::CompressionType::None)
        .add_file("lorem_ipsum.txt");
    let mut cab_writer = cab_builder.build(Cursor::new(Vec::new())).unwrap();
    while let Some(mut file_writer) = cab_writer.next_file().unwrap() {
        file_writer.write_all(original.as_bytes()).unwrap();
    }
    let cab_file = cab_writer.finish().unwrap().into_inner();
    assert!(cab_file.len() > original.len());

    let mut cabinet = cab::Cabinet::new(Cursor::new(cab_file)).unwrap();
    {
        let folder = cabinet.folder_entries().nth(0).unwrap();
        assert_eq!(folder.compression_type(), cab::CompressionType::None);
        assert!(folder.num_data_blocks() > 1);
        let file = folder.file_entries().nth(0).unwrap();
        assert_eq!(file.uncompressed_size() as usize, original.len());
    }
    let mut output = Vec::new();
    let mut file_reader = cabinet.read_file("lorem_ipsum.txt").unwrap();
    file_reader.read_to_end(&mut output).unwrap();
    assert_eq!(output.len(), original.len());
    assert_eq!(String::from_utf8_lossy(&output), original);
}

#[test]
fn cabinet_with_one_big_mszipped_text_file() {
    let original = lipsum::lipsum(30000);

    let mut cab_builder = cab::CabinetBuilder::new();
    cab_builder
        .add_folder(cab::CompressionType::MsZip)
        .add_file("lorem_ipsum.txt");
    let mut cab_writer = cab_builder.build(Cursor::new(Vec::new())).unwrap();
    while let Some(mut file_writer) = cab_writer.next_file().unwrap() {
        file_writer.write_all(original.as_bytes()).unwrap();
    }
    let cab_file = cab_writer.finish().unwrap().into_inner();
    assert!(cab_file.len() < original.len());

    let mut cabinet = cab::Cabinet::new(Cursor::new(cab_file)).unwrap();
    {
        let folder = cabinet.folder_entries().nth(0).unwrap();
        assert_eq!(folder.compression_type(), cab::CompressionType::MsZip);
        let file = folder.file_entries().nth(0).unwrap();
        assert_eq!(file.uncompressed_size() as usize, original.len());
    }
    let mut output = Vec::new();
    let mut file_reader = cabinet.read_file("lorem_ipsum.txt").unwrap();
    file_reader.read_to_end(&mut output).unwrap();
    assert_eq!(output.len(), original.len());
    assert_eq!(String::from_utf8_lossy(&output), original);
}

// ========================================================================= //

fn random_data_roundtrip(num_bytes: usize, ctype: cab::CompressionType) {
    use rand::{RngCore, SeedableRng};

    let mut original = vec![0; num_bytes];
    rand::rngs::SmallRng::from_entropy().fill_bytes(&mut original);

    let mut cab_builder = cab::CabinetBuilder::new();
    cab_builder.add_folder(ctype).add_file("binary");
    let mut cab_writer = cab_builder.build(Cursor::new(Vec::new())).unwrap();
    while let Some(mut file_writer) = cab_writer.next_file().unwrap() {
        file_writer.write_all(&original).unwrap();
    }
    let cab_file = cab_writer.finish().unwrap().into_inner();

    let mut cabinet = cab::Cabinet::new(Cursor::new(cab_file)).unwrap();
    {
        let folder = cabinet.folder_entries().nth(0).unwrap();
        assert_eq!(folder.compression_type(), ctype);
        assert!((folder.num_data_blocks() as usize) >= (num_bytes / 0x8000));
        let file = folder.file_entries().nth(0).unwrap();
        assert_eq!(file.name(), "binary");
        assert_eq!(file.uncompressed_size() as usize, original.len());
    }
    let mut output = Vec::<u8>::new();
    let mut file_reader = cabinet.read_file("binary").unwrap();
    file_reader.read_to_end(&mut output).unwrap();
    assert_eq!(output, original);
}

#[test]
fn cabinet_with_one_small_uncompressed_binary_file() {
    random_data_roundtrip(10_000, cab::CompressionType::None);
}

#[test]
fn cabinet_with_one_small_mszipped_binary_file() {
    random_data_roundtrip(10_000, cab::CompressionType::MsZip);
}

#[test]
fn cabinet_with_one_big_uncompressed_binary_file() {
    random_data_roundtrip(1_000_000, cab::CompressionType::None);
}

#[test]
fn cabinet_with_one_big_mszipped_binary_file() {
    random_data_roundtrip(1_000_000, cab::CompressionType::MsZip);
}

// ========================================================================= //
