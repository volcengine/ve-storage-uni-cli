//! Safe share-page URL handling, independent of the share API endpoint.

use super::client::{Error, Result};
use super::share_types::validate_required;
use std::fmt;

/// Parsed page URL with its password removed from `safe_url` and Debug output.
#[derive(Clone)]
pub struct ParsedShareUrl {
    pub safe_url: String,
    pub origin: String,
    pub share_id: String,
    pub password: Option<String>,
}

impl fmt::Debug for ParsedShareUrl {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ParsedShareUrl")
            .field("origin", &self.origin)
            .field("password", &self.password.as_ref().map(|_| "***"))
            .finish_non_exhaustive()
    }
}

/// Build `/share/<id>` using one encoded path segment at a safe HTTP(S) origin.
/// Returns an input error for invalid origins, empty/control identifiers or dot segments.
pub fn build_share_url(share_base_origin: &str, share_id: &str) -> Result<String> {
    let mut url = safe_origin(share_base_origin)?;
    validate_share_id_component(share_id)?;
    url.path_segments_mut()
        .map_err(|_| invalid_share_url())?
        .push("share")
        .push(share_id);
    Ok(url.to_string())
}

/// Validate a response ShareID independently of page-origin configuration.
/// Returns an input error for empty identifiers, controls or dot segments.
pub(crate) fn validate_share_id_component(share_id: &str) -> Result<()> {
    validate_required(share_id, "ShareID")?;
    if matches!(share_id, "." | "..") {
        return Err(invalid_share_url());
    }
    Ok(())
}

/// Parse `/share/<id>` with one optional trailing slash, decode once and extract one `pwd`.
/// Returns the canonical URL without a trailing slash or password query.
/// Returns a sanitized error for malformed escapes, duplicate passwords or unsafe URLs.
pub fn parse_share_url(input: &str) -> Result<ParsedShareUrl> {
    let url = reqwest::Url::parse(input).map_err(|_| invalid_share_url())?;
    validate_url_safety(&url, input)?;
    let raw_path = raw_url_path(input).ok_or_else(invalid_share_url)?;
    let encoded_id = raw_path
        .strip_prefix("/share/")
        .ok_or_else(invalid_share_url)?;
    // [Review Fix #ShareTrailingSlash1] A copied page URL may end in one slash; normalize it without accepting subpaths or changing the share identity.
    let encoded_id = encoded_id.strip_suffix('/').unwrap_or(encoded_id);
    if encoded_id.is_empty() || encoded_id.contains('/') {
        return Err(invalid_share_url());
    }
    let share_id = decode_component(encoded_id, false)?;
    let origin = url.origin().ascii_serialization();
    let safe_url = build_share_url(&origin, &share_id)?;
    let password = parse_password(url.query())?;
    Ok(ParsedShareUrl {
        safe_url,
        origin,
        share_id,
        password,
    })
}

/// Derive the share-page origin by mapping standard IDS API hosts and adding a validated Instance label.
/// HTTP(S) scheme, port and already-prefixed Instance labels are retained.
/// Custom hosts retain their domain; returns an input error for unsafe origins or Instance labels.
pub fn derive_share_origin(endpoint: &str, instance_id: &str) -> Result<String> {
    // [Review Fix #ShareProtocol1] Protocol selection belongs to deployment configuration; page mapping only changes the host.
    let mut url = safe_origin(endpoint)?;
    // [Review Fix #LiveDomain1] A delivery URL must reach the page service, not the API service.
    map_ids_host(&mut url, true)?;
    if instance_id.len() > 63
        || instance_id.is_empty()
        || instance_id.starts_with('-')
        || instance_id.ends_with('-')
        || !instance_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err(Error::Client(
            "InstanceID must be a valid DNS label for share pages".into(),
        ));
    }
    let host = url.host_str().ok_or_else(invalid_share_url)?;
    if !is_loopback(&url)
        && !host
            .split('.')
            .next()
            .is_some_and(|label| label.eq_ignore_ascii_case(instance_id))
    {
        let host = format!("{instance_id}.{host}");
        url.set_host(Some(&host)).map_err(|_| invalid_share_url())?;
    }
    Ok(url.origin().ascii_serialization())
}

/// Derive the anonymous API origin from a safe share-page origin.
/// Maps public/BOE IDS page hosts, preserving Instance, scheme and port; custom hosts stay unchanged.
/// Returns an input error for unsafe origins. Explicit receiver endpoint overrides bypass this derivation.
pub fn derive_share_api_origin(page_origin: &str) -> Result<String> {
    let mut url = safe_origin(page_origin)?;
    // [Review Fix #LiveDomain1] Receiver transport has its own API origin and never loads account configuration.
    map_ids_host(&mut url, false)?;
    Ok(url.origin().ascii_serialization())
}

/// Extract an instance label from an exact standard IDS page/API host.
/// Custom domains return None; unsafe origins return a sanitized input error.
pub(crate) fn standard_share_instance(origin: &str) -> Result<Option<String>> {
    let url = safe_origin(origin)?;
    let host = url.host_str().ok_or_else(invalid_share_url)?;
    for suffix in [
        "idsapp.volces.com",
        "idsapp-boe.volces.com",
        "idsapi.volces.com",
        "idsapi-boe.volces.com",
    ] {
        if let Some(instance) = host.strip_suffix(&format!(".{suffix}")) {
            if !instance.is_empty() && !instance.contains('.') {
                return Ok(Some(instance.into()));
            }
        }
    }
    Ok(None)
}

fn map_ids_host(url: &mut reqwest::Url, is_page: bool) -> Result<()> {
    let host = url.host_str().ok_or_else(invalid_share_url)?;
    if let Some(mapped) = mapped_ids_host(host, is_page) {
        url.set_host(Some(&mapped))
            .map_err(|_| invalid_share_url())?;
    }
    Ok(())
}

fn mapped_ids_host(host: &str, is_page: bool) -> Option<String> {
    for (api, page) in [
        ("idsapi.volces.com", "idsapp.volces.com"),
        ("idsapi-boe.volces.com", "idsapp-boe.volces.com"),
    ] {
        let (source, target) = if is_page { (api, page) } else { (page, api) };
        let mapped = if host == source {
            Some(target.to_owned())
        } else {
            host.strip_suffix(&format!(".{source}"))
                .filter(|prefix| !prefix.is_empty() && !prefix.contains('.'))
                .map(|prefix| format!("{prefix}.{target}"))
        };
        if mapped.is_some() {
            return mapped;
        }
    }
    None
}

pub(crate) fn safe_origin(input: &str) -> Result<reqwest::Url> {
    let url = reqwest::Url::parse(input).map_err(|_| invalid_share_url())?;
    validate_url_safety(&url, input)?;
    // [Review Fix #1] Validate the raw path before URL normalization can erase dot segments.
    if !matches!(url.path(), "" | "/")
        || url.query().is_some()
        || raw_url_path(input).is_some_and(|path| path != "/")
    {
        return Err(invalid_share_url());
    }
    Ok(url)
}

fn validate_url_safety(url: &reqwest::Url, input: &str) -> Result<()> {
    // [Review Fix #ShareProtocol4] Require a matching raw scheme before the authority delimiter; a delimiter inside a path must not bypass validation.
    let authority = input
        .split_once("://")
        .filter(|(scheme, _)| scheme.eq_ignore_ascii_case(url.scheme()))
        .map(|(_, rest)| rest.split(['/', '?', '#']).next().unwrap_or(""))
        .ok_or_else(invalid_share_url)?;
    // [Review Fix #1] Browsers normalize raw backslashes into route separators.
    if input.trim() != input
        || input.contains('\\')
        || input.chars().any(char::is_control)
        || !url.username().is_empty()
        || url.password().is_some()
        || authority.contains('@')
        || url.fragment().is_some()
        || url.host_str().is_none()
        || !matches!(url.scheme(), "https" | "http")
    {
        return Err(invalid_share_url());
    }
    Ok(())
}

fn is_loopback(url: &reqwest::Url) -> bool {
    match url.host() {
        Some(url::Host::Ipv4(address)) => address.is_loopback(),
        Some(url::Host::Ipv6(address)) => address.is_loopback(),
        Some(url::Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
        None => false,
    }
}

fn raw_url_path(input: &str) -> Option<&str> {
    let (_, rest) = input.split_once("://")?;
    // [Review Fix #7] Query/fragment values cannot supply a missing raw route.
    let authority_and_path = rest.split(['?', '#']).next()?;
    let position = authority_and_path.find('/')?;
    Some(&authority_and_path[position..])
}

fn parse_password(query: Option<&str>) -> Result<Option<String>> {
    let mut password = None;
    for pair in query
        .unwrap_or("")
        .split('&')
        .filter(|pair| !pair.is_empty())
    {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        let key = decode_component(key, true)?;
        let value = decode_component(value, true)?;
        if key == "pwd" {
            if password.is_some() {
                return Err(invalid_share_url());
            }
            password = Some(value);
        }
    }
    Ok(password)
}

/// Decode percent escapes once; query mode also converts `+` to a space.
/// Returns UTF-8 text or a sanitized error for malformed escapes/encoding.
pub(super) fn decode_component(input: &str, is_query: bool) -> Result<String> {
    let mut decoded = Vec::with_capacity(input.len());
    let mut bytes = input.bytes();
    while let Some(byte) = bytes.next() {
        let value = match byte {
            b'%' => {
                let high = bytes
                    .next()
                    .and_then(hex_digit)
                    .ok_or_else(invalid_share_url)?;
                let low = bytes
                    .next()
                    .and_then(hex_digit)
                    .ok_or_else(invalid_share_url)?;
                high * 16 + low
            }
            b'+' if is_query => b' ',
            byte => byte,
        };
        decoded.push(value);
    }
    String::from_utf8(decoded).map_err(|_| invalid_share_url())
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn invalid_share_url() -> Error {
    Error::Client("share URL must use a safe HTTP(S) origin and /share/<id> with at most one trailing slash; select folders with --path".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_receiver_binding_recognizes_only_exact_instance_hosts() {
        for host in ["inst.idsapp.volces.com", "inst.idsapi-boe.volces.com"] {
            assert_eq!(
                standard_share_instance(&format!("http://{host}:8080")).unwrap(),
                Some("inst".into())
            );
        }
        for host in [
            "idsapp.volces.com",
            "nested.inst.idsapp.volces.com",
            "inst.idsapp.volces.com.evil.test",
            "inst.example.test",
            "127.0.0.1",
        ] {
            assert_eq!(
                standard_share_instance(&format!("https://{host}")).unwrap(),
                None
            );
        }
    }

    #[test]
    fn standard_http_api_configuration_preserves_page_protocol_and_port() {
        for (api, expected) in [
            (
                "http://idsapi-boe.volces.com",
                "http://vtagt3g9n.idsapp-boe.volces.com",
            ),
            (
                "http://vtagt3g9n.idsapi-boe.volces.com",
                "http://vtagt3g9n.idsapp-boe.volces.com",
            ),
            (
                "HTTP://IDSAPI.VOLCES.COM/",
                "http://vtagt3g9n.idsapp.volces.com",
            ),
            (
                "http://idsapi-boe.volces.com:80",
                "http://vtagt3g9n.idsapp-boe.volces.com",
            ),
            (
                "http://idsapi-boe.volces.com:8443",
                "http://vtagt3g9n.idsapp-boe.volces.com:8443",
            ),
        ] {
            assert_eq!(derive_share_origin(api, "vtagt3g9n").unwrap(), expected);
        }
    }

    #[test]
    fn http_origins_preserve_raw_validation_without_a_host_allowlist() {
        for api in [
            " http://idsapi-boe.volces.com",
            "http://idsapi-boe.volces.com ",
            "http://idsapi-boe.volces.com/../",
            "http://idsapi-boe.volces.com/%2e",
            "http://idsapi-boe.volces.com?pwd=secret",
            "http://user:secret@idsapi-boe.volces.com",
            r"http://idsapi-boe.volces.com\",
            "http://idsapi-boe.volces.com#secret",
        ] {
            let error = derive_share_origin(api, "inst").unwrap_err();
            assert!(!error.to_string().contains("secret"));
        }
        assert_eq!(
            derive_share_origin("http://127.0.0.1:8080", "inst").unwrap(),
            "http://127.0.0.1:8080"
        );
        for host in [
            "idsapi-boe.volces.com.attacker.test",
            "fake-idsapi-boe.volces.com",
            "nested.inst.idsapi-boe.volces.com",
            "custom.example.test",
        ] {
            let origin = format!("http://{host}:8080");
            assert_eq!(derive_share_api_origin(&origin).unwrap(), origin);
            assert_eq!(
                derive_share_origin(&origin, "inst").unwrap(),
                format!("http://inst.{host}:8080")
            );
        }
    }

    #[test]
    fn public_http_page_url_round_trips_and_redacts_password() {
        let page = build_share_url("http://inst.idsapp-boe.volces.com:8080", "s/id").unwrap();
        assert_eq!(page, "http://inst.idsapp-boe.volces.com:8080/share/s%2Fid");
        let parsed = parse_share_url(&format!("{page}?pwd=secret")).unwrap();
        assert_eq!(parsed.share_id, "s/id");
        assert_eq!(parsed.password.as_deref(), Some("secret"));
        assert_eq!(parsed.safe_url, page);
        assert!(!format!("{parsed:?}").contains("secret"));
        assert_eq!(
            derive_share_api_origin(&parsed.origin).unwrap(),
            "http://inst.idsapi-boe.volces.com:8080"
        );
    }

    #[test]
    fn receiver_urls_accept_one_trailing_slash_with_the_same_identity() {
        for origin in [
            "http://inst.idsapp-boe.volces.com",
            "https://pages.example.test:8443",
        ] {
            for encoded_id in ["2T4U8z0lY1n", "s%2Fid", "s%252Fid"] {
                let canonical = format!("{origin}/share/{encoded_id}");
                let expected = parse_share_url(&format!("{canonical}?pwd=p%2525%2B")).unwrap();
                let received = parse_share_url(&format!("{canonical}/?pwd=p%2525%2B")).unwrap();
                assert_eq!(received.share_id, expected.share_id);
                assert_eq!(received.origin, expected.origin);
                assert_eq!(received.safe_url, expected.safe_url);
                assert_eq!(received.password, expected.password);
                assert!(!format!("{received:?}").contains("p%25+"));
            }
        }
    }

    #[test]
    fn receiver_urls_reject_extra_routes_and_empty_identifiers() {
        for path in [
            "/share/",
            "/share//",
            "/share/s//",
            "/share/s/child/",
            "/share/../",
            "/share/%2e%2e/",
        ] {
            let error = parse_share_url(&format!(
                "http://pages.example.test{path}?pwd=fixture-secret"
            ))
            .unwrap_err();
            assert!(!error.to_string().contains("fixture-secret"), "{path}");
        }
    }

    #[test]
    fn production_api_origin_builds_delivery_urls_with_instance_once() {
        for scheme in ["http", "https"] {
            for api in [
                format!("{scheme}://idsapi.volces.com:8443"),
                format!("{scheme}://vtagt3g9n.idsapi.volces.com:8443"),
            ] {
                let origin = derive_share_origin(&api, "vtagt3g9n").unwrap();
                assert_eq!(
                    build_share_url(&origin, "2T4U8z0lY1n").unwrap(),
                    format!("{scheme}://vtagt3g9n.idsapp.volces.com:8443/share/2T4U8z0lY1n")
                );
            }
        }
    }

    #[test]
    fn standard_api_origins_produce_page_origins() {
        for (api, page) in [
            (
                "https://idsapi.volces.com",
                "https://inst-1.idsapp.volces.com",
            ),
            (
                "https://inst-1.idsapi-boe.volces.com:8443",
                "https://inst-1.idsapp-boe.volces.com:8443",
            ),
        ] {
            assert_eq!(derive_share_origin(api, "inst-1").unwrap(), page);
        }
    }

    #[test]
    fn custom_and_lookalike_hosts_are_not_rewritten() {
        for host in [
            "pages.example.test",
            "idsapp.volces.com.attacker.test",
            "fake-idsapp.volces.com",
            "nested.inst.idsapp-boe.volces.com",
            "idsapp-stable.volces.com",
        ] {
            let origin = format!("https://{host}:8443");
            assert_eq!(derive_share_api_origin(&origin).unwrap(), origin);
        }
        assert_eq!(
            build_share_url("https://pages.example.test", "s").unwrap(),
            "https://pages.example.test/share/s"
        );
        assert_eq!(
            derive_share_api_origin("https://inst.idsapi.volces.com").unwrap(),
            "https://inst.idsapi.volces.com"
        );
        assert_eq!(
            derive_share_origin("https://inst.idsapp-boe.volces.com", "inst").unwrap(),
            "https://inst.idsapp-boe.volces.com"
        );
    }

    #[test]
    fn share_url_round_trips_one_encoded_segment() {
        let built = build_share_url("https://instance.example.test", "id/%中文 ?").unwrap();
        let parsed = parse_share_url(&built).unwrap();
        assert_eq!(parsed.share_id, "id/%中文 ?");
        assert_eq!(parsed.safe_url, built);
    }

    #[test]
    fn share_url_decodes_password_once_and_removes_it() {
        let parsed = parse_share_url("https://example.test/share/a%252Fb?pwd=p%2525%2B").unwrap();
        assert_eq!(parsed.share_id, "a%2Fb");
        assert_eq!(parsed.password.as_deref(), Some("p%25+"));
        assert_eq!(parsed.safe_url, "https://example.test/share/a%252Fb");
        assert!(!format!("{parsed:?}").contains("p%25+"));
    }

    #[test]
    fn unsafe_or_ambiguous_share_urls_are_rejected_without_echo() {
        for input in [
            "https://u:secret@h.test/share/a",
            "https://h.test/share/a#secret",
            "https://h.test/share/a/b",
            "https://h.test/share/",
            "https://h.test/share/..",
            "https://h.test/share/a?pwd=secret&pwd=x",
            "https://h.test/share/%ZZ",
            "https://h.test/share/a?pwd=%X",
            "ftp://example.test/share/a",
        ] {
            let error = parse_share_url(input).unwrap_err();
            assert!(!error.to_string().contains("secret"), "{input}");
        }
    }

    #[test]
    fn safe_origins_require_no_userinfo_query_or_path() {
        for origin in [
            "https://u@h.test",
            "https://h.test/base",
            "https://h.test/?pwd=s",
            "https://h.test/#s",
            "file:///x",
        ] {
            assert!(build_share_url(origin, "id").is_err());
        }
        assert!(build_share_url("http://127.0.0.1:8080", "id").is_ok());
        assert!(build_share_url("https://h.test", "").is_err());
    }

    #[test]
    fn raw_path_normalization_cannot_bypass_origin_or_route_validation() {
        for origin in [
            "http:pages.example.test/base/..",
            "http:/pages.example.test/base/..",
            "https:pages.example.test/base/..",
            "https:/pages.example.test/base/..",
            "http:pages.example.test/base/../https://other.test",
            "https:pages.example.test/base/../http://other.test",
            "http:pages.example.test/path/http://",
            "https://h.test/.",
            "https://h.test/base/..",
            "https://h.test/%2e",
        ] {
            assert!(build_share_url(origin, "id").is_err(), "{origin}");
            assert!(derive_share_api_origin(origin).is_err(), "{origin}");
        }
        for page in [
            "http:pages.example.test/base/../https://other.test/share/s",
            "https:pages.example.test/base/../http://other.test/share/s",
            r"https://h.test/share/a\b",
            r"https://h.test/share/a\",
            "https://h.test/share/%00",
        ] {
            assert!(parse_share_url(page).is_err(), "{page}");
        }
    }

    #[test]
    fn query_values_cannot_supply_a_missing_share_route() {
        for input in [
            "https://h.test?next=/share/s",
            "https://h.test/?next=/share/s",
            "https://h.test?next=/share/s&pwd=secret",
        ] {
            let error =
                parse_share_url(input).expect_err("query path must not become the share route");
            assert!(!error.to_string().contains("secret"));
        }
    }

    #[test]
    fn derived_origin_changes_only_validated_host_label() {
        assert_eq!(
            derive_share_origin("https://api.example.test", "ins-1").unwrap(),
            "https://ins-1.api.example.test"
        );
        assert_eq!(
            derive_share_origin("https://ins-1.api.example.test", "ins-1").unwrap(),
            "https://ins-1.api.example.test"
        );
        assert_eq!(
            derive_share_origin("http://127.0.0.1:8080", "ins-1").unwrap(),
            "http://127.0.0.1:8080"
        );
        assert!(derive_share_origin("https://api.example.test", "x/y").is_err());
    }
}
