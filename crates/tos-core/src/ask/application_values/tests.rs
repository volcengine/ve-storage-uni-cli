use super::*;

#[test]
fn complete_and_absent_bundles_are_distinct() {
    assert!(parse_bundle(None, None).unwrap().is_none());
    let bundle = parse_bundle(Some("synthetic_app"), Some("synthetic-secret"))
        .unwrap()
        .unwrap();
    assert_eq!(bundle.app_id, "synthetic_app");
    assert!(bundle.secret == "synthetic-secret");
}

#[test]
fn every_partial_bundle_is_rejected() {
    for mask in 1..3 {
        assert!(parse_bundle(
            (mask & 1 != 0).then_some("synthetic_app"),
            (mask & 2 != 0).then_some("synthetic-secret"),
        )
        .is_err());
    }
}

#[test]
fn application_identifiers_are_bounded_ascii_identifiers() {
    for app_id in ["", "bad app", "bad/app", "应用", &"a".repeat(257)] {
        assert!(parse_bundle(Some(app_id), Some("fixture")).is_err());
    }
    assert!(parse_bundle(Some(&"a".repeat(256)), Some("fixture")).is_ok());
}

#[test]
fn secrets_are_exact_bounded_ascii_graphic_values() {
    for secret in [
        "",
        " padded",
        "padded ",
        "a b",
        "a\nb",
        "a\tb",
        "☃",
        "\u{7f}",
        &"s".repeat(4097),
    ] {
        assert!(parse_bundle(Some("fixture"), Some(secret)).is_err());
    }
    assert!(parse_bundle(Some("fixture"), Some(&"s".repeat(4096))).is_ok());
}

#[test]
fn scopes_require_strict_tokens_business_access_and_no_duplicates() {
    for scopes in [
        "",
        " ",
        "offline_access",
        "scope  other",
        " scope",
        "scope ",
        "scope\tother",
        "scope scope",
        "scope offline_access scope",
        "bad\"scope",
        "bad\\scope",
        "☃",
        &"s".repeat(257),
    ] {
        assert!(validate_scopes(scopes).is_err());
    }
    assert!(validate_scopes("scope offline_access").is_ok());
}

#[test]
fn scope_count_and_total_size_are_bounded() {
    let sixty_four = (0..64)
        .map(|index| format!("scope{index}"))
        .collect::<Vec<_>>()
        .join(" ");
    assert!(validate_scopes(&sixty_four).is_ok());
    assert!(validate_scopes(&format!("{sixty_four} extra")).is_err());
    let largest = (0..16)
        .map(|index| format!("{index:03}{}", "s".repeat(253)))
        .collect::<Vec<_>>()
        .join(" ");
    assert!(largest.len() > 4096);
    assert!(validate_scopes(&largest).is_err());
}

#[test]
fn scope_size_boundary_accepts_exactly_the_limit() {
    let prefix = (0..15)
        .map(|index| format!("{index:03}{}", "s".repeat(253)))
        .collect::<Vec<_>>()
        .join(" ");
    let at_limit = format!("{prefix} {}", "s".repeat(241));
    assert_eq!(at_limit.len(), 4096);
    assert!(validate_scopes(&at_limit).is_ok());
    assert!(validate_scopes(&format!("{at_limit}s")).is_err());
}
