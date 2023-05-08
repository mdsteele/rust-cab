pub struct Checksum {
    value: u32,
    remainder: u32,
    remainder_shift: u32,
}

impl Checksum {
    pub fn new() -> Checksum {
        Checksum { value: 0, remainder: 0, remainder_shift: 0 }
    }

    pub fn value(&self) -> u32 {
        match self.remainder_shift {
            0 => self.value,
            8 => self.value ^ self.remainder,
            16 => {
                self.value
                    ^ (self.remainder >> 8)
                    ^ ((self.remainder & 0xff) << 8)
            }
            24 => {
                self.value
                    ^ (self.remainder >> 16)
                    ^ (self.remainder & 0xff00)
                    ^ ((self.remainder & 0xff) << 16)
            }
            _ => unreachable!(),
        }
    }

    pub fn update(&mut self, buf: &[u8]) {
        for &byte in buf {
            self.remainder |= (byte as u32) << self.remainder_shift;
            if self.remainder_shift == 24 {
                self.value ^= self.remainder;
                self.remainder = 0;
                self.remainder_shift = 0;
            } else {
                self.remainder_shift += 8;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Checksum;

    #[test]
    fn empty_checksum() {
        assert_eq!(Checksum::new().value(), 0);
    }

    #[test]
    fn simple_checksums() {
        let mut checksum = Checksum::new();
        checksum.update(b"\x0e\0\x0e\0Hello, world!\n");
        assert_eq!(checksum.value(), 0x7f2e1a4c);

        let mut checksum = Checksum::new();
        checksum.update(b"\x1d\0\x1d\0Hello, world!\nSee you later!\n");
        assert_eq!(checksum.value(), 0x3509541a);
    }

    #[test]
    fn checksum_from_cab_spec() {
        // This comes from the example cabinet file found in the CAB spec.
        let mut checksum = Checksum::new();
        checksum.update(
            b"\x97\0\x97\0#include <stdio.h>\r\n\r\n\
              void main(void)\r\n{\r\n    \
              printf(\"Hello, world!\\n\");\r\n}\r\n\
              #include <stdio.h>\r\n\r\n\
              void main(void)\r\n{\r\n    \
              printf(\"Welcome!\\n\");\r\n}\r\n\r\n",
        );
        assert_eq!(checksum.value(), 0x30a65abd);
    }
}
