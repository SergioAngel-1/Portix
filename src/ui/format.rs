/// Sanitize text for safe display in the terminal.
///
/// This removes control characters that could be used for:
/// - Terminal title injection
/// - Cursor movement
/// - Buffer injection
///
/// See §13.2 in PRD for security details.
pub fn sanitize_for_terminal(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_control() {
                // Allow tab (expand to spaces) and newline
                if c == '\t' {
                    "  ".to_string()
                } else if c == '\n' {
                    "\n".to_string()
                } else {
                    // Replace other control characters
                    "·".to_string()
                }
            } else {
                c.to_string()
            }
        })
        .collect()
}

/// Format bytes in human-readable form.
pub fn format_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{} B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1} K", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1} M", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.2} G", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

/// Format duration in human-readable form.
pub fn format_duration(d: std::time::Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        let mins = secs / 60;
        let secs = secs % 60;
        format!("{}m {}s", mins, secs)
    } else {
        let hours = secs / 3600;
        let mins = (secs % 3600) / 60;
        format!("{}h {}m", hours, mins)
    }
}

/// Format CPU percentage.
pub fn format_cpu(percent: f64) -> String {
    if percent < 0.1 {
        "  —".to_string()
    } else {
        format!("{:>5.1}%", percent)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_ansi_escape() {
        let malicious = "\x1b]0;pwned\x07";
        let clean = sanitize_for_terminal(malicious);
        assert!(!clean.contains('\x1b'));
        assert!(!clean.contains('\x07'));
    }

    #[test]
    fn test_sanitize_control_chars() {
        let input = "hello\x01world\x02!";
        let clean = sanitize_for_terminal(input);
        // Control chars are replaced with "·"
        assert!(clean.contains("hello"));
        assert!(clean.contains("world"));
        assert!(clean.contains("·"));
        assert!(!clean.contains('\x01'));
        assert!(!clean.contains('\x02'));
    }

    #[test]
    fn test_format_bytes() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(1023), "1023 B");
        assert_eq!(format_bytes(1024), "1.0 K");
        assert_eq!(format_bytes(1024 * 1024), "1.0 M");
    }
}
