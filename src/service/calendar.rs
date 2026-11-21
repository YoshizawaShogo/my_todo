//! 平日の計算：土日を休みとする（祝日は扱わない）。

use jiff::ToSpan;
use jiff::civil::Date;

use super::{ServiceError, ServiceResult};

/// 平日（月〜金）か
pub fn is_weekday(d: Date) -> bool {
    d.weekday().to_monday_one_offset() <= 5
}

fn next_day(d: Date) -> ServiceResult<Date> {
    d.checked_add(1.day())
        .map_err(|e| ServiceError::Invalid(format!("日付を計算できません: {e}")))
}

/// base から d の前日までの平日の数（d が土日なら、次の月曜日と同じ数になる）。d < base なら 0
pub fn weekdays_between(base: Date, d: Date) -> ServiceResult<i64> {
    let mut n = 0;
    let mut cur = base;
    while cur < d {
        if is_weekday(cur) {
            n += 1;
        }
        cur = next_day(cur)?;
    }
    Ok(n)
}

/// start（土日なら次の月曜日）から n 平日あとの日
pub fn add_weekdays(start: Date, n: i64) -> ServiceResult<Date> {
    let mut d = start;
    while !is_weekday(d) {
        d = next_day(d)?;
    }
    for _ in 0..n.max(0) {
        d = next_day(d)?;
        while !is_weekday(d) {
            d = next_day(d)?;
        }
    }
    Ok(d)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> Date {
        s.parse().unwrap()
    }

    #[test]
    fn weekday_arithmetic() {
        // 2026-11-18（水）から
        assert_eq!(
            weekdays_between(d("2026-11-18"), d("2026-11-18")).unwrap(),
            0
        );
        assert_eq!(
            weekdays_between(d("2026-11-18"), d("2026-11-20")).unwrap(),
            2
        );
        assert_eq!(
            weekdays_between(d("2026-11-18"), d("2026-11-23")).unwrap(),
            3
        );
        // 土日は次の月曜日と同じ数
        assert_eq!(
            weekdays_between(d("2026-11-18"), d("2026-11-21")).unwrap(),
            3
        );
        assert_eq!(add_weekdays(d("2026-11-18"), 3).unwrap(), d("2026-11-23"));
        // 土日から始めると次の月曜日から
        assert_eq!(add_weekdays(d("2026-11-21"), 0).unwrap(), d("2026-11-23"));
        assert_eq!(add_weekdays(d("2026-11-20"), 1).unwrap(), d("2026-11-23"));
    }
}
