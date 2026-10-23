use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const LANG_VI: &str = "vi";
pub const LANG_EN: &str = "en";
pub const LANG_KO: &str = "ko";

pub mod stage {
    pub const QUEUED: &str = "queued";
    pub const FETCHING: &str = "fetching";
    pub const SUMMARIZING: &str = "summarizing";
    pub const VOICING: &str = "voicing";
    pub const SYNTHESIZING: &str = "synthesizing";
    pub const ASSEMBLING: &str = "assembling";
    pub const PUBLISHING: &str = "publishing";
    pub const PUBLISHED: &str = "published";
    pub const READY: &str = "ready";
    pub const REVIEW: &str = "review";
}

pub mod status {
    pub const QUEUED: &str = "queued";
    pub const RUNNING: &str = "running";
    pub const READY: &str = "ready";
    pub const PUBLISHED: &str = "published";
    pub const FAILED: &str = "failed";
    pub const CANCELLED: &str = "cancelled";
    pub const REVIEW: &str = "review";
}

#[derive(Clone, sqlx::FromRow, Serialize)]
pub struct Show {
    pub id: i64,
    pub title: String,
    pub author: String,
    pub description: String,
    pub category: String,
    pub owner_name: String,
    pub owner_email: String,
    pub explicit: bool,
    pub language: String,
    pub website_url: String,
    pub cover_path: Option<String>,
    pub cover_version: i64,
    pub cover_custom: bool,
    pub feed_token: String,
    pub feed_enabled: bool,
    pub default_minutes: i64,
    pub auto_publish: bool,
    pub voice_preset_json: String,
    pub created_at: i64,
    pub updated_at: i64,
}

impl std::fmt::Debug for Show {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Show")
            .field("id", &self.id)
            .field("title", &self.title)
            .field("author", &self.author)
            .field("description", &self.description)
            .field("category", &self.category)
            .field("owner_name", &self.owner_name)
            .field("owner_email", &self.owner_email)
            .field("explicit", &self.explicit)
            .field("language", &self.language)
            .field("website_url", &self.website_url)
            .field("cover_path", &self.cover_path)
            .field("cover_version", &self.cover_version)
            .field("cover_custom", &self.cover_custom)
            .field("feed_token", &"<redacted>")
            .field("feed_enabled", &self.feed_enabled)
            .field("default_minutes", &self.default_minutes)
            .field("auto_publish", &self.auto_publish)
            .field("voice_preset_json", &self.voice_preset_json)
            .field("created_at", &self.created_at)
            .field("updated_at", &self.updated_at)
            .finish()
    }
}

impl Show {
    pub fn feed_complete(&self) -> bool {
        !self.title.trim().is_empty()
            && !self.description.trim().is_empty()
            && !self.category.trim().is_empty()
            && !self.owner_name.trim().is_empty()
            && !self.owner_email.trim().is_empty()
            && self
                .cover_path
                .as_deref()
                .is_some_and(|p| !p.trim().is_empty())
    }

    pub fn cover_url(&self, base: &str) -> String {
        format!(
            "{base}/cover/{}-{}.jpg",
            self.feed_token, self.cover_version
        )
    }
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize)]
pub struct Episode {
    pub uuid: String,
    pub show_id: i64,
    pub video_id: String,
    pub source_url: String,
    pub source_title: String,
    pub source_channel: String,
    pub source_duration_s: Option<i64>,
    pub source_thumbnail_url: Option<String>,
    pub source_upload_date: Option<String>,
    pub language: String,
    pub transcript_path: Option<String>,
    pub transcript_kind: Option<String>,
    pub transcript_chars: Option<i64>,
    pub summary_text: Option<String>,
    pub audio_script: Option<String>,
    pub summary_edited_at: Option<i64>,
    pub review_required: bool,
    pub summary_hash: Option<String>,
    pub target_minutes: i64,
    pub stage: String,
    pub status: String,
    pub progress_json: Option<String>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub attempts: i64,
    pub next_attempt_at: Option<i64>,
    pub voice_id: Option<i64>,
    pub voice_design_id: Option<i64>,
    pub active_take_id: Option<i64>,
    pub take_ids_json: Option<String>,
    pub chunk_count: Option<i64>,
    pub chunk_done: i64,
    pub audio_version: i64,
    pub audio_path: Option<String>,
    pub audio_bytes: Option<i64>,
    pub audio_duration_s: Option<i64>,
    pub audio_summary_hash: Option<String>,
    pub published_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

pub fn progress_percent_for(
    stage: &str,
    chunk_done: i64,
    chunk_count: Option<i64>,
    progress_json: Option<&str>,
) -> u8 {
    match stage {
        stage::QUEUED => 0,
        stage::FETCHING => progress_json
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
            .and_then(|value| value.get("percent").and_then(|p| p.as_u64()))
            .map(|percent| percent.clamp(10, 35) as u8)
            .unwrap_or(10),
        stage::SUMMARIZING => 45,
        stage::VOICING => 55,
        stage::SYNTHESIZING => {
            let total = chunk_count.unwrap_or(0).max(0);
            if total == 0 {
                55
            } else {
                let done = chunk_done.clamp(0, total);
                (55 + (25 * done / total) as u8).min(80)
            }
        }
        stage::ASSEMBLING => 85,
        stage::REVIEW => 45,
        stage::PUBLISHING => 95,
        stage::READY | stage::PUBLISHED => 100,
        _ => 0,
    }
}

pub fn stage_label_for(stage: &str) -> &'static str {
    match stage {
        stage::QUEUED => "Đang chờ",
        stage::FETCHING => "Đang tải nguồn",
        stage::SUMMARIZING => "Đang tóm tắt",
        stage::VOICING => "Đang chuẩn bị giọng đọc",
        stage::SYNTHESIZING => "Đang tạo audio",
        stage::ASSEMBLING => "Đang ghép audio",
        stage::PUBLISHING => "Đang phát hành",
        stage::READY => "Sẵn sàng",
        stage::PUBLISHED => "Đã đăng",
        stage::REVIEW => "Chờ duyệt tóm tắt",
        _ => "Đang xử lý",
    }
}

pub fn status_label_for(status: &str) -> &'static str {
    match status {
        status::QUEUED => "Đang chờ",
        status::RUNNING => "Đang xử lý",
        status::READY => "Sẵn sàng",
        status::PUBLISHED => "Đã đăng",
        status::FAILED => "Lỗi",
        status::CANCELLED => "Đã huỷ",
        status::REVIEW => "Chờ duyệt",
        _ => "Không xác định",
    }
}

pub fn created_label_for(ts: i64) -> String {
    let seconds = crate::db::now_ts() - ts;
    if seconds < 60 {
        "vừa xong".into()
    } else if seconds < 3600 {
        format!("{} phút trước", seconds / 60)
    } else if seconds < 86_400 {
        format!("{} giờ trước", seconds / 3600)
    } else {
        format!("{} ngày trước", seconds / 86_400)
    }
}

pub fn error_label_for(code: Option<&str>) -> Option<&'static str> {
    match code? {
        "invalid_url" => Some("Link video không hợp lệ."),
        "source_unavailable" => Some("Không tải được video. Vui lòng thử lại sau."),
        "source_too_long" => Some("Video quá dài để xử lý. Liên hệ admin nếu cần hỗ trợ."),
        "source_too_short" => Some("Video quá ngắn để tạo podcast."),
        "no_captions" => Some("Video không có phụ đề."),
        "unsupported_language" => Some("Chỉ hỗ trợ tiếng Việt, Anh, Hàn."),
        "transcript_too_short" => Some("Nội dung quá ngắn để tóm tắt."),
        "transcript_invalid" => Some("Không đọc được phụ đề của video."),
        "voice_not_selected" => Some("Chưa chọn giọng đọc cho ngôn ngữ này. Liên hệ admin."),
        "voice_design_failed" | "tts_unavailable" => {
            Some("Dịch vụ đọc chưa sẵn sàng. Liên hệ admin.")
        }
        "synthesis_failed" => Some("Không tạo được audio. Vui lòng thử lại."),
        "assembly_failed" => Some("Không ghép được audio. Liên hệ admin."),
        "disk_full" => Some("Hết dung lượng lưu trữ. Liên hệ admin."),
        _ => Some("Không xử lý được tập này. Vui lòng liên hệ admin."),
    }
}

impl Episode {
    pub fn progress_percent(&self) -> u8 {
        progress_percent_for(
            &self.stage,
            self.chunk_done,
            self.chunk_count,
            self.progress_json.as_deref(),
        )
    }

    pub fn progress_detail(&self) -> Option<String> {
        let total = self.chunk_count?;
        if self.stage == stage::SYNTHESIZING && total > 0 {
            Some(format!(
                "Chunk {}/{}",
                self.chunk_done.clamp(0, total),
                total
            ))
        } else {
            None
        }
    }

    pub fn stage_label(&self) -> &'static str {
        stage_label_for(&self.stage)
    }

    pub fn status_label(&self) -> &'static str {
        status_label_for(&self.status)
    }

    pub fn created_label(&self) -> String {
        created_label_for(self.created_at)
    }

    pub fn error_label(&self) -> Option<&'static str> {
        error_label_for(self.error_code.as_deref())
    }

    pub fn is_active(&self) -> bool {
        matches!(self.status.as_str(), status::QUEUED | status::RUNNING)
    }

    pub fn is_terminal(&self) -> bool {
        matches!(
            self.status.as_str(),
            status::PUBLISHED | status::READY | status::FAILED | status::CANCELLED
        )
    }

    pub fn summary(&self) -> &str {
        self.summary_text.as_deref().unwrap_or("")
    }

    pub fn summary_words(&self) -> usize {
        self.summary_text
            .as_deref()
            .unwrap_or("")
            .split_whitespace()
            .count()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoicePreset {
    pub gender: String,
    pub age: String,
    pub pitch: String,
    pub style: String,
}

impl Default for VoicePreset {
    fn default() -> Self {
        Self {
            gender: "female".into(),
            age: "middle_aged".into(),
            pitch: "low".into(),
            style: "warm".into(),
        }
    }
}

pub fn supported_language(value: &str) -> Option<&'static str> {
    let base = value
        .split(['-', '_'])
        .next()
        .unwrap_or(value)
        .to_ascii_lowercase();
    match base.as_str() {
        "vi" => Some(LANG_VI),
        "en" => Some(LANG_EN),
        "ko" => Some(LANG_KO),
        _ => None,
    }
}

pub fn language_label(value: &str) -> &'static str {
    match supported_language(value) {
        Some("vi") => "Tiếng Việt",
        Some("en") => "English",
        Some("ko") => "한국어",
        _ => "Không xác định",
    }
}

pub fn voice_gender_label(value: Option<&str>) -> &'static str {
    match value {
        Some("female") => "Nữ",
        Some("male") => "Nam",
        _ => "Không rõ",
    }
}

pub fn voice_age_label(value: Option<&str>) -> &'static str {
    match value {
        Some("young") => "Trẻ",
        Some("middle_aged") => "Trung niên",
        Some("old") => "Lớn tuổi",
        _ => "Không rõ",
    }
}

pub fn rate_cps(language: &str) -> f64 {
    match supported_language(language).unwrap_or(LANG_VI) {
        LANG_VI => 17.4,
        LANG_EN => 15.0,
        LANG_KO => 7.0,
        _ => unreachable!(),
    }
}

pub fn target_chars(minutes: i64, language: &str) -> usize {
    (minutes.max(1) as f64 * rate_cps(language) * 60.0).round() as usize
}

pub fn sha256_hex(text: &str) -> String {
    let digest = Sha256::digest(text.as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_rates_and_target_length_are_stable() {
        assert_eq!(rate_cps("vi"), 17.4);
        assert_eq!(rate_cps("en"), 15.0);
        assert_eq!(rate_cps("ko"), 7.0);
        assert_eq!(target_chars(10, "vi"), 10_440);
        assert_eq!(supported_language("en-US"), Some("en"));
        assert_eq!(supported_language("ja"), None);
    }

    #[test]
    fn show_debug_redacts_the_feed_token() {
        let show = Show {
            id: 1,
            title: "Show".into(),
            author: "Author".into(),
            description: "Description".into(),
            category: "Technology".into(),
            owner_name: "Owner".into(),
            owner_email: "owner@example.com".into(),
            explicit: false,
            language: "vi".into(),
            website_url: "https://example.test".into(),
            cover_path: Some("/tmp/cover.jpg".into()),
            cover_version: 1,
            cover_custom: false,
            feed_token: "super-secret-token".into(),
            feed_enabled: true,
            default_minutes: 10,
            auto_publish: true,
            voice_preset_json: "{}".into(),
            created_at: 1,
            updated_at: 2,
        };
        let debug = format!("{show:?}");
        assert!(debug.contains("redacted"), "{debug}");
        assert!(!debug.contains("super-secret-token"), "{debug}");
    }

    #[test]
    fn summary_words_counts_words_in_the_display_summary() {
        let episode = Episode {
            uuid: "u1".into(),
            show_id: 1,
            video_id: "v1".into(),
            source_url: "https://example.test/v1".into(),
            source_title: "Video".into(),
            source_channel: "Channel".into(),
            source_duration_s: None,
            source_thumbnail_url: None,
            source_upload_date: None,
            language: "vi".into(),
            transcript_path: None,
            transcript_kind: None,
            transcript_chars: None,
            summary_text: Some("Một hai ba.".into()),
            audio_script: None,
            summary_edited_at: None,
            summary_hash: None,
            review_required: false,
            target_minutes: 10,
            stage: stage::READY.into(),
            status: status::READY.into(),
            progress_json: None,
            error_code: None,
            error_message: None,
            attempts: 0,
            next_attempt_at: None,
            voice_id: None,
            voice_design_id: None,
            active_take_id: None,
            take_ids_json: None,
            chunk_count: None,
            chunk_done: 0,
            audio_version: 1,
            audio_path: None,
            audio_bytes: None,
            audio_duration_s: None,
            audio_summary_hash: None,
            published_at: None,
            created_at: 1,
            updated_at: 1,
        };
        assert_eq!(episode.summary_words(), 3);
    }

    #[test]
    fn summary_hash_is_sha256_hex() {
        assert_eq!(
            sha256_hex("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn maps_stage_and_chunk_progress_to_percent() {
        assert_eq!(progress_percent_for("queued", 0, None, None), 0);
        assert_eq!(
            progress_percent_for("fetching", 0, None, Some(r#"{"percent":22}"#)),
            22
        );
        assert_eq!(progress_percent_for("summarizing", 0, None, None), 45);
        assert_eq!(progress_percent_for("voicing", 0, None, None), 55);
        assert_eq!(progress_percent_for("synthesizing", 2, Some(4), None), 67);
        assert_eq!(progress_percent_for("synthesizing", 4, Some(4), None), 80);
        assert_eq!(progress_percent_for("assembling", 0, None, None), 85);
        assert_eq!(progress_percent_for("review", 0, None, None), 45);
        assert_eq!(progress_percent_for("publishing", 0, None, None), 95);
        assert_eq!(progress_percent_for("published", 0, None, None), 100);
        assert_eq!(progress_percent_for("ready", 0, None, None), 100);
    }

    #[test]
    fn relative_created_label_is_human_readable() {
        let now = chrono::Utc::now().timestamp();
        assert_eq!(created_label_for(now - 30), "vừa xong");
        assert_eq!(created_label_for(now - 120), "2 phút trước");
        assert_eq!(created_label_for(now - 7200), "2 giờ trước");
    }

    #[test]
    fn labels_and_errors_are_user_facing() {
        assert_eq!(stage_label_for("synthesizing"), "Đang tạo audio");
        assert_eq!(stage_label_for("review"), "Chờ duyệt tóm tắt");
        assert_eq!(status_label_for("review"), "Chờ duyệt");
        assert_eq!(status_label_for("ready"), "Sẵn sàng");
        assert_eq!(status_label_for("cancelled"), "Đã huỷ");
        assert_eq!(
            error_label_for(Some("voice_not_selected")),
            Some("Chưa chọn giọng đọc cho ngôn ngữ này. Liên hệ admin.")
        );
        assert_eq!(
            error_label_for(Some("internal")),
            Some("Không xử lý được tập này. Vui lòng liên hệ admin.")
        );
        assert_eq!(error_label_for(None), None);
    }

    #[test]
    fn language_voice_gender_and_age_labels_are_user_facing() {
        assert_eq!(language_label("vi"), "Tiếng Việt");
        assert_eq!(language_label("en-US"), "English");
        assert_eq!(language_label("ko"), "한국어");
        assert_eq!(language_label("ja"), "Không xác định");

        assert_eq!(voice_gender_label(Some("female")), "Nữ");
        assert_eq!(voice_gender_label(Some("male")), "Nam");
        assert_eq!(voice_gender_label(None), "Không rõ");

        assert_eq!(voice_age_label(Some("young")), "Trẻ");
        assert_eq!(voice_age_label(Some("middle_aged")), "Trung niên");
        assert_eq!(voice_age_label(Some("old")), "Lớn tuổi");
        assert_eq!(voice_age_label(Some("unknown")), "Không rõ");
    }
}
