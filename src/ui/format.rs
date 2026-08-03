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

/// The one way system-derived text is allowed to reach the screen.
///
/// Sanitizes, then guarantees the result is a single line. Any `String` built
/// from `cmdline`, `comm`, or a project name must pass through here.
pub fn cell(text: &str) -> String {
    sanitize_for_terminal(text).replace('\n', " ")
}

/// Compact byte formatting for table cells: "312M", not "312.4 M".
pub fn format_bytes_compact(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    let b = bytes as f64;
    if bytes < 1024 {
        format!("{}B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.0}K", b / KIB)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.0}M", b / (KIB * KIB))
    } else {
        format!("{:.1}G", b / (KIB * KIB * KIB))
    }
}

/// Compact uptime for table cells: "2h14m".
pub fn format_uptime(d: std::time::Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else {
        format!("{}h{}m", secs / 3600, (secs % 3600) / 60)
    }
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

/// Truncate a string from the center, keeping the head and tail.
///
/// Operates on `char`s, never byte indices: project paths routinely contain
/// multi-byte characters (`~/Documentos/Proyectos`) and slicing them by byte
/// offset panics.
pub fn truncate_center(s: &str, max_len: usize) -> String {
    let char_count = s.chars().count();
    if char_count <= max_len {
        return s.to_string();
    }
    if max_len <= 3 {
        return s.chars().take(max_len).collect();
    }

    let keep = (max_len - 3) / 2;
    let head: String = s.chars().take(keep).collect();
    let tail: String = s.chars().skip(char_count - keep).collect();
    format!("{}...{}", head, tail)
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

    /// argv[0] = "\x1b]0;pwned\x07" must not reach the terminal through a cell.
    #[test]
    fn test_cell_strips_escapes_and_newlines() {
        let clean = cell("web\x1b]0;pwned\x07\nsecond line");
        assert!(!clean.contains('\x1b'));
        assert!(!clean.contains('\n'));
    }

    #[test]
    fn test_truncate_center_does_not_split_multibyte_chars() {
        // The exact input that panicked: a real project path with 'ó'.
        let path = "leadgen-frontend/Documentos/Proyectos";
        assert_eq!(truncate_center(path, 19), "leadgen-...royectos");

        // Truncation boundary landing inside a multi-byte char must not panic.
        for len in 0..30 {
            let _ = truncate_center("Documentós-Proyectós-ñ", len);
        }
    }

    #[test]
    fn test_truncate_center_passes_through_short_strings() {
        assert_eq!(truncate_center("vite", 20), "vite");
        assert_eq!(truncate_center("ñ", 1), "ñ");
    }

    #[test]
    fn test_format_bytes() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(1023), "1023 B");
        assert_eq!(format_bytes(1024), "1.0 K");
        assert_eq!(format_bytes(1024 * 1024), "1.0 M");
    }
}
