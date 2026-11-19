//! Checks that a field value has the syntax of its FIX data type.

use crate::schema::FieldType;
use fix_codec::parse_utc_timestamp;

/// Returns true when `value` is a valid text for `ty`. A value in the
/// message cannot be empty or contain SOH, so those cases need no check.
pub fn matches_type(ty: FieldType, value: &[u8]) -> bool {
    match ty {
        FieldType::String | FieldType::Data => true,
        FieldType::Exchange => !value.is_empty(),
        FieldType::Int => is_int(value),
        FieldType::Length | FieldType::SeqNum | FieldType::NumInGroup => is_digits(value),
        FieldType::DayOfMonth => {
            is_digits(value) && number(value).is_some_and(|n| (1..=31).contains(&n))
        }
        FieldType::Float
        | FieldType::Qty
        | FieldType::Price
        | FieldType::PriceOffset
        | FieldType::Amt
        | FieldType::Percentage => is_decimal(value),
        FieldType::Char => value.len() == 1,
        FieldType::Boolean => matches!(value, b"Y" | b"N"),
        FieldType::Currency => is_letters(value, 3),
        FieldType::Country => is_letters(value, 2),
        FieldType::MultipleValueString => is_multiple_value(value),
        FieldType::UtcTimestamp => parse_utc_timestamp(value).is_some(),
        FieldType::UtcTimeOnly => is_time_only(value),
        FieldType::UtcDate | FieldType::LocalMktDate => is_date(value),
        FieldType::MonthYear => is_month_year(value),
    }
}

fn is_digits(value: &[u8]) -> bool {
    !value.is_empty() && value.iter().all(u8::is_ascii_digit)
}

fn number(value: &[u8]) -> Option<u32> {
    if value.len() > 9 || !is_digits(value) {
        return None;
    }
    Some(value.iter().fold(0, |n, d| n * 10 + u32::from(d - b'0')))
}

fn is_int(value: &[u8]) -> bool {
    is_digits(value.strip_prefix(b"-").unwrap_or(value))
}

/// A decimal number: an optional minus sign, digits, and an optional
/// fraction. No exponent.
fn is_decimal(value: &[u8]) -> bool {
    let unsigned = value.strip_prefix(b"-").unwrap_or(value);
    let mut parts = unsigned.splitn(2, |byte| *byte == b'.');
    let whole = parts.next().unwrap_or_default();
    let fraction = parts.next();
    let digits = |part: &[u8]| part.iter().all(u8::is_ascii_digit);
    digits(whole)
        && fraction.is_none_or(digits)
        && (!whole.is_empty() || fraction.is_some_and(|f| !f.is_empty()))
}

fn is_letters(value: &[u8], len: usize) -> bool {
    value.len() == len && value.iter().all(u8::is_ascii_uppercase)
}

/// Values that are separated by single spaces.
fn is_multiple_value(value: &[u8]) -> bool {
    value
        .split(|byte| *byte == b' ')
        .all(|part| !part.is_empty())
}

fn is_time_only(value: &[u8]) -> bool {
    let fraction_ok = match value.get(8..) {
        Some([]) => true,
        Some([b'.', fraction @ ..]) => (1..=9).contains(&fraction.len()) && is_digits(fraction),
        _ => false,
    };
    value.len() >= 8
        && value[2] == b':'
        && value[5] == b':'
        && fraction_ok
        && number(&value[0..2]).is_some_and(|h| h < 24)
        && number(&value[3..5]).is_some_and(|m| m < 60)
        && number(&value[6..8]).is_some_and(|s| s <= 60)
}

fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        2 if year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400)) => {
            29
        }
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

fn is_date(value: &[u8]) -> bool {
    if value.len() != 8 {
        return false;
    }
    let (Some(year), Some(month), Some(day)) = (
        number(&value[0..4]),
        number(&value[4..6]),
        number(&value[6..8]),
    ) else {
        return false;
    };
    (1..=12).contains(&month) && (1..=days_in_month(year, month)).contains(&day)
}

/// `YYYYMM`, `YYYYMMDD`, or `YYYYMMwN` with N from 1 to 5.
fn is_month_year(value: &[u8]) -> bool {
    let month_ok = value.len() >= 6
        && number(&value[0..4]).is_some()
        && number(&value[4..6]).is_some_and(|m| (1..=12).contains(&m));
    month_ok
        && match value.len() {
            6 => true,
            8 if value[6] == b'w' => matches!(value[7], b'1'..=b'5'),
            8 => is_date(value),
            _ => false,
        }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(ty: FieldType, text: &str) -> bool {
        matches_type(ty, text.as_bytes())
    }

    #[test]
    fn numbers() {
        assert!(ok(FieldType::Int, "-12"));
        assert!(!ok(FieldType::Int, "1.5"));
        assert!(!ok(FieldType::Int, "-"));
        assert!(ok(FieldType::Qty, "100"));
        assert!(ok(FieldType::Price, "-0.25"));
        assert!(ok(FieldType::Price, ".5"));
        assert!(!ok(FieldType::Price, "1e5"));
        assert!(!ok(FieldType::Price, "."));
        assert!(!ok(FieldType::Length, "-1"));
        assert!(ok(FieldType::DayOfMonth, "31"));
        assert!(!ok(FieldType::DayOfMonth, "32"));
    }

    #[test]
    fn text_types() {
        assert!(ok(FieldType::Char, "A"));
        assert!(!ok(FieldType::Char, "AB"));
        assert!(ok(FieldType::Boolean, "Y"));
        assert!(!ok(FieldType::Boolean, "y"));
        assert!(ok(FieldType::Currency, "USD"));
        assert!(!ok(FieldType::Currency, "usd"));
        assert!(ok(FieldType::Country, "US"));
        assert!(ok(FieldType::MultipleValueString, "1 2 A"));
        assert!(!ok(FieldType::MultipleValueString, "1  2"));
    }

    #[test]
    fn dates_and_times() {
        assert!(ok(FieldType::UtcTimestamp, "20261118-12:30:45.123"));
        assert!(ok(FieldType::UtcTimestamp, "20261118-12:30:45"));
        assert!(!ok(FieldType::UtcTimestamp, "20261318-12:30:45"));
        assert!(ok(FieldType::UtcTimeOnly, "23:59:60"));
        assert!(!ok(FieldType::UtcTimeOnly, "24:00:00"));
        assert!(ok(FieldType::UtcDate, "20240229"));
        assert!(!ok(FieldType::UtcDate, "20230229"));
        assert!(ok(FieldType::LocalMktDate, "20261118"));
        assert!(ok(FieldType::MonthYear, "202611"));
        assert!(ok(FieldType::MonthYear, "20261118"));
        assert!(ok(FieldType::MonthYear, "202611w2"));
        assert!(!ok(FieldType::MonthYear, "202611w6"));
        assert!(!ok(FieldType::MonthYear, "202613"));
    }
}
