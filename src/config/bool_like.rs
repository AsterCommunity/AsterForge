pub fn parse_bool_like(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Some(true),
        "false" | "0" | "no" | "off" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::parse_bool_like;

    #[test]
    fn parses_supported_true_values() {
        for value in ["true", " TRUE ", "1", "yes", "on"] {
            assert_eq!(parse_bool_like(value), Some(true));
        }
    }

    #[test]
    fn parses_supported_false_values() {
        for value in ["false", " FALSE ", "0", "no", "off"] {
            assert_eq!(parse_bool_like(value), Some(false));
        }
    }

    #[test]
    fn rejects_unknown_values() {
        for value in ["", "  ", "maybe", "truthy"] {
            assert_eq!(parse_bool_like(value), None);
        }
    }
}
