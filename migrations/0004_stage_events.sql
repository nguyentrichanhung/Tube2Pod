CREATE TABLE stage_events (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  episode_uuid TEXT NOT NULL REFERENCES episodes(uuid) ON DELETE CASCADE,
  stage TEXT NOT NULL,
  entered_at INTEGER NOT NULL,
  exited_at INTEGER,
  outcome TEXT CHECK (outcome IN ('ok', 'failed', 'cancelled', 'interrupted')),
  error_code TEXT
);

CREATE INDEX idx_stage_events_entered ON stage_events(entered_at);
CREATE INDEX idx_stage_events_episode ON stage_events(episode_uuid);
CREATE INDEX idx_stage_events_stage ON stage_events(stage, entered_at);
CREATE INDEX idx_stage_events_outcome ON stage_events(outcome, entered_at);
