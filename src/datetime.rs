use std::convert::TryInto;

use time::PrimitiveDateTime;

pub fn datetime_from_bits(date: u16, time: u16) -> Option<PrimitiveDateTime> {
    let year = (date >> 9) as i32 + 1980;
    let month = (((date >> 5) & 0xf) as u8).try_into().ok()?;
    let day = (date & 0x1f) as u8;
    let date = time::Date::from_calendar_date(year, month, day).ok()?;

    let hour = (time >> 11) as u8;
    let minute = ((time >> 5) & 0x3f) as u8;
    let second = 2 * (time & 0x1f) as u8;
    let time = time::Time::from_hms(hour, minute, second).ok()?;

    Some(PrimitiveDateTime::new(date, time))
}

pub fn datetime_to_bits(mut datetime: PrimitiveDateTime) -> (u16, u16) {
    // Clamp to legal range:
    if datetime.year() < 1980 {
        return (0x21, 0); // 1980-01-01 00:00:00
    } else if datetime.year() > 2107 {
        return (0xff9f, 0xbf7d); // 2107-12-31 23:59:59
    }

    // Round to nearest two seconds:
    if datetime.second() % 2 != 0 {
        datetime += time::Duration::seconds(1);
    }

    let year = datetime.year() as u16;
    let month = datetime.month() as u16;
    let day = datetime.day() as u16;
    let date = ((year - 1980) << 9) | (month << 5) | day;
    let hour = datetime.hour() as u16;
    let minute = datetime.minute() as u16;
    let second = datetime.second() as u16;
    let time = (hour << 11) | (minute << 5) | (second / 2);
    (date, time)
}

#[cfg(test)]
mod tests {
    use time::macros::datetime;

    use super::{datetime_from_bits, datetime_to_bits};

    #[test]
    fn valid_datetime_bits() {
        let dt = datetime!(2018-01-06 15:19:42);
        assert_eq!(datetime_to_bits(dt), (0x4c26, 0x7a75));
        assert_eq!(datetime_from_bits(0x4c26, 0x7a75), Some(dt));
    }

    #[test]
    fn datetime_outside_range() {
        let dt = datetime!(1977-02-03 4:05:06);
        let bits = datetime_to_bits(dt);
        let dt = datetime!(1980-01-01 0:00:00);
        assert_eq!(datetime_from_bits(bits.0, bits.1), Some(dt));
        assert_eq!(bits, (0x0021, 0x0000));

        let dt = datetime!(2110-02-03 4:05:06);
        let bits = datetime_to_bits(dt);
        let dt = datetime!(2107-12-31 23:59:58);
        assert_eq!(datetime_from_bits(bits.0, bits.1), Some(dt));
        assert_eq!(bits, (0xff9f, 0xbf7d));
    }

    #[test]
    fn datetime_round_to_nearest_two_seconds() {
        // Round down:
        let dt = datetime!(2012-03-04 1:02:06.900);
        let bits = datetime_to_bits(dt);
        let dt = datetime!(2012-03-04 1:02:06);
        assert_eq!(datetime_from_bits(bits.0, bits.1), Some(dt));
        assert_eq!(bits, (0x4064, 0x0843));

        // Round up:
        let dt = datetime!(2012-03-04 5:06:59.3);
        let bits = datetime_to_bits(dt);
        let dt = datetime!(2012-03-04 5:07:00);
        assert_eq!(datetime_from_bits(bits.0, bits.1), Some(dt));
        assert_eq!(bits, (0x4064, 0x28e0));
    }
}
