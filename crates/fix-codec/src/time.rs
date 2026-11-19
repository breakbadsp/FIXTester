/// A UTC time in milliseconds since 1970-01-01T00:00:00Z.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct UtcMillis(pub u64);

const MILLIS_PER_DAY: u64 = 86_400_000;
/// The length of `YYYYMMDD-HH:MM:SS.sss`.
const TIMESTAMP_LEN: usize = 21;

/// Writes `YYYYMMDD-HH:MM:SS.sss`, the UTCTimestamp format with milliseconds.
/// FIX 4.2, 4.3, and 4.4 all accept it.
pub fn write_utc_timestamp(time: UtcMillis, out: &mut [u8; TIMESTAMP_LEN]) -> &[u8] {
    let days = time.0 / MILLIS_PER_DAY;
    let millis_of_day = time.0 % MILLIS_PER_DAY;
    let (year, month, day) = civil_from_days(days);
    assert!(year <= 9999, "a FIX year has four digits");
    *out = *b"00000000-00:00:00.000";
    put_digits(&mut out[0..4], year);
    put_digits(&mut out[4..6], month);
    put_digits(&mut out[6..8], day);
    put_digits(&mut out[9..11], millis_of_day / 3_600_000);
    put_digits(&mut out[12..14], millis_of_day / 60_000 % 60);
    put_digits(&mut out[15..17], millis_of_day / 1000 % 60);
    put_digits(&mut out[18..21], millis_of_day % 1000);
    out
}

/// Writes `value` as decimal digits into all of `out`, with leading zeros.
fn put_digits(out: &mut [u8], mut value: u64) {
    for slot in out.iter_mut().rev() {
        *slot = b'0' + u8::try_from(value % 10).unwrap_or_default();
        value /= 10;
    }
}

/// Parses a UTCTimestamp: `YYYYMMDD-HH:MM:SS`, with an optional fraction of
/// 1 to 9 digits. Digits after milliseconds are dropped.
pub fn parse_utc_timestamp(text: &[u8]) -> Option<UtcMillis> {
    if text.len() < 17 || text[8] != b'-' || text[11] != b':' || text[14] != b':' {
        return None;
    }
    let number = |range: std::ops::Range<usize>| crate::parse_u64(&text[range]);
    let (year, month, day) = (number(0..4)?, number(4..6)?, number(6..8)?);
    let (hour, minute, second) = (number(9..11)?, number(12..14)?, number(15..17)?);
    let valid_date = (1..=12).contains(&month) && (1..=days_in_month(year, month)).contains(&day);
    // A second of 60 is a leap second.
    if !valid_date || hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    let millis = match &text[17..] {
        [] => 0,
        [b'.', fraction @ ..] if (1..=9).contains(&fraction.len()) => {
            let digits = &fraction[..fraction.len().min(3)];
            let value = crate::parse_u64(digits)?;
            let scale = [100, 10, 1][digits.len() - 1];
            crate::parse_u64(fraction)?;
            value * scale
        }
        _ => return None,
    };
    let days = days_from_civil(year, month, day)?;
    let seconds = hour * 3600 + minute * 60 + second;
    Some(UtcMillis(days * MILLIS_PER_DAY + seconds * 1000 + millis))
}

fn days_in_month(year: u64, month: u64) -> u64 {
    let leap = (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400);
    match month {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

// The two functions below are the civil calendar algorithms of Howard
// Hinnant, for dates from 1970.

fn days_from_civil(year: u64, month: u64, day: u64) -> Option<u64> {
    if year < 1970 {
        return None;
    }
    let year = if month <= 2 { year - 1 } else { year };
    let era = year / 400;
    let year_of_era = year - era * 400;
    let month_index = (month + 9) % 12;
    let day_of_year = (153 * month_index + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    Some(era * 146_097 + day_of_era - 719_468)
}

fn civil_from_days(days: u64) -> (u64, u64, u64) {
    let days = days + 719_468;
    let era = days / 146_097;
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + u64::from(month <= 2);
    (year, month, day)
}
