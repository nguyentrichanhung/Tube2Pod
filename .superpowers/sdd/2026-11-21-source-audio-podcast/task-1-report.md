# Task 1 report

Implemented `audio_tracks` and metadata decoding while retaining the caption API. Grouping uses an audio track ID if provided, otherwise language plus the track display name from `format_note` with quality/codec details removed. Default comes first; otherwise tracks follow the preferred representative format in descending extractor order. Audio-only is preferred to a muxed format for the same track.

Verification (CARGO_TARGET_DIR=/home/workspace/Tube2Pod/target):
- Before implementation, `cargo test --locked services::ytdlp::tests::` failed to compile because the adapter and formats did not exist.
- After implementation, the focused suite passed: 7 passed.
- `cargo test --locked` passed: 335 passed. The compiler warns that the newly decoded description is not yet read by this task.
- Reviewed the installed yt-dlp YouTube extractor and safe fields from the authorized single-track probe and multitrack tutorial probe. A test includes only safe fields from the latter. The same-language alternatives and a default differing from original are synthetic examples matching the extractor's output shape.

Limits: installed yt-dlp does not expose the YouTube audioTrack ID in the observed output. Tracks with identical language and identical display name cannot be distinguished from those fields; an unlabelled track without an explicit default role is excluded when multiple track groups exist. No actual multitrack sample with same-language alternatives or default distinct from original was available. No media was downloaded or live service changed.

## Review fixes

Later extractor formats now win within a track and rank tracks, while explicit defaults remain first. A default without language and a stable distinct descriptor without language are retained. Both orders of the default/original suffixes group consistently. A sole unlabeled track remains available; where it appears beside identified groups without an explicit role, mapping remains ambiguous and it is excluded.

Before the fix, the three preference/missing-language regressions failed, as did the separate marker-order regression. After the fix, `CARGO_TARGET_DIR=/home/workspace/Tube2Pod/target cargo test --locked services::ytdlp::tests::` passed: 10 passed, 0 failed, 328 filtered out. The description dead-code warning remains. The full suite was not rerun in this review fix round.
