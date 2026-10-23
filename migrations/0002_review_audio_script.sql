ALTER TABLE episodes ADD COLUMN audio_script TEXT;
ALTER TABLE episodes ADD COLUMN review_required INTEGER NOT NULL DEFAULT 0;
UPDATE episodes SET audio_script = summary_text WHERE audio_script IS NULL;
