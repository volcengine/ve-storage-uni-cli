//! Bounded structural prevalidation before typed JSON parsing.

use std::fmt;

/// Fixed structural rejection without retaining any input or parser context.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct JsonDepthError;

impl fmt::Display for JsonDepthError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("invalid JSON structure")
    }
}
impl std::error::Error for JsonDepthError {}

/// Check that `input` has balanced JSON containers and at most 64 nesting levels.
/// Brackets inside quoted strings are ignored, accounting for escaped quotes.
/// Returns fixed `JsonDepthError` for excess/mismatched/unclosed containers or
/// unclosed strings. Performs one byte pass with fixed storage and retains no
/// input. This is not full JSON syntax validation: typed Serde parsing must follow.
pub fn check_json_depth(input: &[u8]) -> Result<(), JsonDepthError> {
    // [Review Fix #67] Unknown Serde fields can bypass recursive type parsing; bound all input structure first.
    let mut structure = JsonStructure::new();
    for &byte in input {
        structure.accept(byte)?;
    }
    if structure.is_in_string || structure.depth != 0 {
        return Err(JsonDepthError);
    }
    Ok(())
}

const MAX_JSON_DEPTH: usize = 64;

struct JsonStructure {
    opening_brackets: [u8; MAX_JSON_DEPTH],
    depth: usize,
    is_in_string: bool,
    is_escaped: bool,
}

impl JsonStructure {
    fn new() -> Self {
        Self {
            opening_brackets: [0; MAX_JSON_DEPTH],
            depth: 0,
            is_in_string: false,
            is_escaped: false,
        }
    }

    fn accept(&mut self, byte: u8) -> Result<(), JsonDepthError> {
        if self.is_in_string {
            self.accept_string_byte(byte);
            return Ok(());
        }
        match byte {
            b'"' => self.is_in_string = true,
            b'{' | b'[' => self.open(byte)?,
            b'}' | b']' => self.close(byte)?,
            _ => {}
        }
        Ok(())
    }

    fn accept_string_byte(&mut self, byte: u8) {
        if self.is_escaped {
            self.is_escaped = false;
        } else if byte == b'\\' {
            self.is_escaped = true;
        } else if byte == b'"' {
            self.is_in_string = false;
        }
    }

    fn open(&mut self, byte: u8) -> Result<(), JsonDepthError> {
        if self.depth == MAX_JSON_DEPTH {
            return Err(JsonDepthError);
        }
        self.opening_brackets[self.depth] = byte;
        self.depth += 1;
        Ok(())
    }

    fn close(&mut self, byte: u8) -> Result<(), JsonDepthError> {
        let next_depth = self.depth.checked_sub(1).ok_or(JsonDepthError)?;
        let expected_opening = if byte == b'}' { b'{' } else { b'[' };
        if self.opening_brackets[next_depth] != expected_opening {
            return Err(JsonDepthError);
        }
        self.depth = next_depth;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn container_depth_limit_is_exactly_sixty_four() {
        let accepted = format!("{}0{}", "[".repeat(64), "]".repeat(64));
        let rejected = format!("{}0{}", "[".repeat(65), "]".repeat(65));
        assert_eq!(check_json_depth(accepted.as_bytes()), Ok(()));
        assert_eq!(check_json_depth(rejected.as_bytes()), Err(JsonDepthError));
    }

    #[test]
    fn very_deep_unknown_subtrees_are_rejected_without_recursive_parsing() {
        let document = format!(
            r#"{{"unknown":{}0{}}}"#,
            "[".repeat(10_000),
            "]".repeat(10_000)
        );
        assert_eq!(check_json_depth(document.as_bytes()), Err(JsonDepthError));
    }

    #[test]
    fn quoted_brackets_and_escaped_quotes_do_not_affect_depth() {
        for document in [
            br#"{"text":"[[[}}]"}"#.as_slice(),
            br#"{"text":"\\\"[}\\","nested":[{}]}"#,
        ] {
            assert_eq!(check_json_depth(document), Ok(()));
            assert!(serde_json::from_slice::<serde_json::Value>(document).is_ok());
        }
    }

    #[test]
    fn mismatched_and_unclosed_structures_are_rejected() {
        for document in [
            "[}",
            "{]",
            "}",
            "]",
            "[",
            "{",
            "[[{}]",
            "\"unclosed",
            "{\"text\":\"escape\\",
        ] {
            assert_eq!(check_json_depth(document.as_bytes()), Err(JsonDepthError));
        }
    }

    #[test]
    fn balanced_json_and_scalar_inputs_need_full_syntax_validation_later() {
        for document in [
            "{}",
            "[]",
            "[{},[]]",
            "null",
            "123",
            "true",
            "\"text\"",
            "",
            "invalid token",
        ] {
            assert_eq!(check_json_depth(document.as_bytes()), Ok(()));
        }
    }

    #[test]
    fn structural_errors_are_fixed_and_have_no_source() {
        let error = check_json_depth(b"{\"secret-sentinel\": [}").unwrap_err();
        assert_eq!(format!("{error}"), "invalid JSON structure");
        assert_eq!(format!("{error:?}"), "JsonDepthError");
        assert!(std::error::Error::source(&error).is_none());
    }
}
