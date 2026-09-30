use crate::{StorageError, StorageResult};

/// # Errors
/// 日期须是有效的公历 YYYY-MM-DD（0001 到 9999 年）。
pub fn validate_utc_day(day: &str) -> StorageResult<()> {
    let valid = || {
        if day.len() != 10
            || !day.is_ascii()
            || day.as_bytes()[4] != b'-'
            || day.as_bytes()[7] != b'-'
            || ![&day[..4], &day[5..7], &day[8..]]
                .iter()
                .all(|s| s.bytes().all(|b| b.is_ascii_digit()))
        {
            return None;
        }
        let year: u32 = day[..4].parse().ok()?;
        let month: u32 = day[5..7].parse().ok()?;
        let date: u32 = day[8..].parse().ok()?;
        let days = match month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 if year.is_multiple_of(400)
                || (year.is_multiple_of(4) && !year.is_multiple_of(100)) =>
            {
                29
            }
            2 => 28,
            _ => return None,
        };
        (year > 0 && date > 0 && date <= days).then_some(())
    };
    valid().ok_or_else(|| StorageError::InvalidData("invalid UTC day".into()))
}
