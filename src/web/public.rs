use crate::db::{episodes, shows};
use crate::error::{AppError, AppResult};
use crate::models::Episode;
use crate::services::feed::{self, FeedEpisode};
use crate::state::AppState;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, HeaderValue, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use std::path::PathBuf;
use std::time::UNIX_EPOCH;
use tower::ServiceExt;
use tower_http::services::ServeFile;

const FEED_CACHE: &str = "public, max-age=300";
const AUDIO_CACHE: &str = "public, max-age=31536000, immutable";
const COVER_CACHE: &str = "public, max-age=31536000, immutable";

pub fn public_router(state: AppState) -> Router {
    Router::new()
        .route("/feed/{file}", get(feed_xml))
        .route("/audio/{token}/{file}", get(audio))
        .route("/cover/{file}", get(cover))
        .route("/healthz", get(|| async { "ok" }))
        .with_state(state)
}

pub async fn feed_xml(
    State(state): State<AppState>,
    Path(file): Path<String>,
    headers: HeaderMap,
) -> AppResult<Response> {
    let token = file.strip_suffix(".xml").ok_or(AppError::NotFound)?;
    let show = shows::get_by_feed_token(&state.pool, token)
        .await?
        .ok_or(AppError::NotFound)?;
    if !show.feed_enabled || !show.feed_complete() {
        return Err(AppError::NotFound);
    }

    let rows = episodes::list_feed(&state.pool, show.id).await?;
    let mut feed_episodes = Vec::with_capacity(rows.len());
    for episode in rows {
        let Some(audio_path) = &episode.audio_path else {
            continue;
        };
        if !tokio::fs::try_exists(audio_path).await.unwrap_or(false) {
            continue;
        }
        feed_episodes.push(to_feed_episode(&show, &episode, &state.cfg.public_base_url));
    }

    let last_build = feed_episodes
        .iter()
        .map(|episode| episode.updated_at)
        .fold(show.updated_at, i64::max);
    let etag = feed::etag(&show, &feed_episodes, last_build);
    if if_none_match(&headers, &etag) {
        let mut response = StatusCode::NOT_MODIFIED.into_response();
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static(FEED_CACHE));
        response.headers_mut().insert(
            header::ETAG,
            HeaderValue::from_str(&etag).map_err(|e| AppError::Internal(e.into()))?,
        );
        return Ok(response);
    }

    let xml = feed::render(
        &show,
        &feed_episodes,
        &state.cfg.public_base_url,
        last_build,
    )?;
    Ok((
        [
            (header::CONTENT_TYPE, "application/rss+xml; charset=utf-8"),
            (header::CACHE_CONTROL, FEED_CACHE),
            (header::ETAG, etag.as_str()),
        ],
        xml,
    )
        .into_response())
}

pub async fn audio(
    State(state): State<AppState>,
    Path((token, file)): Path<(String, String)>,
    request: Request<Body>,
) -> AppResult<Response> {
    let (uuid, version) = parse_audio_name(&file).ok_or(AppError::NotFound)?;
    let show = shows::get_by_feed_token(&state.pool, &token)
        .await?
        .ok_or(AppError::NotFound)?;
    if !show.feed_enabled {
        return Err(AppError::NotFound);
    }
    let episode = episodes::get(&state.pool, &uuid)
        .await?
        .ok_or(AppError::NotFound)?;
    if episode.show_id != show.id {
        return Err(AppError::NotFound);
    }
    if file != format!("{uuid}-{version}.mp3") {
        return Err(AppError::NotFound);
    }

    let path = state.cfg.data_dir.join("audio").join(&file);
    let mut response = serve_contained(&state, path, request).await?;
    if cacheable(response.status()) {
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static(AUDIO_CACHE));
    }
    Ok(response)
}

pub async fn cover(
    State(state): State<AppState>,
    Path(file): Path<String>,
    request: Request<Body>,
) -> AppResult<Response> {
    let (token, version) = parse_cover_name(&file).ok_or(AppError::NotFound)?;
    let show = shows::get_by_feed_token(&state.pool, &token)
        .await?
        .ok_or(AppError::NotFound)?;
    if !show.feed_enabled || !show.feed_complete() || version != show.cover_version {
        return Err(AppError::NotFound);
    }
    let path = state
        .cfg
        .data_dir
        .join("shows")
        .join(show.id.to_string())
        .join("cover.jpg");
    let target = resolve_contained(&state, path).await?;
    let metadata = tokio::fs::metadata(&target)
        .await
        .map_err(|_| AppError::NotFound)?;
    let etag = file_etag(&metadata);
    let etag_value = HeaderValue::from_str(&etag).map_err(|e| AppError::Internal(e.into()))?;

    if if_none_match(request.headers(), &etag) {
        let mut response = StatusCode::NOT_MODIFIED.into_response();
        response.headers_mut().insert(header::ETAG, etag_value);
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static(COVER_CACHE));
        return Ok(response);
    }

    let mut response = serve_file(target, request).await?;
    if cacheable(response.status()) {
        response.headers_mut().insert(header::ETAG, etag_value);
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static(COVER_CACHE));
    }
    Ok(response)
}

pub(crate) async fn serve_contained(
    state: &AppState,
    path: PathBuf,
    request: Request<Body>,
) -> AppResult<Response> {
    let target = resolve_contained(state, path).await?;
    serve_file(target, request).await
}

pub(crate) async fn resolve_contained(state: &AppState, path: PathBuf) -> AppResult<PathBuf> {
    let target = tokio::fs::canonicalize(&path)
        .await
        .map_err(|_| AppError::NotFound)?;
    let root = tokio::fs::canonicalize(&state.cfg.data_dir)
        .await
        .map_err(|e| AppError::Internal(e.into()))?;
    if !target.starts_with(&root) {
        return Err(AppError::NotFound);
    }
    Ok(target)
}

async fn serve_file(path: PathBuf, request: Request<Body>) -> AppResult<Response> {
    let response = ServeFile::new(path)
        .oneshot(request)
        .await
        .map_err(|e| AppError::Internal(e.into()))?;
    Ok(response.map(Body::new))
}

fn cacheable(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::OK | StatusCode::PARTIAL_CONTENT | StatusCode::NOT_MODIFIED
    )
}

fn if_none_match(headers: &HeaderMap, etag: &str) -> bool {
    headers
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value.split(',').any(|candidate| {
                let candidate = candidate.trim();
                candidate == "*" || candidate == etag || candidate.strip_prefix("W/") == Some(etag)
            })
        })
}

fn file_etag(metadata: &std::fs::Metadata) -> String {
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!("\"{modified:x}-{:x}\"", metadata.len())
}

fn to_feed_episode(show: &crate::models::Show, episode: &Episode, base_url: &str) -> FeedEpisode {
    FeedEpisode {
        uuid: episode.uuid.clone(),
        title: episode.source_title.clone(),
        description: episode.summary().to_string(),
        source_url: episode.source_url.clone(),
        published_at: episode.published_at.unwrap_or(episode.created_at),
        updated_at: episode.updated_at,
        audio_url: format!(
            "{base_url}/audio/{}/{}-{}.mp3",
            show.feed_token, episode.uuid, episode.audio_version
        ),
        audio_bytes: episode.audio_bytes.unwrap_or(0),
        audio_duration_s: episode.audio_duration_s.unwrap_or(0),
        explicit: show.explicit,
    }
}

fn parse_audio_name(file: &str) -> Option<(String, i64)> {
    if file.contains('/') || file.contains('\\') || file.contains("..") {
        return None;
    }
    let stem = file.strip_suffix(".mp3")?;
    let (uuid, version) = stem.rsplit_once('-')?;
    uuid::Uuid::parse_str(uuid).ok()?;
    let version = version.parse::<i64>().ok()?;
    (version > 0).then(|| (uuid.to_string(), version))
}

fn parse_cover_name(file: &str) -> Option<(String, i64)> {
    if file.contains('/') || file.contains('\\') || file.contains("..") {
        return None;
    }
    let stem = file.strip_suffix(".jpg")?;
    let (token, version) = stem.rsplit_once('-')?;
    if token.is_empty()
        || !token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return None;
    }
    let version = version.parse::<i64>().ok()?;
    (version > 0).then(|| (token.to_string(), version))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{
        self,
        episodes::{self, NewEpisode},
        shows::{self, NewShow},
    };
    use crate::models::{stage, status, Episode};
    use crate::services::feed;
    use axum::body::Body;
    use axum::http::{header, Request, StatusCode};
    use axum::response::Response;
    use http_body_util::BodyExt;
    use std::sync::Arc;
    use tokio::sync::Notify;
    use tower::ServiceExt;

    const BASE: &str = "https://pod.example.ts.net:8443";
    const AUDIO: &[u8] = b"ID3fake-mp3";

    struct Fixture {
        state: crate::state::AppState,
        _dir: tempfile::TempDir,
        token: String,
        uuid: String,
        show_id: i64,
    }

    async fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let cfg = crate::config::Config::from_lookup(|key| match key {
            "PUBLIC_BASE_URL" => Some(BASE.into()),
            "OMNIVOICE_TAILNET_USER" => Some("owner".into()),
            "DATA_DIR" => Some(dir.path().display().to_string()),
            _ => None,
        })
        .unwrap();
        let pool = db::test_pool().await;
        let show = shows::create(
            &pool,
            NewShow {
                title: "Show".into(),
                description: "Mô tả".into(),
                category: "Technology".into(),
                owner_name: "Owner".into(),
                owner_email: "owner@example.com".into(),
                explicit: true,
                ..NewShow::default()
            },
        )
        .await
        .unwrap();
        let cover = cfg
            .data_dir
            .join("shows")
            .join(show.id.to_string())
            .join("cover.jpg");
        tokio::fs::create_dir_all(cover.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(&cover, b"\xff\xd8\xff\xe0cover")
            .await
            .unwrap();
        let show = shows::set_cover(&pool, show.id, &cover.display().to_string(), false, false)
            .await
            .unwrap();
        let show = shows::set_feed_enabled(&pool, show.id, true).await.unwrap();
        let episode = episodes::create(
            &pool,
            NewEpisode {
                show_id: show.id,
                video_id: "v1".into(),
                source_url: "https://www.youtube.com/watch?v=v1".into(),
                source_title: "Tập 1".into(),
                source_channel: "Kênh".into(),
                source_duration_s: Some(600),
                source_thumbnail_url: None,
                source_upload_date: None,
                language: "vi".into(),
                target_minutes: 10,
                review_required: false,
            },
        )
        .await
        .unwrap();
        let state = crate::state::AppState {
            pool,
            cfg: Arc::new(cfg),
            notify: Arc::new(Notify::new()),
        };
        install_audio(&state, &episode).await;
        episodes::finish(
            &state.pool,
            &episode.uuid,
            stage::PUBLISHED,
            status::PUBLISHED,
            true,
        )
        .await
        .unwrap();

        Fixture {
            token: show.feed_token.clone(),
            uuid: episode.uuid.clone(),
            show_id: show.id,
            state,
            _dir: dir,
        }
    }

    async fn install_audio(state: &crate::state::AppState, episode: &Episode) {
        let path = state
            .cfg
            .data_dir
            .join("audio")
            .join(format!("{}-1.mp3", episode.uuid));
        tokio::fs::create_dir_all(path.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(&path, AUDIO).await.unwrap();
        episodes::set_audio(
            &state.pool,
            &episode.uuid,
            &path.display().to_string(),
            AUDIO.len() as i64,
            600,
            "hash",
            1,
        )
        .await
        .unwrap();
    }

    async fn add_episode(state: &crate::state::AppState, show_id: i64, video_id: &str) -> Episode {
        episodes::create(
            &state.pool,
            NewEpisode {
                show_id,
                video_id: video_id.into(),
                source_url: format!("https://www.youtube.com/watch?v={video_id}"),
                source_title: format!("Tập {video_id}"),
                source_channel: "Kênh".into(),
                source_duration_s: Some(600),
                source_thumbnail_url: None,
                source_upload_date: None,
                language: "vi".into(),
                target_minutes: 10,
                review_required: false,
            },
        )
        .await
        .unwrap()
    }

    async fn get(state: &crate::state::AppState, uri: &str) -> Response {
        request(state, Request::builder().uri(uri)).await
    }

    async fn request(
        state: &crate::state::AppState,
        builder: axum::http::request::Builder,
    ) -> Response {
        public_router(state.clone())
            .oneshot(builder.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    async fn text(response: Response) -> String {
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    async fn empty(response: Response) -> bool {
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .is_empty()
    }

    #[tokio::test]
    async fn serves_feed_with_guid_audio_url_and_show_explicit() {
        let fx = fixture().await;
        let response = get(&fx.state, &format!("/feed/{}.xml", fx.token)).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .unwrap()
                .to_str()
                .unwrap(),
            "application/rss+xml; charset=utf-8"
        );
        assert_eq!(
            response
                .headers()
                .get(header::CACHE_CONTROL)
                .unwrap()
                .to_str()
                .unwrap(),
            "public, max-age=300"
        );
        let xml = text(response).await;
        assert!(xml.contains(&format!("tube2pod:{}", fx.uuid)));
        assert!(xml.contains(&format!("{BASE}/audio/{}/{}-1.mp3", fx.token, fx.uuid)));
        assert!(xml.contains("<itunes:explicit>true</itunes:explicit>"));

        let missing = get(&fx.state, "/feed/nope.xml").await;
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn feed_description_uses_the_display_summary_not_the_audio_script() {
        let fx = fixture().await;
        episodes::set_summary(
            &fx.state.pool,
            &fx.uuid,
            "Tóm tắt hiển thị.",
            "SCRIPT_NOI_DAY_DU_KHONG_DUOC_LO",
            "hash",
        )
        .await
        .unwrap();

        let xml = text(get(&fx.state, &format!("/feed/{}.xml", fx.token)).await).await;
        assert!(xml.contains("Tóm tắt hiển thị."), "{xml}");
        assert!(!xml.contains("SCRIPT_NOI_DAY_DU_KHONG_DUOC_LO"), "{xml}");
    }

    #[tokio::test]
    async fn feed_is_404_unless_enabled_and_complete() {
        let fx = fixture().await;
        let pool = &fx.state.pool;
        let incomplete = shows::create(
            pool,
            NewShow {
                title: "Chưa đủ".into(),
                ..NewShow::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(
            get(&fx.state, &format!("/feed/{}.xml", incomplete.feed_token))
                .await
                .status(),
            StatusCode::NOT_FOUND
        );

        let mut complete = shows::create(
            pool,
            NewShow {
                title: "Đã đủ".into(),
                description: "Mô tả".into(),
                category: "Technology".into(),
                owner_name: "Owner".into(),
                owner_email: "owner@example.com".into(),
                ..NewShow::default()
            },
        )
        .await
        .unwrap();
        complete = shows::set_cover(pool, complete.id, "/tmp/cover.jpg", false, false)
            .await
            .unwrap();
        assert_eq!(
            get(&fx.state, &format!("/feed/{}.xml", complete.feed_token))
                .await
                .status(),
            StatusCode::NOT_FOUND
        );

        complete = shows::set_feed_enabled(pool, complete.id, true)
            .await
            .unwrap();
        assert_eq!(
            get(&fx.state, &format!("/feed/{}.xml", complete.feed_token))
                .await
                .status(),
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn feed_membership_uses_published_at_and_audio_path() {
        let fx = fixture().await;
        let pool = &fx.state.pool;

        let unpublished = add_episode(&fx.state, fx.show_id, "no-pub").await;
        install_audio(&fx.state, &unpublished).await;

        let no_audio = add_episode(&fx.state, fx.show_id, "no-audio").await;
        episodes::finish(pool, &no_audio.uuid, stage::READY, status::READY, true)
            .await
            .unwrap();

        let missing = add_episode(&fx.state, fx.show_id, "missing").await;
        install_audio(&fx.state, &missing).await;
        episodes::finish(
            pool,
            &missing.uuid,
            stage::PUBLISHED,
            status::PUBLISHED,
            true,
        )
        .await
        .unwrap();
        tokio::fs::remove_file(
            fx.state
                .cfg
                .data_dir
                .join("audio")
                .join(format!("{}-1.mp3", missing.uuid)),
        )
        .await
        .unwrap();

        episodes::reset_for_reread(pool, &fx.uuid).await.unwrap();
        let xml = text(get(&fx.state, &format!("/feed/{}.xml", fx.token)).await).await;

        assert_eq!(xml.matches("<item>").count(), 1, "{xml}");
        assert!(xml.contains(&format!("tube2pod:{}", fx.uuid)), "{xml}");
        for hidden in [&unpublished.uuid, &no_audio.uuid, &missing.uuid] {
            assert!(!xml.contains(hidden), "{hidden} leaked into {xml}");
        }
    }

    #[tokio::test]
    async fn feed_orders_by_published_at_descending() {
        let fx = fixture().await;
        let second = add_episode(&fx.state, fx.show_id, "v2").await;
        install_audio(&fx.state, &second).await;
        episodes::finish(
            &fx.state.pool,
            &second.uuid,
            stage::PUBLISHED,
            status::PUBLISHED,
            true,
        )
        .await
        .unwrap();
        sqlx::query("UPDATE episodes SET published_at = ?1 WHERE uuid = ?2")
            .bind(100_i64)
            .bind(&fx.uuid)
            .execute(&fx.state.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE episodes SET published_at = ?1 WHERE uuid = ?2")
            .bind(200_i64)
            .bind(&second.uuid)
            .execute(&fx.state.pool)
            .await
            .unwrap();
        episodes::reset_for_reread(&fx.state.pool, &fx.uuid)
            .await
            .unwrap();

        let xml = text(get(&fx.state, &format!("/feed/{}.xml", fx.token)).await).await;
        let newest = xml.find(&format!("tube2pod:{}", second.uuid)).unwrap();
        let oldest = xml.find(&format!("tube2pod:{}", fx.uuid)).unwrap();
        assert!(newest < oldest, "{xml}");
    }

    #[tokio::test]
    async fn feed_breaks_published_at_ties_by_uuid_ascending() {
        let fx = fixture().await;
        let second = add_episode(&fx.state, fx.show_id, "v2").await;
        install_audio(&fx.state, &second).await;
        episodes::finish(
            &fx.state.pool,
            &second.uuid,
            stage::PUBLISHED,
            status::PUBLISHED,
            true,
        )
        .await
        .unwrap();

        let low = "00000000-0000-4000-8000-000000000001";
        let high = "ffffffff-ffff-4fff-8fff-ffffffffffff";
        for (old, new) in [(&fx.uuid, high), (&second.uuid, low)] {
            sqlx::query("UPDATE episodes SET uuid = ?1, published_at = ?2 WHERE uuid = ?3")
                .bind(new)
                .bind(100_i64)
                .bind(old)
                .execute(&fx.state.pool)
                .await
                .unwrap();
        }

        let xml = text(get(&fx.state, &format!("/feed/{}.xml", fx.token)).await).await;
        let low_position = xml.find(&format!("tube2pod:{low}")).unwrap();
        let high_position = xml.find(&format!("tube2pod:{high}")).unwrap();
        assert!(low_position < high_position, "{xml}");
    }

    #[tokio::test]
    async fn feed_etag_returns_304_and_tracks_show_edits() {
        let fx = fixture().await;
        let uri = format!("/feed/{}.xml", fx.token);
        let response = get(&fx.state, &uri).await;
        let etag = response
            .headers()
            .get(header::ETAG)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();

        let cached = request(
            &fx.state,
            Request::builder()
                .uri(&uri)
                .header(header::IF_NONE_MATCH, &etag),
        )
        .await;
        assert_eq!(cached.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(
            cached
                .headers()
                .get(header::ETAG)
                .unwrap()
                .to_str()
                .unwrap(),
            etag
        );
        assert_eq!(
            cached
                .headers()
                .get(header::CACHE_CONTROL)
                .unwrap()
                .to_str()
                .unwrap(),
            "public, max-age=300"
        );
        assert!(empty(cached).await);

        let stale = request(
            &fx.state,
            Request::builder()
                .uri(&uri)
                .header(header::IF_NONE_MATCH, "\"stale\""),
        )
        .await;
        assert_eq!(stale.status(), StatusCode::OK);

        sqlx::query("UPDATE episodes SET updated_at = ?1 WHERE show_id = ?2")
            .bind(100_i64)
            .bind(fx.show_id)
            .execute(&fx.state.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE shows SET updated_at = ?1 WHERE id = ?2")
            .bind(200_i64)
            .bind(fx.show_id)
            .execute(&fx.state.pool)
            .await
            .unwrap();
        let xml = text(get(&fx.state, &uri).await).await;
        assert!(xml.contains(&format!(
            "<lastBuildDate>{}</lastBuildDate>",
            feed::rfc2822(200)
        )));
        assert!(!xml.contains(&format!(
            "<lastBuildDate>{}</lastBuildDate>",
            feed::rfc2822(100)
        )));

        sqlx::query("UPDATE shows SET updated_at = ?1 WHERE id = ?2")
            .bind(50_i64)
            .bind(fx.show_id)
            .execute(&fx.state.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE episodes SET updated_at = ?1 WHERE show_id = ?2")
            .bind(300_i64)
            .bind(fx.show_id)
            .execute(&fx.state.pool)
            .await
            .unwrap();
        let xml = text(get(&fx.state, &uri).await).await;
        assert!(xml.contains(&format!(
            "<lastBuildDate>{}</lastBuildDate>",
            feed::rfc2822(300)
        )));
    }

    #[tokio::test]
    async fn supports_head_and_ranged_audio() {
        let fx = fixture().await;
        let uri = format!("/audio/{}/{}-1.mp3", fx.token, fx.uuid);
        let ranged = request(
            &fx.state,
            Request::builder()
                .uri(&uri)
                .header(header::RANGE, "bytes=0-3"),
        )
        .await;
        assert_eq!(ranged.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            ranged
                .headers()
                .get(header::CONTENT_RANGE)
                .unwrap()
                .to_str()
                .unwrap(),
            format!("bytes 0-3/{}", AUDIO.len())
        );
        assert_eq!(
            ranged
                .headers()
                .get(header::CACHE_CONTROL)
                .unwrap()
                .to_str()
                .unwrap(),
            "public, max-age=31536000, immutable"
        );
        let body = ranged.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], &AUDIO[..4]);

        let unsatisfiable = request(
            &fx.state,
            Request::builder()
                .uri(&uri)
                .header(header::RANGE, "bytes=99-100"),
        )
        .await;
        assert_eq!(unsatisfiable.status(), StatusCode::RANGE_NOT_SATISFIABLE);
        assert!(unsatisfiable.headers().get(header::CACHE_CONTROL).is_none());

        let head = request(&fx.state, Request::builder().method("HEAD").uri(&uri)).await;
        assert_eq!(head.status(), StatusCode::OK);
        assert_eq!(
            head.headers()
                .get(header::CONTENT_LENGTH)
                .unwrap()
                .to_str()
                .unwrap(),
            AUDIO.len().to_string()
        );
        assert!(empty(head).await);

        let feed_head = request(
            &fx.state,
            Request::builder()
                .method("HEAD")
                .uri(format!("/feed/{}.xml", fx.token)),
        )
        .await;
        assert_eq!(feed_head.status(), StatusCode::OK);
        assert!(empty(feed_head).await);
    }

    #[tokio::test]
    async fn audio_rejects_traversal_foreign_episodes_and_disabled_feeds() {
        let fx = fixture().await;
        for uri in [
            format!("/audio/{}/../tube2pod.db", fx.token),
            format!("/audio/{}/%2e%2e%2Ftube2pod.db", fx.token),
            format!("/audio/{}/..%2Ftube2pod.db", fx.token),
        ] {
            let status = get(&fx.state, &uri).await.status();
            assert!(
                matches!(status, StatusCode::NOT_FOUND | StatusCode::BAD_REQUEST),
                "{uri} returned {status}"
            );
        }

        let wrong_version = format!("/audio/{}/{}-2.mp3", fx.token, fx.uuid);
        let missing = get(&fx.state, &wrong_version).await;
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
        assert!(missing.headers().get(header::CACHE_CONTROL).is_none());

        let other = shows::create(
            &fx.state.pool,
            NewShow {
                title: "Other".into(),
                description: "Mô tả".into(),
                category: "Technology".into(),
                owner_name: "Owner".into(),
                owner_email: "owner@example.com".into(),
                ..NewShow::default()
            },
        )
        .await
        .unwrap();
        let other = shows::set_cover(&fx.state.pool, other.id, "/tmp/cover.jpg", false, false)
            .await
            .unwrap();
        let other = shows::set_feed_enabled(&fx.state.pool, other.id, true)
            .await
            .unwrap();
        let foreign = add_episode(&fx.state, other.id, "foreign").await;
        install_audio(&fx.state, &foreign).await;

        let foreign_uri = format!("/audio/{}/{}-1.mp3", fx.token, foreign.uuid);
        assert_eq!(
            get(&fx.state, &foreign_uri).await.status(),
            StatusCode::NOT_FOUND
        );

        shows::set_feed_enabled(&fx.state.pool, other.id, false)
            .await
            .unwrap();
        let disabled_uri = format!("/audio/{}/{}-1.mp3", other.feed_token, foreign.uuid);
        assert_eq!(
            get(&fx.state, &disabled_uri).await.status(),
            StatusCode::NOT_FOUND
        );
    }

    #[tokio::test]
    async fn serves_cover_with_etag_cache_and_rejects_bad_names() {
        let fx = fixture().await;
        let uri = format!("/cover/{}-1.jpg", fx.token);
        let response = get(&fx.state, &uri).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .unwrap()
                .to_str()
                .unwrap(),
            "image/jpeg"
        );
        assert_eq!(
            response
                .headers()
                .get(header::CACHE_CONTROL)
                .unwrap()
                .to_str()
                .unwrap(),
            "public, max-age=31536000, immutable"
        );
        let etag = response
            .headers()
            .get(header::ETAG)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        assert!(!etag.is_empty());
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..3], &[0xff, 0xd8, 0xff]);

        let head = request(&fx.state, Request::builder().method("HEAD").uri(&uri)).await;
        assert_eq!(head.status(), StatusCode::OK);
        assert_eq!(
            head.headers().get(header::ETAG).unwrap().to_str().unwrap(),
            etag
        );
        assert!(empty(head).await);

        let cached = request(
            &fx.state,
            Request::builder()
                .uri(&uri)
                .header(header::IF_NONE_MATCH, &etag),
        )
        .await;
        assert_eq!(cached.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(
            cached
                .headers()
                .get(header::ETAG)
                .unwrap()
                .to_str()
                .unwrap(),
            etag
        );
        assert_eq!(
            cached
                .headers()
                .get(header::CACHE_CONTROL)
                .unwrap()
                .to_str()
                .unwrap(),
            "public, max-age=31536000, immutable"
        );
        assert!(empty(cached).await);

        let cover_path = fx
            .state
            .cfg
            .data_dir
            .join("shows")
            .join(fx.show_id.to_string())
            .join("cover.jpg");
        tokio::fs::write(&cover_path, b"\xff\xd8\xff\xe0a-larger-cover")
            .await
            .unwrap();
        let changed = get(&fx.state, &uri).await;
        assert_ne!(
            changed
                .headers()
                .get(header::ETAG)
                .unwrap()
                .to_str()
                .unwrap(),
            etag
        );

        for path in [
            "/cover/nope-1.jpg".to_string(),
            format!("/cover/{}-0.jpg", fx.token),
            format!("/cover/{}-1.png", fx.token),
            format!("/cover/{}-abc.jpg", fx.token),
            "/cover/../tube2pod.db".to_string(),
        ] {
            let response = get(&fx.state, &path).await;
            assert!(
                matches!(
                    response.status(),
                    StatusCode::NOT_FOUND | StatusCode::BAD_REQUEST
                ),
                "{path} returned {}",
                response.status()
            );
            assert!(
                response.headers().get(header::CACHE_CONTROL).is_none(),
                "{path} cached an error"
            );
        }
    }

    #[tokio::test]
    async fn cover_is_hidden_unless_the_feed_is_enabled_and_complete() {
        let fx = fixture().await;
        let uri = format!("/cover/{}-1.jpg", fx.token);
        assert_eq!(get(&fx.state, &uri).await.status(), StatusCode::OK);

        shows::set_feed_enabled(&fx.state.pool, fx.show_id, false)
            .await
            .unwrap();
        assert_eq!(get(&fx.state, &uri).await.status(), StatusCode::NOT_FOUND);

        sqlx::query("UPDATE shows SET feed_enabled = 1, description = '' WHERE id = ?1")
            .bind(fx.show_id)
            .execute(&fx.state.pool)
            .await
            .unwrap();
        assert_eq!(
            get(&fx.state, &uri).await.status(),
            StatusCode::NOT_FOUND,
            "an incomplete feed must not expose the cover"
        );

        sqlx::query("UPDATE shows SET description = 'Mô tả' WHERE id = ?1")
            .bind(fx.show_id)
            .execute(&fx.state.pool)
            .await
            .unwrap();
        assert_eq!(get(&fx.state, &uri).await.status(), StatusCode::OK);
        assert_eq!(
            get(&fx.state, &format!("/cover/{}-99.jpg", fx.token))
                .await
                .status(),
            StatusCode::NOT_FOUND,
            "only the current cover version may be served"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn rejects_symlinks_that_escape_data_dir() {
        use std::os::unix::fs::symlink;

        let fx = fixture().await;
        let outside = tempfile::tempdir().unwrap();
        let secret = outside.path().join("secret.bin");
        tokio::fs::write(&secret, b"secret").await.unwrap();

        let audio = fx
            .state
            .cfg
            .data_dir
            .join("audio")
            .join(format!("{}-1.mp3", fx.uuid));
        tokio::fs::remove_file(&audio).await.unwrap();
        symlink(&secret, &audio).unwrap();
        let response = get(&fx.state, &format!("/audio/{}/{}-1.mp3", fx.token, fx.uuid)).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert!(response.headers().get(header::CACHE_CONTROL).is_none());

        let cover = fx
            .state
            .cfg
            .data_dir
            .join("shows")
            .join(fx.show_id.to_string())
            .join("cover.jpg");
        tokio::fs::remove_file(&cover).await.unwrap();
        symlink(&secret, &cover).unwrap();
        let response = get(&fx.state, &format!("/cover/{}-1.jpg", fx.token)).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert!(response.headers().get(header::CACHE_CONTROL).is_none());
    }

    #[tokio::test]
    async fn healthz_reports_ok() {
        let fx = fixture().await;
        let response = get(&fx.state, "/healthz").await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(text(response).await, "ok");

        let head = request(&fx.state, Request::builder().method("HEAD").uri("/healthz")).await;
        assert_eq!(head.status(), StatusCode::OK);
        assert!(empty(head).await);
    }

    #[tokio::test]
    async fn public_router_does_not_expose_admin_routes() {
        let fx = fixture().await;
        for uri in ["/", "/settings", "/library", "/api/queue"] {
            assert_eq!(
                get(&fx.state, uri).await.status(),
                StatusCode::NOT_FOUND,
                "{uri} leaked onto the public listener"
            );
        }
    }

    #[tokio::test]
    async fn admin_router_does_not_expose_public_routes() {
        let fx = fixture().await;
        for uri in [
            format!("/feed/{}.xml", fx.token),
            format!("/audio/{}/{}-1.mp3", fx.token, fx.uuid),
            format!("/cover/{}-1.jpg", fx.token),
        ] {
            let response = crate::web::admin_router(fx.state.clone())
                .oneshot(Request::builder().uri(&uri).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                StatusCode::NOT_FOUND,
                "{uri} leaked onto the admin listener"
            );
        }
    }

    #[test]
    fn audio_name_parser_is_strict() {
        let uuid = "0f8fad5b-d9cb-469f-a165-70867728950e";
        assert_eq!(
            parse_audio_name(&format!("{uuid}-2.mp3")),
            Some((uuid.into(), 2))
        );
        assert!(parse_audio_name(&format!("{uuid}-0.mp3")).is_none());
        assert!(parse_audio_name("../x-1.mp3").is_none());
        assert!(parse_audio_name(&format!("{uuid}-1.wav")).is_none());
        assert!(parse_audio_name(&format!("{uuid}/-1.mp3")).is_none());
        assert!(parse_audio_name("not-a-uuid-1.mp3").is_none());
    }

    #[test]
    fn cover_name_parser_accepts_url_safe_tokens_only() {
        assert_eq!(
            parse_cover_name("AbC-1_2-3.jpg"),
            Some(("AbC-1_2".into(), 3))
        );
        assert!(parse_cover_name("AbC-1_2-0.jpg").is_none());
        assert!(parse_cover_name("..-1.jpg").is_none());
        assert!(parse_cover_name("token/%2e%2e-1.jpg").is_none());
        assert!(parse_cover_name("tok!.en-1.jpg").is_none());
        assert!(parse_cover_name("token-abc.jpg").is_none());
        assert!(parse_cover_name("token-1.png").is_none());
    }
}
