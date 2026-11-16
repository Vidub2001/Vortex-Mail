//! Fuseaux horaires des rendez-vous (lot 6, relecture F1).
//!
//! Un rendez-vous est enregistré à l'heure locale de son fuseau
//! (`DTSTART;TZID=Europe/Paris:20261109T090000`) : il reste à 9 h après un
//! changement d'heure, chez tous les clients. La norme demande alors un bloc
//! `VTIMEZONE` qui décrit le fuseau ; il est calculé ici à partir de la base
//! des fuseaux (celle du système sous Linux, la copie embarquée par `jiff`
//! sous Windows).

use jiff::civil::{DateTime, Weekday};
use jiff::tz::TimeZone;
use jiff::Timestamp;

use crate::pimfmt::When;

const DAY: i64 = 86_400;
const WEEKDAYS: [&str; 7] = ["SU", "MO", "TU", "WE", "TH", "FR", "SA"];

/// Changement d'heure : instant (UTC), décalages avant et après, heure d'été
/// ou non, abréviation, et heure locale du changement (dans l'ancien décalage,
/// comme le veut le `DTSTART` d'un `VTIMEZONE`).
struct Transition {
    from: i32,
    to: i32,
    dst: bool,
    name: String,
    local: DateTime,
}

/// Décalage (secondes), heure d'été, abréviation à un instant.
fn offset_at(tz: &TimeZone, second: i64) -> Option<(i32, bool, String)> {
    let info = tz.to_offset_info(Timestamp::from_second(second).ok()?);
    Some((info.offset().seconds(), info.dst().is_dst(), info.abbreviation().to_string()))
}

/// Date-heure UTC d'un instant.
fn utc_civil(second: i64) -> Option<DateTime> {
    Some(Timestamp::from_second(second).ok()?.to_zoned(TimeZone::UTC).datetime())
}

/// Instant du 1er janvier d'une année, 0 h UTC.
fn year_start(year: i16) -> Option<i64> {
    Some(DateTime::new(year, 1, 1, 0, 0, 0, 0).ok()?.to_zoned(TimeZone::UTC).ok()?.timestamp().as_second())
}

/// Changements d'heure entre deux instants (recherche jour par jour, puis à la seconde).
fn transitions(tz: &TimeZone, from: i64, to: i64) -> Option<Vec<Transition>> {
    let mut out = Vec::new();
    let mut at = from;
    let mut before = offset_at(tz, at)?;
    while at < to {
        let next = at + DAY;
        let after = offset_at(tz, next)?;
        if (after.0, after.1) != (before.0, before.1) {
            let (mut lo, mut hi) = (at, next);
            while hi - lo > 1 {
                let mid = lo + (hi - lo) / 2;
                let probe = offset_at(tz, mid)?;
                if (probe.0, probe.1) == (before.0, before.1) {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            let local = utc_civil(hi + i64::from(before.0))?;
            out.push(Transition { from: before.0, to: after.0, dst: after.1, name: after.2.clone(), local });
            before = after;
        }
        at = next;
    }
    Some(out)
}

fn weekday_code(day: Weekday) -> &'static str {
    WEEKDAYS[usize::try_from(day.to_sunday_zero_offset()).unwrap_or(0)]
}

/// Années vérifiées avant d'écrire une règle annuelle sans fin : certains
/// fuseaux suivent une règle (« vendredi avant le dernier dimanche ») qui
/// coïncide quatre ans avec une règle simple, puis s'en écarte (relecture
/// du 12 novembre, n° 42).
const CHECKED_YEARS: i16 = 16;

/// Règle annuelle commune à des changements d'heure (« dernier dimanche de
/// mars à 2 h ») ; `None` si elle varie d'une année à l'autre, ou s'il n'y a
/// pas exactement un changement par année sur les `years` années observées.
fn yearly_rule(group: &[&Transition], years: usize) -> Option<String> {
    let first = group.first()?;
    if group.len() < 2 || group.len() != years {
        return None;
    }
    let mut seen: Vec<i16> = group.iter().map(|t| t.local.year()).collect();
    seen.dedup();
    if seen.len() != years {
        return None;
    }
    let (month, weekday, time) = (first.local.month(), first.local.weekday(), first.local.time());
    let regular = group.iter().all(|t| {
        t.local.month() == month && t.local.weekday() == weekday && t.local.time() == time && t.from == first.from && t.to == first.to
    });
    if !regular {
        return None;
    }
    let is_last = |dt: &DateTime| dt.day() + 7 > dt.days_in_month();
    let nth = |dt: &DateTime| (dt.day() - 1) / 7 + 1;
    let rank = if group.iter().all(|t| is_last(&t.local)) {
        "-1".to_string()
    } else if group.iter().all(|t| nth(&t.local) == nth(&first.local)) {
        nth(&first.local).to_string()
    } else {
        return None;
    };
    Some(format!("FREQ=YEARLY;BYMONTH={month};BYDAY={rank}{}", weekday_code(weekday)))
}

/// `+0100`, `-0430`, `+053045`.
fn utc_offset(seconds: i32) -> String {
    let sign = if seconds < 0 { '-' } else { '+' };
    let s = seconds.unsigned_abs();
    let (h, m, rest) = (s / 3600, s % 3600 / 60, s % 60);
    if rest == 0 {
        format!("{sign}{h:02}{m:02}")
    } else {
        format!("{sign}{h:02}{m:02}{rest:02}")
    }
}

fn stamp(dt: &DateTime) -> String {
    format!("{:04}{:02}{:02}T{:02}{:02}{:02}", dt.year(), dt.month(), dt.day(), dt.hour(), dt.minute(), dt.second())
}

fn component(dst: bool, start: &str, from: i32, to: i32, name: &str, rule: Option<&str>) -> String {
    let kind = if dst { "DAYLIGHT" } else { "STANDARD" };
    let name = if name.is_empty() { String::new() } else { format!("TZNAME:{name}\r\n") };
    let rule = rule.map_or_default(|r| format!("RRULE:{r}\r\n"));
    format!(
        "BEGIN:{kind}\r\nDTSTART:{start}\r\nTZOFFSETFROM:{}\r\nTZOFFSETTO:{}\r\n{name}{rule}END:{kind}\r\n",
        utc_offset(from),
        utc_offset(to)
    )
}

/// Bloc `VTIMEZONE` d'un fuseau IANA, valable autour de `year` : règles
/// annuelles quand elles se vérifient sur seize ans, sinon chaque changement
/// de l'année précédente aux trois suivantes ; `None` si le fuseau est inconnu.
#[must_use]
pub(crate) fn vtimezone(tzid: &str, year: i16) -> Option<String> {
    let tz = TimeZone::get(tzid).ok()?;
    let from = year_start(year.checked_sub(1)?)?;
    let list = transitions(&tz, from, year_start(year.checked_add(3)?)?)?;
    let long = transitions(&tz, from, year_start(year.checked_sub(1)?.checked_add(CHECKED_YEARS)?)?)?;
    let years = usize::try_from(CHECKED_YEARS).unwrap_or(0);
    let mut out = format!("BEGIN:VTIMEZONE\r\nTZID:{tzid}\r\n");
    if list.is_empty() {
        let (offset, dst, name) = offset_at(&tz, year_start(year)?)?;
        out.push_str(&component(dst, "19700101T000000", offset, offset, &name, None));
    }
    for dst in [false, true] {
        let group: Vec<&Transition> = list.iter().filter(|t| t.dst == dst).collect();
        let Some(first) = group.first() else { continue };
        let checked: Vec<&Transition> = long.iter().filter(|t| t.dst == dst).collect();
        match yearly_rule(&checked, years) {
            Some(rule) => out.push_str(&component(dst, &stamp(&first.local), first.from, first.to, &first.name, Some(&rule))),
            None => {
                for t in &group {
                    out.push_str(&component(dst, &stamp(&t.local), t.from, t.to, &t.name, None));
                }
            }
        }
    }
    out.push_str("END:VTIMEZONE\r\n");
    Some(out)
}

/// Instant (secondes UTC) d'une date iCalendar. Fuseau inconnu ou absent :
/// heure prise en UTC, ce qui suffit pour comparer deux valeurs d'une même série.
#[must_use]
pub(crate) fn instant(w: &When) -> Option<i64> {
    let v = w.value.trim();
    let num = |from: usize, to: usize| v.get(from..to).and_then(|s| s.parse::<i16>().ok());
    let small = |from: usize, to: usize| num(from, to).and_then(|n| i8::try_from(n).ok());
    let (year, month, day) = (num(0, 4)?, small(4, 6)?, small(6, 8)?);
    let (hour, minute, second) = if v.len() >= 15 { (small(9, 11)?, small(11, 13)?, small(13, 15)?) } else { (0, 0, 0) };
    let dt = DateTime::new(year, month, day, hour, minute, second, 0).ok()?;
    let zone =
        if v.ends_with('Z') || w.tzid.is_empty() || v.len() == 8 { TimeZone::UTC } else { TimeZone::get(&w.tzid).unwrap_or(TimeZone::UTC) };
    Some(dt.to_zoned(zone).ok()?.timestamp().as_second())
}

/// Décalage de l'heure locale du système à un instant, en secondes (0 si
/// le fuseau est illisible) : en-tête `Date` des messages envoyés.
#[must_use]
pub(crate) fn local_offset(second: i64) -> i32 {
    offset_at(&TimeZone::system(), second).map_or(0, |(offset, _, _)| offset)
}

/// Fuseau du système (nom IANA, « Europe/Paris ») ; vide s'il est inconnu.
#[must_use]
pub(crate) fn system_zone() -> String {
    TimeZone::system().iana_name().unwrap_or("").to_string()
}

/// Date civile (`AAAA-MM-JJ`) d'un instant dans un fuseau ; fuseau vide ou
/// inconnu : UTC.
#[must_use]
pub(crate) fn civil_date(second: i64, tzid: &str) -> Option<String> {
    let zone = if tzid.is_empty() { TimeZone::UTC } else { TimeZone::get(tzid).unwrap_or(TimeZone::UTC) };
    let date = Timestamp::from_second(second).ok()?.to_zoned(zone).date();
    Some(format!("{:04}-{:02}-{:02}", date.year(), date.month(), date.day()))
}

/// Même instant (ou, à défaut, même valeur).
#[must_use]
pub(crate) fn same_instant(a: &When, b: &When) -> bool {
    match (instant(a), instant(b)) {
        (Some(x), Some(y)) => x == y,
        _ => a.value.trim() == b.value.trim(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn when(value: &str, tzid: &str) -> When {
        When { value: value.into(), tzid: tzid.into() }
    }

    #[test]
    fn fuseau_avec_heure_d_ete() {
        let paris = vtimezone("Europe/Paris", 2026).expect("Europe/Paris");
        assert!(paris.starts_with("BEGIN:VTIMEZONE\r\nTZID:Europe/Paris\r\n"), "{paris}");
        assert!(paris.contains("BEGIN:DAYLIGHT\r\nDTSTART:20250330T020000\r\nTZOFFSETFROM:+0100\r\nTZOFFSETTO:+0200\r\nTZNAME:CEST\r\nRRULE:FREQ=YEARLY;BYMONTH=3;BYDAY=-1SU\r\n"), "{paris}");
        assert!(paris.contains("BEGIN:STANDARD\r\nDTSTART:20251026T030000\r\nTZOFFSETFROM:+0200\r\nTZOFFSETTO:+0100\r\nTZNAME:CET\r\nRRULE:FREQ=YEARLY;BYMONTH=10;BYDAY=-1SU\r\n"), "{paris}");
        let new_york = vtimezone("America/New_York", 2026).expect("America/New_York");
        assert!(new_york.contains("RRULE:FREQ=YEARLY;BYMONTH=3;BYDAY=2SU"), "{new_york}");
        assert!(new_york.contains("RRULE:FREQ=YEARLY;BYMONTH=11;BYDAY=1SU"), "{new_york}");
    }

    #[test]
    fn regle_verifiee_sur_seize_ans() {
        // Relecture du 12 novembre, n° 42 : Jérusalem change le vendredi avant
        // le dernier dimanche de mars ; une règle « dernier vendredi » serait
        // fausse en 2028. Les changements sont alors écrits un par un.
        // Le retour à l'heure d'hiver (dernier dimanche d'octobre) garde sa règle.
        let jerusalem = vtimezone("Asia/Jerusalem", 2026).expect("Asia/Jerusalem");
        let daylight: Vec<&str> = jerusalem.split("BEGIN:DAYLIGHT").skip(1).collect();
        assert!(daylight.len() >= 4, "{jerusalem}");
        assert!(daylight.iter().all(|block| !block.split("END:DAYLIGHT").next().unwrap_or("").contains("RRULE")), "{jerusalem}");
        assert!(jerusalem.contains("RRULE:FREQ=YEARLY;BYMONTH=10;BYDAY=-1SU"), "{jerusalem}");
        // Paris garde sa règle.
        assert!(vtimezone("Europe/Paris", 2026).expect("Europe/Paris").contains("RRULE:FREQ=YEARLY;BYMONTH=3;BYDAY=-1SU"));
        assert_eq!(civil_date(1_794_027_599, "America/New_York").as_deref(), Some("2026-11-06"));
        assert_eq!(civil_date(1_794_027_599, "").as_deref(), Some("2026-11-07"));
    }

    #[test]
    fn fuseau_sans_heure_d_ete() {
        let tokyo = vtimezone("Asia/Tokyo", 2026).expect("Asia/Tokyo");
        assert!(tokyo.contains("BEGIN:STANDARD\r\nDTSTART:19700101T000000\r\nTZOFFSETFROM:+0900\r\nTZOFFSETTO:+0900\r\n"), "{tokyo}");
        assert!(!tokyo.contains("DAYLIGHT"));
        assert!(vtimezone("Pas/Un_Fuseau", 2026).is_none());
        assert_eq!(utc_offset(-16_200), "-0430");
    }

    #[test]
    fn instants() {
        // 9 h à Paris en hiver = 8 h UTC ; en été = 7 h UTC.
        assert!(same_instant(&when("20261109T090000", "Europe/Paris"), &when("20261109T080000Z", "")));
        assert!(same_instant(&when("20260615T090000", "Europe/Paris"), &when("20260615T070000Z", "")));
        assert!(!same_instant(&when("20261109T090000", "Europe/Paris"), &when("20261109T090000Z", "")));
        assert_eq!(instant(&when("20261109", "")), instant(&when("20261109T000000Z", "")));
    }
}
