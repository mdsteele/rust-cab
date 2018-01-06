use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime, Timelike};

// ========================================================================= //

pub fn datetime_from_bits(date: u16, time: u16) -> Option<NaiveDateTime> {
    let year = (date >> 9) as i32 + 1980;
    let month = ((date >> 5) & 0xf) as u32;
    let day = (date & 0x1f) as u32;
    let naive_date = match NaiveDate::from_ymd_opt(year, month, day) {
        Some(naive_date) => naive_date,
        None => return None,
    };
    let hour = (time >> 11) as u32;
    let minute = ((time >> 5) & 0x3f) as u32;
    let second = 2 * (time & 0x1f) as u32;
    naive_date.and_hms_opt(hour, minute, second)
}

pub fn datetime_to_bits(mut datetime: NaiveDateTime) -> (u16, u16) {
    // Clamp to legal range:
    if datetime.year() < 1980 {
        return (0x21, 0); // 1980-01-01 00:00:00
    } else if datetime.year() > 2107 {
        return (0xff9f, 0xbf7d); // 2107-12-31 23:59:59
    }

    // Round to nearest two seconds:
    if datetime.second() % 2 != 0 {
        datetime += Duration::seconds(1);
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

// ========================================================================= //

#[cfg(test)]
mod tests {
    use super::{datetime_from_bits, datetime_to_bits};
    use chrono::NaiveDate;

    #[test]
    fn valid_datetime_bits() {
        let dt = NaiveDate::from_ymd(2018, 1, 6).and_hms(15, 19, 42);
        assert_eq!(datetime_to_bits(dt), (0x4c26, 0x7a75));
        assert_eq!(datetime_from_bits(0x4c26, 0x7a75), Some(dt));
    }

    #[test]
    fn datetime_outside_range() {
        let dt = NaiveDate::from_ymd(1977, 2, 3).and_hms(4, 5, 6);
        let bits = datetime_to_bits(dt);
        let dt = NaiveDate::from_ymd(1980, 1, 1).and_hms(0, 0, 0);
        assert_eq!(datetime_from_bits(bits.0, bits.1), Some(dt));
        assert_eq!(bits, (0x0021, 0x0000));

        let dt = NaiveDate::from_ymd(2110, 2, 3).and_hms(4, 5, 6);
        let bits = datetime_to_bits(dt);
        let dt = NaiveDate::from_ymd(2107, 12, 31).and_hms(23, 59, 58);
        assert_eq!(datetime_from_bits(bits.0, bits.1), Some(dt));
        assert_eq!(bits, (0xff9f, 0xbf7d));
    }

    #[test]
    fn datetime_round_to_nearest_two_seconds() {
        // Round down:
        let dt = NaiveDate::from_ymd(2012, 3, 4).and_hms_milli(1, 2, 6, 900);
        let bits = datetime_to_bits(dt);
        let dt = NaiveDate::from_ymd(2012, 3, 4).and_hms(1, 2, 6);
        assert_eq!(datetime_from_bits(bits.0, bits.1), Some(dt));
        assert_eq!(bits, (0x4064, 0x0843));

        // Round up:
        let dt = NaiveDate::from_ymd(2012, 3, 4).and_hms_milli(5, 6, 59, 3);
        let bits = datetime_to_bits(dt);
        let dt = NaiveDate::from_ymd(2012, 3, 4).and_hms(5, 7, 0);
        assert_eq!(datetime_from_bits(bits.0, bits.1), Some(dt));
        assert_eq!(bits, (0x4064, 0x28e0));
    }
}

// ========================================================================= //
