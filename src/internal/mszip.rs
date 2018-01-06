use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use flate2::Compression;
use flate2::read::DeflateDecoder;
use flate2::write::DeflateEncoder;
use std::io::{self, Read, Seek, SeekFrom, Write};

// ========================================================================= //

const MSZIP_SIGNATURE: u16 = 0x4B43; // "CK" stored little-endian
const MSZIP_SIGNATURE_LEN: u64 = 2;

// ========================================================================= //

pub struct MsZipReader<R> {
    decoder: Option<DeflateDecoder<R>>,
    total_uncompressed_size: Option<u64>,
}

impl<R: Read> MsZipReader<R> {
    pub fn new(mut reader: R) -> io::Result<MsZipReader<R>> {
        let signature = reader.read_u16::<LittleEndian>()?;
        if signature != MSZIP_SIGNATURE {
            invalid_data!("Invalid MSZIP signature");
        }
        Ok(MsZipReader {
               decoder: Some(DeflateDecoder::new(reader)),
               total_uncompressed_size: None,
           })
    }

    fn decoder(&mut self) -> &DeflateDecoder<R> {
        self.decoder.as_ref().unwrap()
    }

    fn decoder_mut(&mut self) -> &mut DeflateDecoder<R> {
        self.decoder.as_mut().unwrap()
    }
}

impl<R: Read + Seek> MsZipReader<R> {
    fn seek_to_start(&mut self) -> io::Result<()> {
        self.decoder_mut()
            .get_mut()
            .seek(SeekFrom::Start(MSZIP_SIGNATURE_LEN))?;
        let reader = self.decoder.take().unwrap().into_inner();
        self.decoder = Some(DeflateDecoder::new(reader));
        Ok(())
    }
}

impl<R: Read> Read for MsZipReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.decoder_mut().read(buf)
    }
}

impl<R: Read + Seek> Seek for MsZipReader<R> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let advance_by: u64 = match pos {
            SeekFrom::Start(new_offset) => {
                let mut current_offset = self.decoder().total_out();
                if new_offset < current_offset {
                    self.seek_to_start()?;
                    current_offset = 0;
                }
                new_offset - current_offset
            }
            SeekFrom::Current(delta) => {
                if delta >= 0 {
                    delta as u64
                } else {
                    let old_offset = self.decoder().total_out();
                    let delta = (-delta) as u64;
                    if delta > old_offset {
                        invalid_input!("seek out of range");
                    }
                    self.seek_to_start()?;
                    old_offset - delta
                }
            }
            SeekFrom::End(delta) => {
                let total_size = match self.total_uncompressed_size {
                    Some(size) => size,
                    None => {
                        io::copy(self.decoder_mut(), &mut io::sink())?;
                        let size = self.decoder().total_out();
                        self.total_uncompressed_size = Some(size);
                        size
                    }
                };
                let current_offset = self.decoder().total_out();
                debug_assert!(current_offset <= total_size);
                if delta >= 0 {
                    total_size - current_offset
                } else {
                    let delta = (-delta) as u64;
                    if delta > total_size {
                        invalid_input!("seek out of range");
                    }
                    let new_offset = total_size - delta;
                    if new_offset < current_offset {
                        self.seek_to_start()?;
                        new_offset
                    } else {
                        new_offset - current_offset
                    }
                }
            }
        };
        if advance_by > 0 {
            io::copy(&mut self.decoder_mut().by_ref().take(advance_by),
                     &mut io::sink())?;
        }
        Ok(self.decoder().total_out())
    }
}

// ========================================================================= //

pub struct MsZipWriter<W: Write> {
    encoder: DeflateEncoder<W>,
}

impl<W: Write> MsZipWriter<W> {
    pub fn new(mut writer: W) -> io::Result<MsZipWriter<W>> {
        writer.write_u16::<LittleEndian>(MSZIP_SIGNATURE)?;
        Ok(MsZipWriter {
               encoder: DeflateEncoder::new(writer, Compression::best()),
           })
    }

    pub fn get_mut(&mut self) -> &mut W { self.encoder.get_mut() }

    pub fn finish(self) -> io::Result<W> { self.encoder.finish() }
}

impl<W: Write> Write for MsZipWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.encoder.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> { self.encoder.flush() }
}

// ========================================================================= //

#[cfg(test)]
mod tests {
    use super::{MsZipReader, MsZipWriter};
    use std::io::{Cursor, Read, Seek, SeekFrom, Write};

    #[test]
    fn read_compressed_data() {
        let input: &[u8] = b"CK%\xcc\xd1\t\x031\x0c\x04\xd1V\xb6\x80#\x95\xa4\
              \t\xc5\x12\xc7\x82e\xfb,\xa9\xff\x18\xee{x\xf3\x9d\xdb\x1c\\Q\
              \x0e\x9d}n\x04\x13\xe2\x96\x17\xda\x1ca--kC\x94\x8b\xd18nX\xe7\
              \x89az\x00\x8c\x15>\x15i\xbe\x0e\xe6hTj\x8dD%\xba\xfc\xce\x1e\
              \x96\xef\xda\xe0r\x0f\x81t>%\x9f?\x12]-\x87";
        let expected: &[u8] =
            b"Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed \
              do eiusmod tempor incididunt ut labore et dolore magna aliqua.";
        assert!(input.len() < expected.len());
        let mut mszip_reader = MsZipReader::new(input).unwrap();
        let mut output = Vec::new();
        mszip_reader.read_to_end(&mut output).unwrap();
        assert_eq!(output, expected);
    }

    #[test]
    fn seek_and_read() {
        let input: &[u8] = b"CK%\xcc\xd1\t\x031\x0c\x04\xd1V\xb6\x80#\x95\xa4\
              \t\xc5\x12\xc7\x82e\xfb,\xa9\xff\x18\xee{x\xf3\x9d\xdb\x1c\\Q\
              \x0e\x9d}n\x04\x13\xe2\x96\x17\xda\x1ca--kC\x94\x8b\xd18nX\xe7\
              \x89az\x00\x8c\x15>\x15i\xbe\x0e\xe6hTj\x8dD%\xba\xfc\xce\x1e\
              \x96\xef\xda\xe0r\x0f\x81t>%\x9f?\x12]-\x87";
        let mut mszip_reader = MsZipReader::new(Cursor::new(input)).unwrap();

        mszip_reader.seek(SeekFrom::Start(12)).unwrap();
        let mut output = vec![0u8; 5];
        mszip_reader.read_exact(&mut output).unwrap();
        assert_eq!(output, b"dolor");

        mszip_reader.seek(SeekFrom::Start(6)).unwrap();
        let mut output = vec![0u8; 5];
        mszip_reader.read_exact(&mut output).unwrap();
        assert_eq!(output, b"ipsum");

        mszip_reader.seek(SeekFrom::Current(17)).unwrap();
        let mut output = vec![0u8; 11];
        mszip_reader.read_exact(&mut output).unwrap();
        assert_eq!(output, b"consectetur");

        mszip_reader.seek(SeekFrom::Current(-21)).unwrap();
        let mut output = vec![0u8; 8];
        mszip_reader.read_exact(&mut output).unwrap();
        assert_eq!(output, b"sit amet");

        mszip_reader.seek(SeekFrom::End(-13)).unwrap();
        let mut output = vec![0u8; 5];
        mszip_reader.read_exact(&mut output).unwrap();
        assert_eq!(output, b"magna");

        mszip_reader.seek(SeekFrom::End(-7)).unwrap();
        let mut output = vec![0u8; 6];
        mszip_reader.read_exact(&mut output).unwrap();
        assert_eq!(output, b"aliqua");

        mszip_reader.seek(SeekFrom::End(5)).unwrap();
        let mut output = Vec::new();
        mszip_reader.read_to_end(&mut output).unwrap();
        assert!(output.is_empty());
    }

    #[test]
    fn compression_round_trip() {
        let original: &[u8] =
            b"Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed \
              do eiusmod tempor incididunt ut labore et dolore magna aliqua.";
        let mut compressed = Vec::new();
        {
            let mut mszip_writer = MsZipWriter::new(&mut compressed).unwrap();
            mszip_writer.write_all(original).unwrap();
        }
        assert!(compressed.len() < original.len());
        let mut output = Vec::new();
        let mut mszip_reader = MsZipReader::new(compressed.as_slice())
            .unwrap();
        mszip_reader.read_to_end(&mut output).unwrap();
        assert_eq!(output, original);
    }
}

// ========================================================================= //
