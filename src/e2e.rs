use crate::config::Config;
use crate::db::{
    self,
    shows::{self, NewShow},
};
use crate::state::AppState;
use crate::worker::Pipeline;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;
use tokio::sync::Notify;
use tower::ServiceExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn fake_tool(dir: &std::path::Path, name: &str, body: &str) {
    let path = dir.join(name);
    std::fs::write(&path, body).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[tokio::test]
async fn youtube_url_becomes_a_published_feed_item() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/voices"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "data": [{"id": 42, "name": "Giọng vi", "status": "ready", "language": "vi"}],
            "error": null
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/speech"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true, "data": {"id": 1, "status": "queued"}, "error": null
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/speech/1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true, "data": {"id": 1, "status": "done"}, "error": null
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/speech/1/audio"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"ID3fake-mp3".to_vec()))
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/api/speech/1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true, "data": null, "error": null
        })))
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let fixture =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ytdlp_vi.json");
    let caption =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/captions_vi.json3");
    fake_tool(
        dir.path(),
        "fake-ytdlp.sh",
        &format!(
            "#!/bin/sh\nif [ \"$1\" = \"--dump-single-json\" ]; then cat '{}'; exit 0; fi\nout=''\nwhile [ $# -gt 0 ]; do\n  if [ \"$1\" = \"-o\" ]; then out=\"$2\"; shift; fi\n  shift\ndone\ncp '{}' \"${{out%.*}}.json3\"\n",
            fixture.display(),
            caption.display()
        ),
    );
    fake_tool(
        dir.path(),
        "fake-ffmpeg.sh",
        "#!/bin/sh\nlist=''\nout=''\nwhile [ $# -gt 0 ]; do\n  case \"$1\" in\n    -i) list=\"$2\"; shift 2 ;;\n    *) out=\"$1\"; shift ;;\n  esac\ndone\nfirst=$(sed -n \"s/^file '\\(.*\\)'$/\\1/p\" \"$list\" | head -n 1)\ncp \"$first\" \"$out\"\n",
    );
    fake_tool(dir.path(), "fake-ffprobe.sh", "#!/bin/sh\necho 600\n");

    let cfg = Config::from_lookup(|key| match key {
        "PUBLIC_BASE_URL" => Some("https://pod.example.ts.net:8443".into()),
        "OMNIVOICE_TAILNET_USER" => Some("owner@example.com".into()),
        "OMNIVOICE_BASE_URL" => Some(server.uri()),
        "DATA_DIR" => Some(dir.path().display().to_string()),
        "YTDLP_PATH" => Some(dir.path().join("fake-ytdlp.sh").display().to_string()),
        "FFMPEG_PATH" => Some(dir.path().join("fake-ffmpeg.sh").display().to_string()),
        "FFPROBE_PATH" => Some(dir.path().join("fake-ffprobe.sh").display().to_string()),
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
            auto_publish: true,
            ..NewShow::default()
        },
    )
    .await
    .unwrap();
    let show = shows::set_cover(&pool, show.id, "/tmp/cover.jpg", false, false)
        .await
        .unwrap();
    let show = shows::set_feed_enabled(&pool, show.id, true).await.unwrap();
    shows::upsert_voice(&pool, show.id, "vi", 42, "Giọng vi")
        .await
        .unwrap();
    let episode = db::episodes::create(
        &pool,
        db::episodes::NewEpisode {
            show_id: show.id,
            video_id: "dQw4w9WgXcQ".into(),
            source_url: "https://www.youtube.com/watch?v=dQw4w9WgXcQ".into(),
            source_title: "placeholder".into(),
            source_channel: "".into(),
            source_duration_s: None,
            source_thumbnail_url: None,
            source_upload_date: None,
            language: "vi".into(),
            target_minutes: 10,
        },
    )
    .await
    .unwrap();

    let cfg = Arc::new(cfg);
    let pipeline = Pipeline::new(pool.clone(), cfg.clone());
    pipeline.run_episode(&episode.uuid).await.unwrap();

    let published = db::episodes::get(&pool, &episode.uuid)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(published.status, crate::models::status::PUBLISHED);
    assert_eq!(published.stage, crate::models::stage::PUBLISHED);
    assert_eq!(published.source_title, "Một video thử nghiệm");
    let audio_path = published.audio_path.as_ref().unwrap();
    assert!(std::path::Path::new(audio_path).exists());
    assert_eq!(std::fs::read(audio_path).unwrap(), b"ID3fake-mp3");

    let state = AppState {
        pool,
        cfg,
        notify: Arc::new(Notify::new()),
    };
    let response = crate::web::public::public_router(state)
        .oneshot(
            Request::builder()
                .uri(format!("/feed/{}.xml", show.feed_token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let xml = String::from_utf8_lossy(&body);
    assert!(xml.contains("Một video thử nghiệm"));
    assert!(xml.contains(&format!("tube2pod:{}", episode.uuid)));
    assert!(xml.contains(&format!("{}-1.mp3", episode.uuid)));
}
