/// Return operational Markdown for an exported command skill.
///
/// # Parameters
/// - `surface`: Public command domain (`tos`, `ve-tos`, or `ve-adrive`).
/// - `suffix`: Command path without the domain prefix, such as `cp`.
/// - `is_chinese`: Select Chinese when true, otherwise English.
///
/// # Returns
/// Static Markdown without executable command examples; unknown commands or
/// surfaces return an empty string so unrelated commands do not inherit advice.
///
/// # Errors
/// This function performs no I/O, returns no errors, and does not panic.
pub fn command_guide(surface: &str, suffix: &str, is_chinese: bool) -> &'static str {
    if !matches!(surface, "tos" | "ve-tos" | "ve-adrive") {
        return "";
    }
    match (surface, suffix, is_chinese) {
        ("tos", "cp", false) => include_str!("skill_guides/tos-cp.en.md"),
        ("tos", "cp", true) => include_str!("skill_guides/tos-cp.zh.md"),
        ("ve-tos", "cp", false) => include_str!("skill_guides/ve-tos-cp.en.md"),
        ("ve-tos", "cp", true) => include_str!("skill_guides/ve-tos-cp.zh.md"),
        ("ve-adrive", "cp", false) => include_str!("skill_guides/ve-adrive-cp.en.md"),
        ("ve-adrive", "cp", true) => include_str!("skill_guides/ve-adrive-cp.zh.md"),
        (_, "mv", false) => include_str!("skill_guides/mv.en.md"),
        (_, "mv", true) => include_str!("skill_guides/mv.zh.md"),
        (_, "sync", false) => include_str!("skill_guides/sync.en.md"),
        (_, "sync", true) => include_str!("skill_guides/sync.zh.md"),
        (_, "rm", false) => include_str!("skill_guides/rm.en.md"),
        (_, "rm", true) => include_str!("skill_guides/rm.zh.md"),
        (_, "ls", false) => include_str!("skill_guides/ls.en.md"),
        (_, "ls", true) => include_str!("skill_guides/ls.zh.md"),
        // [Review Fix #1] Describe HEAD/DELETE too: the handler accepts more than GET/PUT.
        ("tos", "presign", false) => include_str!("skill_guides/tos-presign.en.md"),
        ("tos", "presign", true) => include_str!("skill_guides/tos-presign.zh.md"),
        ("ve-tos", "presign", false) => include_str!("skill_guides/presign.en.md"),
        ("ve-tos", "presign", true) => include_str!("skill_guides/presign.zh.md"),
        _ => "",
    }
}

/// Return ByteCloud TOS authentication guidance for exported skills.
///
/// `surface` selects the CLI domain, and `is_chinese` selects the language.
/// Only `tos` receives this guide; other surfaces return an empty string.
/// This static function performs no I/O and cannot fail.
pub fn auth_guide(surface: &str, is_chinese: bool) -> &'static str {
    match (surface, is_chinese) {
        ("tos", false) => include_str!("skill_guides/tos-auth.en.md"),
        ("tos", true) => include_str!("skill_guides/tos-auth.zh.md"),
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::{auth_guide, command_guide};

    #[test]
    fn authentication_guide_is_byte_tos_only() {
        for is_chinese in [false, true] {
            let guide = auth_guide("tos", is_chinese);
            for expected in ["--auth-mode zti", "SEC_TOKEN_STRING", "SEC_TOKEN_PATH"] {
                assert!(guide.contains(expected));
            }
            assert!(auth_guide("ve-tos", is_chinese).is_empty());
            assert!(auth_guide("ve-adrive", is_chinese).is_empty());
        }
    }

    #[test]
    fn copy_guides_cover_decisions_beyond_the_parameter_schema() {
        for surface in ["tos", "ve-tos", "ve-adrive"] {
            for is_chinese in [false, true] {
                let guide = command_guide(surface, "cp", is_chinese);
                for required in [
                    "--recursive",
                    "--include-parent",
                    "--overwrite-strategy",
                    "--checkpoint",
                    "--report-path",
                    "--dry-run",
                    "--no-manifest",
                ] {
                    assert!(guide.contains(required), "{surface}: missing {required}");
                }
            }
        }
    }

    #[test]
    fn backend_copy_restrictions_remain_distinct() {
        assert!(command_guide("tos", "cp", false).contains("--storage-class"));
        assert!(command_guide("ve-tos", "cp", false).contains("same region"));
        assert!(command_guide("ve-adrive", "cp", false).contains("same instance"));
        assert!(command_guide("ve-adrive", "cp", false).contains("Local-to-local"));
    }

    #[test]
    fn signed_url_guide_explains_all_supported_methods() {
        for is_chinese in [false, true] {
            let guide = command_guide("ve-tos", "presign", is_chinese);
            for method in ["GET", "PUT", "HEAD", "DELETE"] {
                assert!(guide.contains(method), "missing method {method}");
            }
        }
    }

    #[test]
    fn unknown_commands_do_not_inherit_transfer_advice() {
        assert_eq!(command_guide("unknown", "cp", false), "");
        assert_eq!(command_guide("tos", "bucket get-acl", false), "");
        assert_eq!(command_guide("ve-adrive", "presign", false), "");
    }

    #[test]
    fn common_guides_exist_in_both_languages_without_pinning_an_entrypoint() {
        for suffix in ["cp", "mv", "sync", "rm", "ls", "presign"] {
            let english = command_guide("tos", suffix, false);
            let chinese = command_guide("tos", suffix, true);
            assert!(!english.is_empty(), "{suffix}");
            assert_ne!(english, chinese, "{suffix}");
            for guide in [english, chinese] {
                assert!(!guide.contains("tos-cli "));
                assert!(!guide.contains("ve-storage-uni-cli "));
            }
        }
    }
}
