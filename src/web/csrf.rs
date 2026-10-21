use crate::error::{AppError, AppResult};
use crate::services::tokens::constant_time_eq;
use axum::extract::Request;
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
        let value = format!("{CSRF_COOKIE}={token}; Path=/; HttpOnly; SameSite=Strict");
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

fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .filter_map(|part| part.trim().split_once('='))
        .find(|(key, _)| *key == name)
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
    if !constant_time_eq(provided, &token.0) {
        return Err(AppError::Forbidden);
    }
    if let (Some(origin), Some(host)) = (
        headers
            .get(header::ORIGIN)
            .and_then(|value| value.to_str().ok()),
        headers
            .get(header::HOST)
            .and_then(|value| value.to_str().ok()),
    ) {
        let origin_host = Url::parse(origin)
            .ok()
            .and_then(|url| url.host_str().map(str::to_string));
        let host_name = host.rsplit_once(':').map(|(name, _)| name).unwrap_or(host);
        if origin_host.as_deref() != Some(host_name) {
            return Err(AppError::Forbidden);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn accepts_matching_header_and_rejects_missing_or_cross_site() {
        let token = CsrfToken("abc".into());
        let mut headers = axum::http::HeaderMap::new();
        headers.insert("x-csrf-token", HeaderValue::from_static("abc"));
        headers.insert("sec-fetch-site", HeaderValue::from_static("same-origin"));
        assert!(verify(&token, &headers, "").is_ok());

        let mut missing = axum::http::HeaderMap::new();
        missing.insert("sec-fetch-site", HeaderValue::from_static("same-origin"));
        assert!(verify(&token, &missing, "").is_err());

        let mut cross = axum::http::HeaderMap::new();
        cross.insert("x-csrf-token", HeaderValue::from_static("abc"));
        cross.insert("sec-fetch-site", HeaderValue::from_static("cross-site"));
        assert!(verify(&token, &cross, "").is_err());
    }
}
