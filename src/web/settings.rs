use crate::db::settings;
use crate::db::shows;
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::web::csrf::{self, CsrfToken};
use askama::Template;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{Html, Redirect};
use axum::Extension;
use axum::Form;
use serde::Deserialize;

const OWNER_NAME: &str = "owner_name";
const OWNER_EMAIL: &str = "owner_email";
const AUTO_PUBLISH: &str = "auto_publish_default";
const DEFAULT_EXPLICIT: &str = "default_explicit";
const MAX_SOURCE_MINUTES: &str = "max_source_minutes";
const MIN_SOURCE_SECONDS: &str = "min_source_seconds";

#[derive(Debug, Clone)]
pub struct AppSettings {
    pub owner_name: String,
    pub owner_email: String,
    pub auto_publish_default: bool,
    pub default_explicit: bool,
    pub max_source_minutes: i64,
    pub min_source_seconds: i64,
}

#[derive(Template)]
#[template(path = "settings.html")]
pub struct SettingsTemplate {
    csrf: String,
    settings: AppSettings,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SettingsForm {
    pub _csrf: String,
    pub owner_name: String,
    pub owner_email: String,
    pub auto_publish_default: Option<String>,
    pub default_explicit: Option<String>,
    pub max_source_minutes: i64,
    pub min_source_seconds: i64,
}

impl SettingsForm {
    #[cfg(test)]
    pub fn default_values() -> Self {
        Self {
            _csrf: "token".into(),
            owner_name: "Owner".into(),
            owner_email: "owner@example.com".into(),
            auto_publish_default: Some("on".into()),
            default_explicit: None,
            max_source_minutes: 360,
            min_source_seconds: 60,
        }
    }
}

pub async fn index(
    State(state): State<AppState>,
    Extension(token): Extension<CsrfToken>,
) -> AppResult<Html<String>> {
    let settings = load(&state.pool).await?;
    Ok(Html(
        SettingsTemplate {
            csrf: token.0,
            settings,
        }
        .render()?,
    ))
}

pub async fn save(
    State(state): State<AppState>,
    Extension(token): Extension<CsrfToken>,
    headers: HeaderMap,
    Form(form): Form<SettingsForm>,
) -> AppResult<Redirect> {
    csrf::verify(&token, &headers, &form._csrf)?;
    let values = validate(form)?;
    let max_source_minutes = values.max_source_minutes.to_string();
    let min_source_seconds = values.min_source_seconds.to_string();
    let pairs = [
        (OWNER_NAME, values.owner_name.as_str()),
        (OWNER_EMAIL, values.owner_email.as_str()),
        (
            AUTO_PUBLISH,
            if values.auto_publish_default {
                "true"
            } else {
                "false"
            },
        ),
        (
            DEFAULT_EXPLICIT,
            if values.default_explicit {
                "true"
            } else {
                "false"
            },
        ),
        (MAX_SOURCE_MINUTES, max_source_minutes.as_str()),
        (MIN_SOURCE_SECONDS, min_source_seconds.as_str()),
    ];
    let mut tx = state.pool.begin().await?;
    settings::set_many_in_tx(&mut tx, &pairs).await?;
    shows::sync_owner_in_tx(&mut tx, &values.owner_name, &values.owner_email).await?;
    tx.commit().await?;
    Ok(Redirect::to("/settings"))
}

pub async fn load(pool: &sqlx::SqlitePool) -> AppResult<AppSettings> {
    Ok(AppSettings {
        owner_name: settings::get_or(pool, OWNER_NAME, "").await?,
        owner_email: settings::get_or(pool, OWNER_EMAIL, "").await?,
        auto_publish_default: settings::get_or(pool, AUTO_PUBLISH, "true").await? == "true",
        default_explicit: settings::get_or(pool, DEFAULT_EXPLICIT, "false").await? == "true",
        max_source_minutes: settings::read_i64(pool, MAX_SOURCE_MINUTES, 360).await?,
        min_source_seconds: settings::read_i64(pool, MIN_SOURCE_SECONDS, 60).await?,
    })
}

pub fn validate(form: SettingsForm) -> AppResult<AppSettings> {
    let owner_name = form.owner_name.trim().to_string();
    let owner_email = form.owner_email.trim().to_string();
    if !owner_email.is_empty()
        && (!owner_email.contains('@') || owner_email.chars().any(char::is_whitespace))
    {
        return Err(AppError::BadRequest(
            "Email chủ sở hữu không hợp lệ.".into(),
        ));
    }
    if !(30..=600).contains(&form.max_source_minutes) {
        return Err(AppError::BadRequest(
            "Giới hạn video phải từ 30 đến 600 phút.".into(),
        ));
    }
    if !(10..=600).contains(&form.min_source_seconds) {
        return Err(AppError::BadRequest(
            "Độ dài tối thiểu phải từ 10 đến 600 giây.".into(),
        ));
    }
    Ok(AppSettings {
        owner_name,
        owner_email,
        auto_publish_default: form.auto_publish_default.is_some(),
        default_explicit: form.default_explicit.is_some(),
        max_source_minutes: form.max_source_minutes,
        min_source_seconds: form.min_source_seconds,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::db::settings as db_settings;
    use crate::state::AppState;
    use axum::body::{to_bytes, Body};
    use axum::http::{header, Request, StatusCode};
    use std::sync::Arc;
    use tokio::sync::Notify;
    use tower::ServiceExt;

    const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    struct Ctx {
        state: AppState,
        _dir: tempfile::TempDir,
    }

    async fn ctx() -> Ctx {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();
        let cfg = Config::from_lookup(move |key| match key {
            "PUBLIC_BASE_URL" => Some("https://pod.example.ts.net:8443".into()),
            "OMNIVOICE_TAILNET_USER" => Some("owner".into()),
            "DATA_DIR" => Some(path.display().to_string()),
            _ => None,
        })
        .unwrap();
        Ctx {
            state: AppState {
                pool: crate::db::test_pool().await,
                cfg: Arc::new(cfg),
                notify: Arc::new(Notify::new()),
                health_cache: Arc::new(tokio::sync::Mutex::new(None)),
            },
            _dir: dir,
        }
    }

    fn form(pairs: &[(&str, &str)]) -> String {
        let mut serializer = url::form_urlencoded::Serializer::new(String::new());
        for (key, value) in pairs {
            serializer.append_pair(key, value);
        }
        serializer.finish()
    }

    async fn send(
        context: &Ctx,
        method: &str,
        path: &str,
        content_type: &str,
        body: Vec<u8>,
        extra_headers: &[(&str, &str)],
    ) -> axum::response::Response {
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header(header::CONTENT_TYPE, content_type)
            .header(header::COOKIE, format!("t2p_csrf={TOKEN}"));
        for (key, value) in extra_headers {
            request = request.header(*key, *value);
        }
        crate::web::admin_router(context.state.clone())
            .oneshot(request.body(Body::from(body)).unwrap())
            .await
            .unwrap()
    }

    fn valid_pairs() -> Vec<(&'static str, &'static str)> {
        vec![
            ("_csrf", TOKEN),
            ("owner_name", "Chủ Podcast"),
            ("owner_email", "owner@example.com"),
            ("auto_publish_default", "on"),
            ("max_source_minutes", "120"),
            ("min_source_seconds", "30"),
        ]
    }

    #[test]
    fn rejects_invalid_email_and_bounds() {
        let mut form = SettingsForm::default_values();
        form.owner_email = "not-an-email".into();
        assert!(validate(form.clone()).is_err());

        let mut too_long = SettingsForm::default_values();
        too_long.max_source_minutes = 601;
        assert!(validate(too_long).is_err());

        let mut too_short = SettingsForm::default_values();
        too_short.min_source_seconds = 9;
        assert!(validate(too_short).is_err());

        let mut negative = SettingsForm::default_values();
        negative.max_source_minutes = -1;
        assert!(validate(negative).is_err());
    }

    #[test]
    fn accepts_empty_owner_email_until_feed_is_enabled() {
        let mut form = SettingsForm::default_values();
        form.owner_email.clear();
        assert!(validate(form).is_ok());
    }

    #[test]
    fn reads_integer_settings_with_defaults_and_fallbacks() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let pool = crate::db::test_pool().await;
            assert_eq!(
                db_settings::read_i64(&pool, "max_source_minutes", 360)
                    .await
                    .unwrap(),
                360
            );
            db_settings::set(&pool, "max_source_minutes", "90")
                .await
                .unwrap();
            assert_eq!(
                db_settings::read_i64(&pool, "max_source_minutes", 360)
                    .await
                    .unwrap(),
                90
            );
            db_settings::set(&pool, "max_source_minutes", "not-a-number")
                .await
                .unwrap();
            assert_eq!(
                db_settings::read_i64(&pool, "max_source_minutes", 360)
                    .await
                    .unwrap(),
                360
            );
        });
    }

    #[tokio::test]
    async fn settings_page_renders_only_service_settings() {
        let context = ctx().await;
        let response = send(
            &context,
            "GET",
            "/settings",
            "application/x-www-form-urlencoded",
            Vec::new(),
            &[],
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains("Cài đặt"));
        assert!(html.contains("Giới hạn nguồn"));
        assert!(!html.contains("Dung lượng"));
        assert!(!html.contains("Thông tin triển khai"));
        assert!(!html.contains("retention_old_audio_days"));
        assert!(!html.contains("retention_tmp_hours"));
        assert!(!html.contains("127.0.0.1"));
        assert!(!html.contains("https://pod.example.ts.net:8443"));
        assert!(html.contains(&format!(r#"value="{TOKEN}""#)));
        assert!(html.contains(r#"name="max_source_minutes" min="30" max="600" value="360""#));
    }

    #[tokio::test]
    async fn settings_page_explains_global_owner() {
        let context = ctx().await;
        let response = send(&context, "GET", "/settings", "", Vec::new(), &[]).await;
        let html = String::from_utf8(
            to_bytes(response.into_body(), 1024 * 1024)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert!(html.contains("Áp dụng cho mọi show và feed"));
    }

    #[tokio::test]
    async fn saves_valid_settings_and_redirects() {
        let context = ctx().await;
        db_settings::set(&context.state.pool, "retention_old_audio_days", "14")
            .await
            .unwrap();
        db_settings::set(&context.state.pool, "retention_tmp_hours", "48")
            .await
            .unwrap();
        let response = send(
            &context,
            "POST",
            "/settings",
            "application/x-www-form-urlencoded",
            form(&valid_pairs()).into_bytes(),
            &[],
        )
        .await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            response
                .headers()
                .get(header::LOCATION)
                .unwrap()
                .to_str()
                .unwrap(),
            "/settings"
        );
        let loaded = load(&context.state.pool).await.unwrap();
        assert_eq!(loaded.owner_name, "Chủ Podcast");
        assert_eq!(loaded.owner_email, "owner@example.com");
        assert!(loaded.auto_publish_default);
        assert!(!loaded.default_explicit);
        assert_eq!(loaded.max_source_minutes, 120);
        assert_eq!(loaded.min_source_seconds, 30);
        assert_eq!(
            db_settings::read_i64(&context.state.pool, "retention_old_audio_days", 7)
                .await
                .unwrap(),
            14
        );
        assert_eq!(
            db_settings::read_i64(&context.state.pool, "retention_tmp_hours", 24)
                .await
                .unwrap(),
            48
        );
    }

    #[tokio::test]
    async fn rejects_bad_csrf_and_out_of_range_values_without_writing() {
        let context = ctx().await;

        let mut bad_token = valid_pairs();
        bad_token[0] = ("_csrf", "deadbeef");
        let response = send(
            &context,
            "POST",
            "/settings",
            "application/x-www-form-urlencoded",
            form(&bad_token).into_bytes(),
            &[],
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);

        let response = send(
            &context,
            "POST",
            "/settings",
            "application/x-www-form-urlencoded",
            form(&valid_pairs()).into_bytes(),
            &[("sec-fetch-site", "cross-site")],
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);

        let mut out_of_range = valid_pairs();
        out_of_range[4] = ("max_source_minutes", "-5");
        let response = send(
            &context,
            "POST",
            "/settings",
            "application/x-www-form-urlencoded",
            form(&out_of_range).into_bytes(),
            &[],
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        assert_eq!(
            db_settings::get(&context.state.pool, "owner_name")
                .await
                .unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn saving_settings_syncs_owner_to_every_show() {
        let context = ctx().await;
        for title in ["Alpha", "Beta"] {
            crate::db::shows::create(
                &context.state.pool,
                crate::db::shows::NewShow {
                    title: title.into(),
                    owner_name: "Old".into(),
                    owner_email: "old@example.com".into(),
                    ..crate::db::shows::NewShow::default()
                },
            )
            .await
            .unwrap();
        }

        let response = send(
            &context,
            "POST",
            "/settings",
            "application/x-www-form-urlencoded",
            form(&valid_pairs()).into_bytes(),
            &[],
        )
        .await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);

        let shows = crate::db::shows::list(&context.state.pool).await.unwrap();
        assert_eq!(shows.len(), 2);
        for show in shows {
            assert_eq!(show.owner_name, "Chủ Podcast");
            assert_eq!(show.owner_email, "owner@example.com");
        }
    }
}
