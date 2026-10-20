PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS shows (
  id INTEGER PRIMARY KEY,
  title TEXT NOT NULL,
  author TEXT NOT NULL DEFAULT '',
  description TEXT NOT NULL DEFAULT '',
  category TEXT NOT NULL DEFAULT 'Technology',
  owner_name TEXT NOT NULL DEFAULT '',
  owner_email TEXT NOT NULL DEFAULT '',
  explicit INTEGER NOT NULL DEFAULT 0,
  language TEXT NOT NULL DEFAULT 'vi' CHECK (language IN ('vi','en','ko')),
  website_url TEXT NOT NULL DEFAULT '',
  cover_path TEXT,
  cover_version INTEGER NOT NULL DEFAULT 1,
  cover_custom INTEGER NOT NULL DEFAULT 0,
  feed_token TEXT NOT NULL UNIQUE,
  feed_enabled INTEGER NOT NULL DEFAULT 0,
  default_minutes INTEGER NOT NULL DEFAULT 10,
  auto_publish INTEGER NOT NULL DEFAULT 1,
  voice_preset_json TEXT NOT NULL DEFAULT '{"gender":"female","age":"middle_aged","pitch":"low","style":"warm"}',
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS show_voices (
  show_id INTEGER NOT NULL REFERENCES shows(id) ON DELETE CASCADE,
  language TEXT NOT NULL,
  omnivoice_voice_id INTEGER NOT NULL,
  voice_name TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  PRIMARY KEY (show_id, language)
);

CREATE TABLE IF NOT EXISTS episodes (
  uuid TEXT PRIMARY KEY,
  show_id INTEGER NOT NULL REFERENCES shows(id) ON DELETE CASCADE,
  video_id TEXT NOT NULL UNIQUE,
  source_url TEXT NOT NULL,
  source_title TEXT NOT NULL,
  source_channel TEXT NOT NULL DEFAULT '',
  source_duration_s INTEGER,
  source_thumbnail_url TEXT,
  source_upload_date TEXT,
  language TEXT NOT NULL,
  transcript_path TEXT,
  transcript_kind TEXT,
  transcript_chars INTEGER,
  summary_text TEXT,
  summary_edited_at INTEGER,
  summary_hash TEXT,
  target_minutes INTEGER NOT NULL,
  stage TEXT NOT NULL,
  status TEXT NOT NULL,
  progress_json TEXT,
  error_code TEXT,
  error_message TEXT,
  attempts INTEGER NOT NULL DEFAULT 0,
  next_attempt_at INTEGER,
  voice_id INTEGER,
  voice_design_id INTEGER,
  active_take_id INTEGER,
  take_ids_json TEXT,
  chunk_count INTEGER,
  chunk_done INTEGER NOT NULL DEFAULT 0,
  audio_version INTEGER NOT NULL DEFAULT 1,
  audio_path TEXT,
  audio_bytes INTEGER,
  audio_duration_s INTEGER,
  audio_summary_hash TEXT,
  published_at INTEGER,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS episodes_queue ON episodes(status, next_attempt_at, created_at);
CREATE INDEX IF NOT EXISTS episodes_show ON episodes(show_id, published_at DESC);

CREATE TABLE IF NOT EXISTS settings (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL,
  updated_at INTEGER NOT NULL
);
