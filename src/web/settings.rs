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
use std::path::Path;

const OWNER_NAME: &str = "owner_name";
const OWNER_EMAIL: &str = "owner_email";
const AUTO_PUBLISH: &str = "auto_publish_default";
const DEFAULT_EXPLICIT: &str = "default_explicit";
const MAX_SOURCE_MINUTES: &str = "max_source_minutes";
const MIN_SOURCE_SECONDS: &str = "min_source_seconds";
const RETENTION_OLD_AUDIO_DAYS: &str = "retention_old_audio_days";
const RETENTION_TMP_HOURS: &str = "retention_tmp_hours";

#[derive(Debug, Clone)]
pub struct AppSettings {
    pub owner_name: String,
    pub owner_email: String,
    pub auto_publish_default: bool,
    pub default_explicit: bool,
    pub max_source_minutes: i64,
    pub min_source_seconds: i64,
    pub retention_old_audio_days: i64,
    pub retention_tmp_hours: i64,
}

#[derive(Debug, Clone)]
pub struct StorageUsage {
    pub total_human: String,
    pub audio_human: String,
    pub tmp_human: String,
    pub audio_percent: u8,
}

#[derive(Template)]
#[template(path = "settings.html")]
pub struct SettingsTemplate {
    csrf: String,
    settings: AppSettings,
    public_base_url: String,
    data_dir: String,
    chunk_chars: usize,
    omnivoice_base_url: String,
    storage: StorageUsage,
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
    pub retention_old_audio_days: i64,
    pub retention_tmp_hours: i64,
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
            retention_old_audio_days: 7,
            retention_tmp_hours: 24,
        }
    }
}

pub async fn index(
    State(state): State<AppState>,
    Extension(token): Extension<CsrfToken>,
) -> AppResult<Html<String>> {
    let settings = load(&state.pool).await?;
    let storage = storage_usage(&state.cfg.data_dir).await;
    Ok(Html(
        SettingsTemplate {
            csrf: token.0,
            settings,
            public_base_url: state.cfg.public_base_url.clone(),
            data_dir: state.cfg.data_dir.display().to_string(),
            chunk_chars: state.cfg.omnivoice.chunk_chars,
            omnivoice_base_url: state.cfg.omnivoice.base_url.clone(),
            storage,
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
    let retention_old_audio_days = values.retention_old_audio_days.to_string();
    let retention_tmp_hours = values.retention_tmp_hours.to_string();
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
        (RETENTION_OLD_AUDIO_DAYS, retention_old_audio_days.as_str()),
        (RETENTION_TMP_HOURS, retention_tmp_hours.as_str()),
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
        retention_old_audio_days: settings::read_i64(pool, RETENTION_OLD_AUDIO_DAYS, 7).await?,
        retention_tmp_hours: settings::read_i64(pool, RETENTION_TMP_HOURS, 24).await?,
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
    if !(1..=365).contains(&form.retention_old_audio_days) {
        return Err(AppError::BadRequest(
            "Số ngày giữ audio cũ phải từ 1 đến 365.".into(),
        ));
    }
    if !(1..=720).contains(&form.retention_tmp_hours) {
        return Err(AppError::BadRequest(
            "Số giờ giữ file tạm phải từ 1 đến 720.".into(),
        ));
    }
    Ok(AppSettings {
        owner_name,
        owner_email,
        auto_publish_default: form.auto_publish_default.is_some(),
        default_explicit: form.default_explicit.is_some(),
        max_source_minutes: form.max_source_minutes,
        min_source_seconds: form.min_source_seconds,
        retention_old_audio_days: form.retention_old_audio_days,
        retention_tmp_hours: form.retention_tmp_hours,
    })
}

async fn storage_usage(data_dir: &Path) -> StorageUsage {
    let total = dir_size(data_dir).await;
    let audio = dir_size(&data_dir.join("audio")).await;
    let tmp = dir_size(&data_dir.join("tmp")).await;
    let audio_percent = if total == 0 {
        0
    } else {
        (audio.saturating_mul(100) / total).min(100) as u8
    };
    StorageUsage {
        total_human: format_bytes(total),
        audio_human: format_bytes(audio),
        tmp_human: format_bytes(tmp),
        audio_percent,
    }
}

async fn dir_size(root: &Path) -> u64 {
    let mut stack = vec![root.to_path_buf()];
    let mut total = 0u64;
    while let Some(dir) = stack.pop() {
        let Ok(mut entries) = tokio::fs::read_dir(&dir).await else {
            continue;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let Ok(file_type) = entry.file_type().await else {
                continue;
            };
            if file_type.is_dir() {
                stack.push(entry.path());
            } else if file_type.is_file() {
                if let Ok(meta) = entry.metadata().await {
                    total = total.saturating_add(meta.len());
                }
            }
        }
    }
    total
}

pub fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let value = bytes as f64;
    if value >= GB {
        format!("{:.1} GB", value / GB)
    } else if value >= MB {
        format!("{:.1} MB", value / MB)
    } else if value >= KB {
        format!("{:.1} KB", value / KB)
    } else {
        format!("{bytes} B")
    }
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
            ("retention_old_audio_days", "14"),
            ("retention_tmp_hours", "48"),
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

        let mut no_retention = SettingsForm::default_values();
        no_retention.retention_old_audio_days = 0;
        assert!(validate(no_retention).is_err());

        let mut too_many_tmp_hours = SettingsForm::default_values();
        too_many_tmp_hours.retention_tmp_hours = 721;
        assert!(validate(too_many_tmp_hours).is_err());
    }

    #[test]
    fn accepts_empty_owner_email_until_feed_is_enabled() {
        let mut form = SettingsForm::default_values();
        form.owner_email.clear();
        assert!(validate(form).is_ok());
    }

    #[test]
    fn formats_storage() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(1536), "1.5 KB");
        assert_eq!(format_bytes(5 * 1024 * 1024), "5.0 MB");
        assert_eq!(format_bytes(2 * 1024 * 1024 * 1024), "2.0 GB");
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
    async fn settings_page_renders_defaults_and_storage() {
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
        assert!(html.contains("Dung lượng"));
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
        assert_eq!(loaded.retention_old_audio_days, 14);
        assert_eq!(loaded.retention_tmp_hours, 48);
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

    #[tokio::test]
    async fn storage_usage_walks_nested_directories() {
        let dir = tempfile::tempdir().unwrap();
        tokio::fs::create_dir_all(dir.path().join("audio/nested"))
            .await
            .unwrap();
        tokio::fs::create_dir_all(dir.path().join("tmp"))
            .await
            .unwrap();
        tokio::fs::write(dir.path().join("audio/a.mp3"), vec![0u8; 1024])
            .await
            .unwrap();
        tokio::fs::write(dir.path().join("audio/nested/b.mp3"), vec![0u8; 512])
            .await
            .unwrap();
        tokio::fs::write(dir.path().join("tmp/c.part"), vec![0u8; 256])
            .await
            .unwrap();

        let usage = storage_usage(dir.path()).await;
        assert_eq!(dir_size(dir.path()).await, 1792);
        assert_eq!(dir_size(&dir.path().join("audio")).await, 1536);
        assert_eq!(usage.audio_human, "1.5 KB");
        assert_eq!(usage.tmp_human, "256 B");
        assert_eq!(usage.total_human, "1.8 KB");
        assert_eq!(usage.audio_percent, 85);
    }
}
