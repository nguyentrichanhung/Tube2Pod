use crate::error::{AppError, AppResult};
use crate::services::tokens::constant_time_eq;
use axum::extract::Request;
use axum::http::uri::Authority;
use axum::http::{header, HeaderMap, HeaderValue};
use axum::middleware::Next;
use axum::response::Response;
use rand::RngCore;
use url::Url;

pub const CSRF_COOKIE: &str = "t2p_csrf";

#[derive(Clone, Debug)]
pub struct CsrfToken(pub String);

pub async fn csrf_middleware(mut request: Request, next: Next) -> Response {
    let existing = cookie_value(request.headers(), CSRF_COOKIE);
    let token = existing.clone().unwrap_or_else(new_token);
    request.extensions_mut().insert(CsrfToken(token.clone()));
    let mut response = next.run(request).await;
    if existing.is_none() {
        let value = format!("{CSRF_COOKIE}={token}; Path=/; HttpOnly; Secure; SameSite=Strict");
        if let Ok(value) = HeaderValue::from_str(&value) {
            response.headers_mut().append(header::SET_COOKIE, value);
        }
    }
    response
}

fn new_token() -> String {
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn valid_token(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .filter_map(|part| part.trim().split_once('='))
        .find(|(key, value)| *key == name && valid_token(value))
        .map(|(_, value)| value.to_string())
}

pub fn verify(token: &CsrfToken, headers: &HeaderMap, form_token: &str) -> AppResult<()> {
    if headers
        .get("sec-fetch-site")
        .and_then(|value| value.to_str().ok())
        == Some("cross-site")
    {
        return Err(AppError::Forbidden);
    }
    let provided = headers
        .get("x-csrf-token")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .unwrap_or(form_token);
    if token.0.is_empty() || provided.is_empty() {
        return Err(AppError::Forbidden);
    }
    if !constant_time_eq(provided, &token.0) {
        return Err(AppError::Forbidden);
    }
    if let Some(origin) = headers.get(header::ORIGIN) {
        let matches = origin
            .to_str()
            .ok()
            .is_some_and(|origin| origin_matches_host(origin, headers));
        if !matches {
            return Err(AppError::Forbidden);
        }
    }
    Ok(())
}

fn origin_matches_host(origin: &str, headers: &HeaderMap) -> bool {
    let Some(host) = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    let Ok(url) = Url::parse(origin) else {
        return false;
    };
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || (url.path() != "/" && !url.path().is_empty())
    {
        return false;
    }
    let (Some(origin_host), Some(origin_port)) = (url.host_str(), url.port_or_known_default())
    else {
        return false;
    };
    let Ok(authority) = host.parse::<Authority>() else {
        return false;
    };
    if authority.host().is_empty() || !origin_host.eq_ignore_ascii_case(authority.host()) {
        return false;
    }
    let host_port = authority.port_u16().unwrap_or(443);
    origin_port == host_port
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    const VALID: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn token() -> CsrfToken {
        CsrfToken(VALID.into())
    }

    fn headers_with(origin: Option<&str>, host: Option<&str>) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("x-csrf-token", HeaderValue::from_static(VALID));
        if let Some(origin) = origin {
            headers.insert(header::ORIGIN, HeaderValue::from_str(origin).unwrap());
        }
        if let Some(host) = host {
            headers.insert(header::HOST, HeaderValue::from_str(host).unwrap());
        }
        headers
    }

    #[test]
    fn accepts_matching_header_and_rejects_missing_or_cross_site() {
        let token = token();
        let mut headers = axum::http::HeaderMap::new();
        headers.insert("x-csrf-token", HeaderValue::from_static(VALID));
        headers.insert("sec-fetch-site", HeaderValue::from_static("same-origin"));
        assert!(verify(&token, &headers, "").is_ok());

        let mut missing = axum::http::HeaderMap::new();
        missing.insert("sec-fetch-site", HeaderValue::from_static("same-origin"));
        assert!(verify(&token, &missing, "").is_err());

        let mut cross = axum::http::HeaderMap::new();
        cross.insert("x-csrf-token", HeaderValue::from_static(VALID));
        cross.insert("sec-fetch-site", HeaderValue::from_static("cross-site"));
        assert!(verify(&token, &cross, "").is_err());
    }

    #[test]
    fn accepts_valid_form_token_without_header() {
        assert!(verify(&token(), &HeaderMap::new(), VALID).is_ok());
    }

    #[test]
    fn rejects_empty_token_or_empty_provided_token() {
        assert!(verify(&CsrfToken(String::new()), &HeaderMap::new(), "").is_err());
        assert!(verify(&CsrfToken(String::new()), &headers_with(None, None), "").is_err());
        assert!(verify(&token(), &HeaderMap::new(), "").is_err());
    }

    #[test]
    fn rejects_mismatched_token() {
        let mut headers = HeaderMap::new();
        headers.insert("x-csrf-token", HeaderValue::from_static("deadbeef"));
        assert!(verify(&token(), &headers, "").is_err());
    }

    #[test]
    fn cookie_value_requires_64_hex_characters() {
        let mut headers = HeaderMap::new();
        headers.insert(header::COOKIE, HeaderValue::from_static("t2p_csrf="));
        assert!(cookie_value(&headers, CSRF_COOKIE).is_none());

        headers.insert(header::COOKIE, HeaderValue::from_static("t2p_csrf=abc"));
        assert!(cookie_value(&headers, CSRF_COOKIE).is_none());

        let non_hex = format!("t2p_csrf={}", "z".repeat(64));
        headers.insert(header::COOKIE, HeaderValue::from_str(&non_hex).unwrap());
        assert!(cookie_value(&headers, CSRF_COOKIE).is_none());

        let valid = format!("t2p_csrf={VALID}");
        headers.insert(header::COOKIE, HeaderValue::from_str(&valid).unwrap());
        assert_eq!(cookie_value(&headers, CSRF_COOKIE), Some(VALID.into()));
    }

    #[test]
    fn accepts_same_origin_with_exact_host_and_port() {
        let headers = headers_with(
            Some("https://pod.example.ts.net:8443"),
            Some("pod.example.ts.net:8443"),
        );
        assert!(verify(&token(), &headers, "").is_ok());
    }

    #[test]
    fn accepts_same_origin_on_default_https_port() {
        let headers = headers_with(
            Some("https://pod.example.ts.net"),
            Some("pod.example.ts.net"),
        );
        assert!(verify(&token(), &headers, "").is_ok());
    }

    #[test]
    fn rejects_origin_port_mismatch() {
        let headers = headers_with(
            Some("https://pod.example.ts.net:8443"),
            Some("pod.example.ts.net:9443"),
        );
        assert!(verify(&token(), &headers, "").is_err());
    }

    #[test]
    fn rejects_origin_scheme_mismatch() {
        let headers = headers_with(
            Some("http://pod.example.ts.net:8443"),
            Some("pod.example.ts.net:8443"),
        );
        assert!(verify(&token(), &headers, "").is_err());
    }

    #[test]
    fn rejects_origin_host_mismatch() {
        let headers = headers_with(
            Some("https://evil.example.ts.net:8443"),
            Some("pod.example.ts.net:8443"),
        );
        assert!(verify(&token(), &headers, "").is_err());
    }

    #[test]
    fn rejects_origin_without_host_header() {
        let headers = headers_with(Some("https://pod.example.ts.net:8443"), None);
        assert!(verify(&token(), &headers, "").is_err());
    }

    #[test]
    fn rejects_malformed_origin() {
        let headers = headers_with(Some("not a url"), Some("pod.example.ts.net:8443"));
        assert!(verify(&token(), &headers, "").is_err());
    }
}
