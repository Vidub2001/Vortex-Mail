//! Dates IMAP `INTERNALDATE` : `"02-Nov-2026 10:00:00 +0100"`.

const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// Jours depuis le 1970-01-01 (algorithme de H. Hinnant).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// Timestamp Unix (secondes) d'une date IMAP, ou `None` si illisible.
#[must_use]
pub fn parse_internal_date(s: &str) -> Option<i64> {
    let mut parts = s.split_whitespace();
    let date = parts.next()?;
    let time = parts.next()?;
    let zone = parts.next()?;

    let mut d = date.split('-');
    let day: i64 = d.next()?.trim().parse().ok()?;
    let month_name = d.next()?;
    let month = i64::try_from(MONTHS.iter().position(|m| m.eq_ignore_ascii_case(month_name))?).ok()? + 1;
    let year: i64 = d.next()?.parse().ok()?;

    let mut t = time.split(':');
    let hour: i64 = t.next()?.parse().ok()?;
    let minute: i64 = t.next()?.parse().ok()?;
    let sec: i64 = t.next()?.parse().ok()?;
    // Bornes avant tout calcul : une date venue du serveur peut annoncer
    // l'an 2026429496729 et faire déborder l'arithmétique (fuzzing, lot 8).
    if !(1..=31).contains(&day)
        || !(0..=9999).contains(&year)
        || !(0..=23).contains(&hour)
        || !(0..=59).contains(&minute)
        || !(0..=60).contains(&sec)
    {
        return None;
    }

    let (sign, digits) = match zone.as_bytes().first()? {
        b'+' => (1, &zone[1..]),
        b'-' => (-1, &zone[1..]),
        _ => return None,
    };
    // Quatre chiffres ASCII : sinon, découper par octets pourrait tomber au
    // milieu d'un caractère (« +1é1 ») et faire paniquer (relecture Y9).
    if digits.len() != 4 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let zh: i64 = digits[..2].parse().ok()?;
    let zm: i64 = digits[2..].parse().ok()?;
    let offset = sign * (zh * 3600 + zm * 60);

    Some(days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + sec - offset)
}

/// Date IMAP (en UTC) pour `APPEND`.
#[must_use]
pub fn format_internal_date(ts: i64) -> String {
    let days = ts.div_euclid(86_400);
    let secs = ts.rem_euclid(86_400);
    let (y, mo, d) = civil_from_days(days);
    format!(
        "{d:02}-{}-{y:04} {:02}:{:02}:{:02} +0000",
        MONTHS[usize::try_from(mo - 1).unwrap_or_default()],
        secs / 3600,
        secs % 3600 / 60,
        secs % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_zone_is_rejected_without_panic() {
        assert_eq!(parse_internal_date("01-Jan-2026 10:00:00 +1é1"), None);
        assert_eq!(parse_internal_date("01-Jan-2026 10:00:00 +1\u{FFFD}"), None);
        assert_eq!(parse_internal_date("01-Jan-2026 10:00:00 +12a4"), None);
    }

    #[test]
    fn out_of_range_values_are_rejected_without_overflow() {
        assert_eq!(parse_internal_date("02-Nov-20264294967296 10:00:00 +0100"), None);
        assert_eq!(parse_internal_date("02-Nov-2026 9223372036854775807:00:00 +0100"), None);
        assert_eq!(parse_internal_date("-9223372036854775808-Nov-2026 10:00:00 +0100"), None);
        assert_eq!(parse_internal_date("32-Nov-2026 10:00:00 +0100"), None);
        assert_eq!(parse_internal_date("02-Nov-2026 10:60:00 +0100"), None);
        assert_eq!(parse_internal_date("31-Dec-9999 23:59:60 -2359"), Some(253_402_387_140));
    }

    #[test]
    fn parse_known_dates() {
        assert_eq!(parse_internal_date("01-Jan-1970 00:00:00 +0000"), Some(0));
        // 2026-11-02 10:00:00 +0100 = 09:00:00 UTC
        assert_eq!(parse_internal_date("02-Nov-2026 10:00:00 +0100"), Some(1_793_610_000));
        assert_eq!(parse_internal_date(" 2-Nov-2026 10:00:00 +0100"), Some(1_793_610_000));
        assert_eq!(parse_internal_date("29-Feb-2024 23:59:59 -0230"), Some(1_709_260_199));
        assert_eq!(parse_internal_date("n'importe quoi"), None);
    }

    #[test]
    fn format_roundtrip() {
        for ts in [0, 951_782_400, 1_709_268_599, 1_793_610_000, 4_102_444_800] {
            assert_eq!(parse_internal_date(&format_internal_date(ts)), Some(ts));
        }
        assert_eq!(format_internal_date(1_793_610_000), "02-Nov-2026 09:00:00 +0000");
    }
}
