use crate::error::AppError;

/// Parse a duration string into decimal hours.
/// Supports: "1.5" (decimal hours), "1:30" (HH:MM), "0:15" (MM only treated as H:MM)
pub fn parse_duration(input: &str) -> Result<f64, AppError> {
    let input = input.trim();

    if let Some((h, m)) = input.split_once(':') {
        let hours: f64 = h
            .parse()
            .map_err(|_| AppError::InvalidInput(format!("Invalid duration hours: '{h}'")))?;
        let minutes: f64 = m
            .parse()
            .map_err(|_| AppError::InvalidInput(format!("Invalid duration minutes: '{m}'")))?;
        if !(0.0..60.0).contains(&minutes) {
            return Err(AppError::InvalidInput(format!(
                "Minutes must be 0-59, got {minutes}"
            )));
        }
        let total = hours + minutes / 60.0;
        if total <= 0.0 {
            return Err(AppError::InvalidInput("Duration must be positive".into()));
        }
        Ok(total)
    } else {
        let hours: f64 = input
            .parse()
            .map_err(|_| AppError::InvalidInput(format!("Invalid duration: '{input}'")))?;
        if hours <= 0.0 {
            return Err(AppError::InvalidInput("Duration must be positive".into()));
        }
        Ok(hours)
    }
}

/// Format decimal hours as "Xh Ym"
pub fn format_duration(hours: f64) -> String {
    let total_minutes = (hours * 60.0).round() as i64;
    let h = total_minutes / 60;
    let m = total_minutes % 60;
    if h > 0 && m > 0 {
        format!("{h}h {m}m")
    } else if h > 0 {
        format!("{h}h")
    } else {
        format!("{m}m")
    }
}

/// Case-insensitive name/code/ID matching for projects and tasks.
/// Returns the ID of the matched item, or an error listing available names.
pub fn resolve_name_to_id<'a>(
    query: &str,
    items: &'a [(String, String, Option<String>)], // (id, name, optional code)
    entity_type: &str,
) -> Result<&'a str, AppError> {
    let q = query.to_lowercase();

    // Exact ID match
    if let Some((id, _, _)) = items.iter().find(|(id, _, _)| id == query) {
        return Ok(id);
    }

    // Case-insensitive name match
    if let Some((id, _, _)) = items.iter().find(|(_, name, _)| name.to_lowercase() == q) {
        return Ok(id);
    }

    // Case-insensitive code match
    if let Some((id, _, _)) = items.iter().find(|(_, _, code)| {
        code.as_ref()
            .map(|c| c.to_lowercase() == q)
            .unwrap_or(false)
    }) {
        return Ok(id);
    }

    let available: Vec<&str> = items.iter().map(|(_, name, _)| name.as_str()).collect();
    Err(AppError::NotFound(format!(
        "{entity_type} '{query}' not found. Available: {}",
        available.join(", ")
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_decimal_hours() {
        assert!((parse_duration("1.5").unwrap() - 1.5).abs() < f64::EPSILON);
        assert!((parse_duration("0.25").unwrap() - 0.25).abs() < f64::EPSILON);
    }

    #[test]
    fn parse_hhmm() {
        assert!((parse_duration("1:30").unwrap() - 1.5).abs() < f64::EPSILON);
        assert!((parse_duration("0:15").unwrap() - 0.25).abs() < f64::EPSILON);
        assert!((parse_duration("2:00").unwrap() - 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn parse_invalid_durations() {
        assert!(parse_duration("abc").is_err());
        assert!(parse_duration("0").is_err());
        assert!(parse_duration("-1").is_err());
        assert!(parse_duration("1:70").is_err());
    }

    #[test]
    fn format_duration_display() {
        assert_eq!(format_duration(1.5), "1h 30m");
        assert_eq!(format_duration(2.0), "2h");
        assert_eq!(format_duration(0.25), "15m");
    }

    #[test]
    fn resolve_by_name_case_insensitive() {
        let items = vec![
            ("id1".into(), "Development".into(), Some("DEV".into())),
            ("id2".into(), "Design".into(), None),
        ];
        assert_eq!(
            resolve_name_to_id("development", &items, "task").unwrap(),
            "id1"
        );
        assert_eq!(resolve_name_to_id("DEV", &items, "task").unwrap(), "id1");
        assert_eq!(resolve_name_to_id("id2", &items, "task").unwrap(), "id2");
        assert!(resolve_name_to_id("nope", &items, "task").is_err());
    }
}
