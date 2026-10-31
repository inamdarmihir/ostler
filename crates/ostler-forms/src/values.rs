use serde_json::Value;

use crate::resolve::normalize;
use crate::FieldOption;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValueError {
    Ambiguous(String),
    NoMatch(String),
    Invalid(String),
}

impl std::fmt::Display for ValueError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ValueError::Ambiguous(s) | ValueError::NoMatch(s) | ValueError::Invalid(s) => {
                f.write_str(s)
            }
        }
    }
}
impl std::error::Error for ValueError {}

fn as_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// Interpret common truthy/falsy spellings.
pub fn coerce_bool(v: &Value) -> Option<bool> {
    match v {
        Value::Bool(b) => Some(*b),
        Value::Number(n) => n.as_f64().map(|x| x != 0.0),
        Value::String(s) => match s.trim().to_lowercase().as_str() {
            "true" | "yes" | "y" | "on" | "1" | "checked" => Some(true),
            "false" | "no" | "n" | "off" | "0" | "unchecked" | "" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

/// Pick the option that `wanted` refers to: exact value, exact label, normalised
/// label, then a unique substring match. Returns the option index.
pub fn match_option(options: &[FieldOption], wanted: &Value) -> Result<usize, ValueError> {
    let w = as_text(wanted);
    let usable: Vec<(usize, &FieldOption)> = options
        .iter()
        .enumerate()
        .filter(|(_, o)| !o.disabled)
        .collect();
    if let Some((i, _)) = usable.iter().find(|(_, o)| o.value == w) {
        return Ok(*i);
    }
    if let Some((i, _)) = usable.iter().find(|(_, o)| o.label == w) {
        return Ok(*i);
    }
    let nw = normalize(&w);
    if nw.is_empty() {
        return Err(ValueError::NoMatch("empty value".into()));
    }
    let eq: Vec<_> = usable
        .iter()
        .filter(|(_, o)| normalize(&o.label) == nw || normalize(&o.value) == nw)
        .collect();
    if eq.len() == 1 {
        return Ok(eq[0].0);
    }
    let sub: Vec<_> = usable
        .iter()
        .filter(|(_, o)| {
            let l = normalize(&o.label);
            !l.is_empty() && (l.contains(&nw) || nw.contains(&l))
        })
        .collect();
    match (eq.len(), sub.len()) {
        (0, 1) => Ok(sub[0].0),
        (0, 0) => Err(ValueError::NoMatch(format!(
            "no option matches {w:?}; available: {}",
            usable
                .iter()
                .map(|(_, o)| o.label.as_str())
                .take(12)
                .collect::<Vec<_>>()
                .join(", ")
        ))),
        _ => Err(ValueError::Ambiguous(format!(
            "{w:?} matches several options"
        ))),
    }
}

const MONTHS: [&str; 12] = [
    "january",
    "february",
    "march",
    "april",
    "may",
    "june",
    "july",
    "august",
    "september",
    "october",
    "november",
    "december",
];

fn month_from_name(s: &str) -> Option<u32> {
    let s = s.to_lowercase();
    MONTHS
        .iter()
        .position(|m| s.len() >= 3 && m.starts_with(s.trim_end_matches('.')))
        .map(|i| i as u32 + 1)
}

fn valid_ymd(y: i32, m: u32, d: u32) -> bool {
    if !(1..=12).contains(&m) || d == 0 {
        return false;
    }
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let dim = match m {
        2 => {
            if leap {
                29
            } else {
                28
            }
        }
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    d <= dim
}

/// Normalise a date to ISO `YYYY-MM-DD`. Refuses ambiguous day/month order.
pub fn normalize_date(input: &str) -> Result<String, ValueError> {
    let s = input.trim();
    let iso = |y: i32, m: u32, d: u32| -> Result<String, ValueError> {
        if valid_ymd(y, m, d) {
            Ok(format!("{y:04}-{m:02}-{d:02}"))
        } else {
            Err(ValueError::Invalid(format!(
                "{input:?} is not a real calendar date"
            )))
        }
    };
    let nums: Vec<&str> = s.split(['-', '/', '.']).collect();
    if nums.len() == 3
        && nums
            .iter()
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
    {
        let n: Vec<u32> = nums.iter().map(|p| p.parse().unwrap_or(0)).collect();
        if nums[0].len() == 4 {
            return iso(n[0] as i32, n[1], n[2]);
        }
        if nums[2].len() == 4 {
            let (a, b, y) = (n[0], n[1], n[2] as i32);
            return match (a > 12, b > 12) {
                (true, false) => iso(y, b, a),
                (false, true) => iso(y, a, b),
                (false, false) if a == b => iso(y, a, b),
                (false, false) => Err(ValueError::Ambiguous(format!(
                    "{input:?} could be day/month or month/day; use YYYY-MM-DD"
                ))),
                (true, true) => Err(ValueError::Invalid(format!(
                    "{input:?} is not a valid date"
                ))),
            };
        }
    }
    // "March 5, 2024", "5 March 2024", "Mar 5 2024"
    let cleaned = s.replace(',', " ");
    let parts: Vec<&str> = cleaned.split_whitespace().collect();
    if parts.len() == 3 {
        let strip = |p: &str| {
            p.trim_end_matches(|c: char| c.is_ascii_alphabetic())
                .to_string()
        };
        if let (Some(m), Ok(d), Ok(y)) = (
            month_from_name(parts[0]),
            strip(parts[1]).parse::<u32>(),
            parts[2].parse::<i32>(),
        ) {
            return iso(y, m, d);
        }
        if let (Ok(d), Some(m), Ok(y)) = (
            strip(parts[0]).parse::<u32>(),
            month_from_name(parts[1]),
            parts[2].parse::<i32>(),
        ) {
            return iso(y, m, d);
        }
    }
    Err(ValueError::Invalid(format!(
        "cannot read {input:?} as a date; use YYYY-MM-DD"
    )))
}

/// Normalise a time to 24h `HH:MM`.
pub fn normalize_time(input: &str) -> Result<String, ValueError> {
    let s = input.trim().to_lowercase();
    let (body, pm, am) = if let Some(b) = s.strip_suffix("pm") {
        (b.trim(), true, false)
    } else if let Some(b) = s.strip_suffix("am") {
        (b.trim(), false, true)
    } else {
        (s.as_str(), false, false)
    };
    let mut it = body.split(':');
    let h: u32 = it
        .next()
        .and_then(|x| x.parse().ok())
        .ok_or_else(|| ValueError::Invalid(format!("cannot read {input:?} as a time")))?;
    let m: u32 = it.next().map(|x| x.parse().unwrap_or(99)).unwrap_or(0);
    let mut h24 = h;
    if pm && h < 12 {
        h24 += 12;
    }
    if am && h == 12 {
        h24 = 0;
    }
    if h24 > 23 || m > 59 || ((am || pm) && !(1..=12).contains(&h)) {
        return Err(ValueError::Invalid(format!(
            "{input:?} is not a valid time"
        )));
    }
    Ok(format!("{h24:02}:{m:02}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn opts(labels: &[(&str, &str)]) -> Vec<FieldOption> {
        labels
            .iter()
            .map(|(v, l)| FieldOption {
                value: v.to_string(),
                label: l.to_string(),
                ..Default::default()
            })
            .collect()
    }

    #[test]
    fn option_matching() {
        let o = opts(&[
            ("us", "United States"),
            ("gb", "United Kingdom"),
            ("de", "Germany"),
        ]);
        assert_eq!(match_option(&o, &json!("gb")), Ok(1));
        assert_eq!(match_option(&o, &json!("Germany")), Ok(2));
        assert_eq!(match_option(&o, &json!("germany")), Ok(2));
        assert_eq!(match_option(&o, &json!("States")), Ok(0));
        assert!(matches!(
            match_option(&o, &json!("United")),
            Err(ValueError::Ambiguous(_))
        ));
        assert!(matches!(
            match_option(&o, &json!("Mars")),
            Err(ValueError::NoMatch(_))
        ));
    }

    #[test]
    fn disabled_options_are_not_selectable() {
        let mut o = opts(&[("a", "Alpha"), ("b", "Beta")]);
        o[0].disabled = true;
        assert!(match_option(&o, &json!("Alpha")).is_err());
    }

    #[test]
    fn dates() {
        assert_eq!(normalize_date("2024-03-05").unwrap(), "2024-03-05");
        assert_eq!(normalize_date("2024/3/5").unwrap(), "2024-03-05");
        assert_eq!(normalize_date("25/12/2024").unwrap(), "2024-12-25");
        assert_eq!(normalize_date("12/25/2024").unwrap(), "2024-12-25");
        assert_eq!(normalize_date("March 5, 2024").unwrap(), "2024-03-05");
        assert_eq!(normalize_date("5 Mar 2024").unwrap(), "2024-03-05");
        assert_eq!(normalize_date("1st May 2024").unwrap(), "2024-05-01");
    }

    #[test]
    fn ambiguous_and_invalid_dates() {
        assert!(matches!(
            normalize_date("03/04/2024"),
            Err(ValueError::Ambiguous(_))
        ));
        assert!(matches!(
            normalize_date("2023-02-29"),
            Err(ValueError::Invalid(_))
        ));
        assert!(normalize_date("2024-02-29").is_ok());
        assert!(normalize_date("tomorrow").is_err());
    }

    #[test]
    fn times() {
        assert_eq!(normalize_time("14:30").unwrap(), "14:30");
        assert_eq!(normalize_time("3:30 PM").unwrap(), "15:30");
        assert_eq!(normalize_time("12:05 am").unwrap(), "00:05");
        assert_eq!(normalize_time("12 pm").unwrap(), "12:00");
        assert!(normalize_time("25:00").is_err());
        assert!(normalize_time("13:00 pm").is_err());
    }

    #[test]
    fn bools() {
        assert_eq!(coerce_bool(&json!("Yes")), Some(true));
        assert_eq!(coerce_bool(&json!(false)), Some(false));
        assert_eq!(coerce_bool(&json!("maybe")), None);
    }
}
