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
}

pub mod status {
    pub const QUEUED: &str = "queued";
    pub const RUNNING: &str = "running";
    pub const READY: &str = "ready";
    pub const PUBLISHED: &str = "published";
    pub const FAILED: &str = "failed";
    pub const CANCELLED: &str = "cancelled";
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
    pub summary_edited_at: Option<i64>,
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

impl Episode {
    pub fn is_terminal(&self) -> bool {
        matches!(
            self.status.as_str(),
            status::PUBLISHED | status::READY | status::FAILED | status::CANCELLED
        )
    }

    pub fn summary(&self) -> &str {
        self.summary_text.as_deref().unwrap_or("")
    }

    pub fn audio_needs_reread(&self) -> bool {
        match (&self.summary_hash, &self.audio_summary_hash) {
            (Some(summary), Some(audio)) => summary != audio,
            (Some(_), None) => true,
            _ => false,
        }
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
    fn summary_hash_is_sha256_hex() {
        assert_eq!(
            sha256_hex("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
