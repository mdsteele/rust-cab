extern crate cab;
extern crate lipsum;

use std::io::{Cursor, Read, Seek, SeekFrom, Write};

// ========================================================================= //

#[test]
fn seek_within_big_uncompressed_file() {
    let original_string = lipsum::lipsum(30000);
    let original_bytes = original_string.as_bytes();

    let mut cab_builder = cab::CabinetBuilder::new();
    cab_builder
        .add_folder(cab::CompressionType::None)
        .add_file("lorem_ipsum.txt");
    let mut cab_writer = cab_builder.build(Cursor::new(Vec::new())).unwrap();
    while let Some(mut file_writer) = cab_writer.next_file().unwrap() {
        file_writer.write_all(original_bytes).unwrap();
    }
    let cab_file = cab_writer.finish().unwrap().into_inner();
    assert!(cab_file.len() > original_bytes.len());

    let mut cabinet = cab::Cabinet::new(Cursor::new(cab_file)).unwrap();
    let mut file_reader = cabinet.read_file("lorem_ipsum.txt").unwrap();
    let mut offset: usize = 1000;
    while offset < original_bytes.len() {
        let start = file_reader.seek(SeekFrom::End(-(offset as i64))).unwrap();
        let mut output = vec![0u8; 1000];
        file_reader.read_exact(&mut output).unwrap();
        assert_eq!(
            &output as &[u8],
            &original_bytes[(start as usize)..][..1000]
        );
        offset += 1000;
    }
}

#[test]
fn seek_within_big_mszipped_file() {
    let original_string = lipsum::lipsum(30000);
    let original_bytes = original_string.as_bytes();

    let mut cab_builder = cab::CabinetBuilder::new();
    cab_builder
        .add_folder(cab::CompressionType::MsZip)
        .add_file("lorem_ipsum.txt");
    let mut cab_writer = cab_builder.build(Cursor::new(Vec::new())).unwrap();
    while let Some(mut file_writer) = cab_writer.next_file().unwrap() {
        file_writer.write_all(original_bytes).unwrap();
    }
    let cab_file = cab_writer.finish().unwrap().into_inner();
    assert!(cab_file.len() < original_bytes.len());

    let mut cabinet = cab::Cabinet::new(Cursor::new(cab_file)).unwrap();
    let mut file_reader = cabinet.read_file("lorem_ipsum.txt").unwrap();
    let mut offset: usize = 1000;
    while offset < original_bytes.len() {
        let start = file_reader.seek(SeekFrom::End(-(offset as i64))).unwrap();
        let mut output = vec![0u8; 1000];
        file_reader.read_exact(&mut output).unwrap();
        assert_eq!(
            &output as &[u8],
            &original_bytes[(start as usize)..][..1000]
        );
        offset += 1000;
    }
}

// Regression test for https://github.com/mdsteele/rust-cab/issues/15
#[test]
fn seek_within_empty_file() {
    let mut cab_builder = cab::CabinetBuilder::new();
    cab_builder.add_folder(cab::CompressionType::None).add_file("empty.txt");
    let mut cab_writer = cab_builder.build(Cursor::new(Vec::new())).unwrap();
    while let Some(mut file_writer) = cab_writer.next_file().unwrap() {
        file_writer.write_all(b"").unwrap();
    }
    let cab_file = cab_writer.finish().unwrap().into_inner();

    let mut cabinet = cab::Cabinet::new(Cursor::new(cab_file)).unwrap();
    for folder in cabinet.folder_entries() {
        assert_eq!(folder.num_data_blocks(), 0);
    }
    let mut file_reader = cabinet.read_file("empty.txt").unwrap();
    file_reader.seek(SeekFrom::Start(0)).unwrap();
    let mut data = Vec::<u8>::new();
    file_reader.read_to_end(&mut data).unwrap();
    assert!(data.is_empty());
}

// ========================================================================= //
