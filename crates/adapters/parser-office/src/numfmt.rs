//! Which cells hold dates (from the number formats in `styles.xml`) and how to write the
//! serial numbers Excel stores for them.

use quick_xml::events::Event;

use crate::xml::{attr, lower, reader};

/// Built-in number formats that are dates or times.
const BUILT_IN_DATE: [u32; 15] = [14, 15, 16, 17, 18, 19, 20, 21, 22, 27, 30, 36, 45, 46, 47];

#[derive(Debug, Default)]
pub(crate) struct Styles {
    /// The format id of each cell style (`cellXfs`, by position).
    cell_formats: Vec<u32>,
    /// Custom formats that are dates: ids.
    custom_dates: Vec<u32>,
    pub(crate) date1904: bool,
}

impl Styles {
    pub(crate) fn read(bytes: &[u8]) -> Self {
        let mut styles = Self::default();
        let mut reader = reader(bytes);
        let mut in_cell_xfs = false;
        while let Ok(event) = reader.read_event() {
            match event {
                Event::Start(e) => match lower(e.local_name().as_ref()).as_str() {
                    "numfmt" => {
                        if let (Some(id), Some(code)) = (
                            attr(&e, "numfmtid").and_then(|v| v.parse().ok()),
                            attr(&e, "formatcode"),
                        ) && is_date_format(&code)
                        {
                            styles.custom_dates.push(id);
                        }
                    }
                    "cellxfs" => in_cell_xfs = true,
                    "xf" if in_cell_xfs => styles.cell_formats.push(
                        attr(&e, "numfmtid")
                            .and_then(|v| v.parse().ok())
                            .unwrap_or(0),
                    ),
                    _ => {}
                },
                Event::End(e) if lower(e.local_name().as_ref()) == "cellxfs" => in_cell_xfs = false,
                Event::Eof => break,
                _ => {}
            }
        }
        styles
    }

    /// Reads the workbook's date system.
    pub(crate) fn with_date1904(mut self, date1904: bool) -> Self {
        self.date1904 = date1904;
        self
    }

    pub(crate) fn is_date(&self, style: usize) -> bool {
        self.cell_formats
            .get(style)
            .is_some_and(|id| BUILT_IN_DATE.contains(id) || self.custom_dates.contains(id))
    }
}

/// Whether a custom format code shows a date or a time (`dd/mm/yyyy`, `h:mm`), ignoring
/// quoted text, bracketed sections (`[Red]`, `[$-416]`) and escaped characters.
fn is_date_format(code: &str) -> bool {
    let mut quoted = false;
    let mut bracket = false;
    let mut escaped = false;
    for c in code.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        match c {
            '\\' | '_' | '*' => escaped = true,
            '"' => quoted = !quoted,
            '[' if !quoted => bracket = true,
            ']' if !quoted => bracket = false,
            'y' | 'Y' | 'd' | 'D' | 'h' | 'H' | 's' | 'S' | 'm' | 'M' if !quoted && !bracket => {
                return true;
            }
            _ => {}
        }
    }
    false
}

/// `2024-03-01` for a whole serial, `2024-03-01 14:30` with a time of day; `None` when the
/// serial is out of range.
pub(crate) fn serial_to_text(serial: f64, date1904: bool) -> Option<String> {
    if !serial.is_finite() || !(0.0..2_958_466.0).contains(&serial) {
        return None;
    }
    let days = serial.floor() as i64;
    let seconds = ((serial - serial.floor()) * 86_400.0).round() as i64;
    // Days since 1970-01-01. Excel's 1900 system counts a non-existent 1900-02-29.
    let unix_days = if date1904 {
        days - 24_107
    } else if days > 60 {
        days - 25_569
    } else {
        days - 25_568
    };
    let (year, month, day) = civil_from_days(unix_days);
    if seconds == 0 {
        Some(format!("{year:04}-{month:02}-{day:02}"))
    } else {
        Some(format!(
            "{year:04}-{month:02}-{day:02} {:02}:{:02}",
            seconds / 3600 % 24,
            seconds / 60 % 60
        ))
    }
}

/// Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (year + i64::from(month <= 2), month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serials_become_dates() {
        assert_eq!(
            serial_to_text(45_352.0, false).as_deref(),
            Some("2024-03-01")
        );
        assert_eq!(serial_to_text(1.0, false).as_deref(), Some("1900-01-01"));
        assert_eq!(serial_to_text(61.0, false).as_deref(), Some("1900-03-01"));
        assert_eq!(
            serial_to_text(45_352.604_166_7, false).as_deref(),
            Some("2024-03-01 14:30")
        );
        assert_eq!(serial_to_text(-1.0, false), None);
        // The 1904 system counts from 1904-01-01 and has no 1900 leap-year quirk.
        assert_eq!(serial_to_text(0.0, true).as_deref(), Some("1904-01-01"));
        assert_eq!(serial_to_text(1461.0, true).as_deref(), Some("1908-01-01"));
        assert_eq!(
            serial_to_text(43_890.0, true).as_deref(),
            Some("2024-03-01")
        );
    }

    #[test]
    fn date_formats_are_told_from_number_formats() {
        assert!(is_date_format("dd/mm/yyyy"));
        assert!(is_date_format("[$-416]d\\ \"de\"\\ mmmm"));
        assert!(is_date_format("h:mm AM/PM"));
        assert!(!is_date_format("0.00"));
        assert!(!is_date_format("#,##0.00\" dias\""));
        assert!(!is_date_format("[Red]0.00"));
    }
}
