//! Same-origin guard for state-changing requests (#17).
//!
//! spall has no authentication and binds to localhost, so any web page the
//! tester visits can send requests to it: the Host allowlist stops DNS
//! rebinding, not a cross-site POST, which still carries
//! `Host: 127.0.0.1:3000`. A forged replay would resend the tester's
//! captured, credentialed requests. Every method other than GET and HEAD
//! must therefore come from spall's own page:
//!
//! - `Content-Type: application/json`. A cross-site page cannot send it
//!   without a CORS preflight, and spall never approves one (it sends no
//!   CORS headers at all).
//! - `Sec-Fetch-Site`, when present, is `same-origin`.
//! - `Origin`, when present, is `http://<Host>` (Host was already checked
//!   against the allowlist).
//! - At least one of those two headers is present.

use axum::http::{HeaderMap, Method};

/// Why a request was refused. The text is safe to return to the client.
pub type Refusal = &'static str;

/// `Ok(())` if the request may proceed. GET and HEAD always may: they
/// must not change state, and the guard is for the routes that do.
pub fn check(method: &Method, headers: &HeaderMap) -> Result<(), Refusal> {
    check_as(method, headers, false)
}

/// [`check`] for `/mcp` (#58): a client that isn't a browser (no `Origin`,
/// no `Sec-Fetch-Site`) is allowed, because the route needs a bearer token
/// no web page can know; a browser's cross-site request is still refused,
/// and so is anything but JSON (a `DELETE` ending a session has no body).
pub fn check_mcp(method: &Method, headers: &HeaderMap) -> Result<(), Refusal> {
    check_as(method, headers, true)
}

fn check_as(method: &Method, headers: &HeaderMap, headless_ok: bool) -> Result<(), Refusal> {
    if method == Method::GET || method == Method::HEAD {
        return Ok(());
    }
    if headless_ok && method == Method::DELETE {
        let browser = headers.contains_key("origin") || headers.contains_key("sec-fetch-site");
        if !browser {
            return Ok(());
        }
    }
    let text = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());

    let content_type = text("content-type").unwrap_or("");
    let essence = content_type.split(';').next().unwrap_or("").trim();
    if !essence.eq_ignore_ascii_case("application/json") {
        return Err("state-changing requests must be application/json");
    }

    let fetch_site = text("sec-fetch-site");
    if let Some(site) = fetch_site {
        if site != "same-origin" {
            return Err("cross-site request refused");
        }
    }

    let origin = text("origin");
    if let Some(origin) = origin {
        let host = text("host").unwrap_or("");
        if host.is_empty() || !origin.eq_ignore_ascii_case(&format!("http://{host}")) {
            return Err("cross-origin request refused");
        }
    }

    if fetch_site.is_none() && origin.is_none() && !headless_ok {
        return Err("request has no Origin or Sec-Fetch-Site; refused");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{header, HeaderValue};

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(header::HOST, HeaderValue::from_static("127.0.0.1:3000"));
        for (k, v) in pairs {
            h.insert(
                axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                HeaderValue::from_str(v).unwrap(),
            );
        }
        h
    }

    const JSON: (&str, &str) = ("content-type", "application/json");

    #[test]
    fn mcp_allows_a_headless_client_and_still_refuses_a_browser_cross_site() {
        let post = Method::POST;
        assert!(
            check(&post, &headers(&[JSON])).is_err(),
            "the UI's routes want a browser's headers"
        );
        assert!(
            check_mcp(&post, &headers(&[JSON])).is_ok(),
            "an MCP client (the token is checked after)"
        );
        assert!(check_mcp(&post, &headers(&[])).is_err(), "JSON only");
        assert!(check_mcp(&post, &headers(&[JSON, ("origin", "http://evil.test")])).is_err());
        assert!(check_mcp(&post, &headers(&[JSON, ("sec-fetch-site", "cross-site")])).is_err());
        assert!(check_mcp(
            &post,
            &headers(&[JSON, ("origin", "http://127.0.0.1:3000")])
        )
        .is_ok());
        assert!(
            check_mcp(&Method::DELETE, &headers(&[])).is_ok(),
            "ending a session has no body"
        );
        assert!(check_mcp(&Method::DELETE, &headers(&[("origin", "http://evil.test")])).is_err());
        assert!(check(&Method::DELETE, &headers(&[])).is_err());
    }
    const SAME_SITE: (&str, &str) = ("sec-fetch-site", "same-origin");
    const OWN_ORIGIN: (&str, &str) = ("origin", "http://127.0.0.1:3000");

    #[test]
    fn reads_are_never_blocked() {
        for m in [Method::GET, Method::HEAD] {
            assert_eq!(check(&m, &headers(&[])), Ok(()));
        }
    }

    #[test]
    fn spalls_own_page_passes() {
        let post = Method::POST;
        assert_eq!(
            check(&post, &headers(&[JSON, SAME_SITE, OWN_ORIGIN])),
            Ok(())
        );
        assert_eq!(check(&post, &headers(&[JSON, SAME_SITE])), Ok(()));
        assert_eq!(check(&post, &headers(&[JSON, OWN_ORIGIN])), Ok(()));
        let charset = ("content-type", "Application/JSON; charset=utf-8");
        assert_eq!(check(&post, &headers(&[charset, SAME_SITE])), Ok(()));
    }

    #[test]
    fn every_unsafe_method_is_guarded() {
        for m in [
            Method::POST,
            Method::PUT,
            Method::PATCH,
            Method::DELETE,
            Method::OPTIONS,
        ] {
            assert!(
                check(&m, &headers(&[SAME_SITE, OWN_ORIGIN])).is_err(),
                "{m}"
            );
        }
    }

    #[test]
    fn simple_request_content_types_are_refused() {
        // The three a cross-site form or no-cors fetch can send without a preflight.
        for ct in [
            "text/plain",
            "application/x-www-form-urlencoded",
            "multipart/form-data; boundary=x",
            "application/json-seq",
            "",
        ] {
            let h = headers(&[("content-type", ct), SAME_SITE, OWN_ORIGIN]);
            assert!(check(&Method::POST, &h).is_err(), "{ct:?}");
        }
        assert!(check(&Method::POST, &headers(&[SAME_SITE, OWN_ORIGIN])).is_err());
    }

    #[test]
    fn foreign_or_missing_origin_is_refused() {
        for site in ["cross-site", "same-site", "none", ""] {
            let h = headers(&[JSON, ("sec-fetch-site", site), OWN_ORIGIN]);
            assert!(check(&Method::POST, &h).is_err(), "sec-fetch-site {site:?}");
        }
        for origin in [
            "http://evil.example",
            "null",
            "https://127.0.0.1:3000",
            "http://127.0.0.1:3001",
            "http://127.0.0.1:3000.evil.example",
            "http://localhost:3000",
        ] {
            let h = headers(&[JSON, SAME_SITE, ("origin", origin)]);
            assert!(check(&Method::POST, &h).is_err(), "origin {origin:?}");
        }
        assert!(
            check(&Method::POST, &headers(&[JSON])).is_err(),
            "neither header: refused"
        );
    }
}
