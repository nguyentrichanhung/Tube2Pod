use crate::config::Config;
use crate::db::{
    self,
    shows::{self, NewShow},
};
use crate::state::AppState;
use crate::worker::Pipeline;
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use std::{os::unix::fs::PermissionsExt, sync::Arc};
use tokio::sync::Notify;
use tower::ServiceExt;

fn fake_tool(dir: &std::path::Path, name: &str, body: &str) {
    let path = dir.join(name);
    std::fs::write(&path, body).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[tokio::test]
async fn youtube_url_becomes_a_published_feed_item() {
    let dir = tempfile::tempdir().unwrap();
    let args = dir.path().join("args");
    fake_tool(
        dir.path(),
        "yt-dlp",
        &format!(
            r##"#!/bin/sh
if [ "$1" = "--dump-single-json" ]; then
  printf '%s' '{{"title":"Video", "description":"Source description", "duration":600, "formats":[{{"format_id":"main", "audio_track_id":"main", "acodec":"opus", "vcodec":"none", "format_note":"main (default)"}}, {{"format_id":"alternate", "audio_track_id":"alt", "acodec":"opus", "vcodec":"none", "format_note":"alternate"}}]}}'
  exit 0
fi
printf '%s\n' "$@" > '{}'
while [ $# -gt 0 ]; do if [ "$1" = -o ]; then shift; out=$1; fi; shift; done
out=$(printf '%s' "$out" | sed 's/%(ext)s/webm/')
printf audio > "$out"
printf '%s\n' "$out"
"##,
            args.display()
        ),
    );
    let fixture =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/audio_sine.mp3");
    fake_tool(
        dir.path(),
        "ffmpeg",
        &format!(
            "#!/bin/sh\nfor a in \"$@\"; do out=$a; done\ncp '{}' \"$out\"\n",
            fixture.display()
        ),
    );
    fake_tool(dir.path(), "ffprobe", "#!/bin/sh\necho 1.044898\n");
    let cfg = Arc::new(
        Config::from_lookup(|key| match key {
            "PUBLIC_BASE_URL" => Some("https://pod.example.ts.net:8443".into()),
            "OMNIVOICE_TAILNET_USER" => Some("owner".into()),
            "DATA_DIR" => Some(dir.path().display().to_string()),
            "YTDLP_PATH" => Some(dir.path().join("yt-dlp").display().to_string()),
            "FFMPEG_PATH" => Some(dir.path().join("ffmpeg").display().to_string()),
            "FFPROBE_PATH" => Some(dir.path().join("ffprobe").display().to_string()),
            _ => None,
        })
        .unwrap(),
    );
    let pool = db::test_pool().await;
    let show = shows::create(
        &pool,
        NewShow {
            title: "Show".into(),
            description: "Description".into(),
            category: "Technology".into(),
            owner_name: "Owner".into(),
            owner_email: "owner@example.com".into(),
            auto_publish: true,
            ..NewShow::default()
        },
    )
    .await
    .unwrap();
    shows::set_cover(&pool, show.id, "/tmp/cover.jpg", false, false)
        .await
        .unwrap();
    let show = shows::set_feed_enabled(&pool, show.id, true).await.unwrap();
    let state = AppState {
        pool: pool.clone(),
        cfg: cfg.clone(),
        notify: Arc::new(Notify::new()),
        health_cache: Arc::new(tokio::sync::Mutex::new(None)),
    };
    let csrf = "a".repeat(64);
    let body = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("_csrf", &csrf)
        .append_pair("url", "https://www.youtube.com/watch?v=dQw4w9WgXcQ")
        .append_pair("show_id", &show.id.to_string())
        .append_pair("preview_video", "dQw4w9WgXcQ")
        .append_pair("source_audio_track", "id:alt")
        .append_pair("language", "en")
        .finish();
    let response = crate::web::admin_router(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/episodes")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("cookie", format!("t2p_csrf={csrf}"))
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(response.status().is_redirection());
    let episode = db::episodes::claim_next(&pool).await.unwrap().unwrap();
    Pipeline::new(pool.clone(), cfg)
        .run_episode(&episode.uuid)
        .await
        .unwrap();
    let episode = db::episodes::get(&pool, &episode.uuid)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(episode.status, "published");
    let stages: Vec<String> =
        sqlx::query_scalar("SELECT stage FROM stage_events WHERE episode_uuid=?1 ORDER BY id")
            .bind(&episode.uuid)
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(stages, ["queued", "fetching", "assembling", "publishing"]);
    assert_eq!(episode.audio_duration_s, Some(1));
    assert_eq!(
        episode.audio_bytes,
        Some(include_bytes!("../tests/fixtures/audio_sine.mp3").len() as i64)
    );
    assert!(std::fs::read_to_string(args)
        .unwrap()
        .contains("-f\nalternate\n"));
    let router = crate::web::public::public_router(state);
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/feed/{}.xml", show.feed_token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let xml = String::from_utf8(
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
    .unwrap();
    assert!(xml.contains("Source description"));
    assert!(xml.contains(&format!("{}-1.mp3", episode.uuid)));
    let response = router
        .oneshot(
            Request::builder()
                .uri(format!("/audio/{}/{}-1.mp3", show.feed_token, episode.uuid))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        &response.into_body().collect().await.unwrap().to_bytes()[..],
        include_bytes!("../tests/fixtures/audio_sine.mp3")
    );
}
