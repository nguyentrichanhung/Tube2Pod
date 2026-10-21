use axum::extract::Request;
use axum::http::{header, StatusCode};
use axum::middleware::Next;
use axum::response::{Html, IntoResponse, Response};

pub async fn html_errors(request: Request, next: Next) -> Response {
    let wants_html = request
        .headers()
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.contains("text/html"));
    let is_htmx = request.headers().contains_key("hx-request");
    let is_api = request.uri().path().starts_with("/api/");
    let response = next.run(request).await;
    if !wants_html
        || is_htmx
        || is_api
        || !(response.status().is_client_error() || response.status().is_server_error())
    {
        return response;
    }
    let (parts, body) = response.into_parts();
    let body = axum::body::to_bytes(body, 1024 * 1024)
        .await
        .unwrap_or_default();
    let json_message = serde_json::from_slice::<serde_json::Value>(&body)
        .ok()
        .and_then(|value| {
            value
                .get("error")
                .and_then(|error| error.as_str())
                .map(str::to_string)
        });
    let message = html_escape(&friendly_message(parts.status, json_message));
    let mut rendered = (
        parts.status,
        Html(format!(
            "<!doctype html><html lang=\"vi\"><head><meta charset=\"utf-8\"><link rel=\"stylesheet\" href=\"/assets/app.css\"><title>Lỗi</title></head><body><main class=\"page\"><div class=\"card stack\" role=\"alert\"><h1>Yêu cầu không hợp lệ</h1><p>{message}</p><p><a class=\"btn\" href=\"/\">Về trang chủ</a></p></div></main></body></html>"
        )),
    )
        .into_response();
    for (name, value) in parts.headers.iter() {
        if name != header::CONTENT_TYPE
            && name != header::CONTENT_LENGTH
            && name != header::TRANSFER_ENCODING
        {
            rendered.headers_mut().insert(name.clone(), value.clone());
        }
    }
    rendered
}

fn friendly_message(status: StatusCode, json_message: Option<String>) -> String {
    match status {
        StatusCode::FORBIDDEN => {
            "Phiên làm việc không hợp lệ hoặc đã hết hạn. Vui lòng tải lại trang và thử lại.".into()
        }
        StatusCode::BAD_REQUEST | StatusCode::CONFLICT => {
            json_message.unwrap_or_else(|| "Yêu cầu không hợp lệ. Vui lòng kiểm tra lại.".into())
        }
        StatusCode::NOT_FOUND => "Không tìm thấy nội dung.".into(),
        _ => "Không xử lý được yêu cầu. Vui lòng thử lại.".into(),
    }
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
