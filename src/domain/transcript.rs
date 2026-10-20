use crate::error::{AppError, AppResult};
use serde_json::Value;
use std::sync::OnceLock;

const GAP_THRESHOLD_MS: i64 = 800;
const MIN_SENTENCE_CHARS: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cue {
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
}

pub fn parse_by_kind(raw: &str, kind: &str) -> AppResult<Vec<Cue>> {
    match kind {
        "json3" => parse_json3(raw),
        "vtt" => parse_vtt(raw),
        other => Err(AppError::BadRequest(format!(
            "unsupported transcript kind: {other}"
        ))),
    }
}

pub fn parse_json3(raw: &str) -> AppResult<Vec<Cue>> {
    let value: Value = serde_json::from_str(raw)?;
    let events = value
        .get("events")
        .and_then(Value::as_array)
        .ok_or_else(|| AppError::BadRequest("invalid json3 transcript".into()))?;
    let mut cues = Vec::new();
    for event in events {
        if is_append(event) {
            continue;
        }
        let text = event
            .get("segs")
            .and_then(Value::as_array)
            .map(|segs| {
                segs.iter()
                    .filter(|seg| !is_append(seg))
                    .filter_map(|seg| seg.get("utf8").and_then(Value::as_str))
                    .collect::<Vec<_>>()
                    .join("")
            })
            .unwrap_or_default();
        let text = clean_text(&text);
        if text.is_empty() {
            continue;
        }
        let start_ms = event.get("tStartMs").and_then(Value::as_i64).unwrap_or(0);
        let end_ms = start_ms
            + event
                .get("dDurationMs")
                .and_then(Value::as_i64)
                .unwrap_or(0);
        cues.push(Cue {
            start_ms,
            end_ms,
            text,
        });
    }
    Ok(cues)
}

fn is_append(value: &Value) -> bool {
    value
        .get("aAppend")
        .is_some_and(|flag| flag.as_bool() == Some(true) || flag.as_i64() == Some(1))
}

pub fn parse_vtt(raw: &str) -> AppResult<Vec<Cue>> {
    let mut cues = Vec::new();
    let mut lines = raw.lines().peekable();
    while let Some(line) = lines.next() {
        let line = line.trim_start_matches('\u{feff}').trim();
        if line.is_empty() || line.starts_with("WEBVTT") {
            continue;
        }
        if line.starts_with("NOTE") {
            while let Some(next) = lines.peek() {
                if next.trim().is_empty() {
                    lines.next();
                    break;
                }
                lines.next();
            }
            continue;
        }
        let Some((start, end)) = parse_timestamp_line(line) else {
            continue;
        };
        let mut parts = Vec::new();
        while let Some(next) = lines.peek() {
            let next = next.trim();
            if next.is_empty() {
                lines.next();
                break;
            }
            if parse_timestamp_line(next).is_some() {
                break;
            }
            let next = lines.next().unwrap().trim();
            let text = strip_vtt_tags(next);
            let text = text.trim();
            if !text.is_empty() {
                parts.push(text.to_string());
            }
        }
        let text = clean_text(&parts.join(" "));
        if text.is_empty() {
            continue;
        }
        if cues
            .last()
            .is_some_and(|previous: &Cue| previous.text == text)
        {
            continue;
        }
        cues.push(Cue {
            start_ms: start,
            end_ms: end,
            text,
        });
    }
    Ok(cues)
}

fn parse_timestamp_line(line: &str) -> Option<(i64, i64)> {
    let (start, rest) = line.split_once("-->")?;
    let end = rest.split_whitespace().next()?;
    Some((parse_vtt_time(start.trim())?, parse_vtt_time(end)?))
}

fn parse_vtt_time(value: &str) -> Option<i64> {
    let parts: Vec<&str> = value.split(':').collect();
    let (hours, minutes, seconds) = match parts.as_slice() {
        [h, m, s] => (h.parse::<i64>().ok()?, m.parse::<i64>().ok()?, *s),
        [m, s] => (0, m.parse::<i64>().ok()?, *s),
        _ => return None,
    };
    let (secs, millis) = seconds
        .split_once('.')
        .or_else(|| seconds.split_once(','))
        .unwrap_or((seconds, "0"));
    let millis = parse_fraction_ms(millis)?;
    Some(((hours * 3600 + minutes * 60 + secs.parse::<i64>().ok()?) * 1000) + millis)
}

fn parse_fraction_ms(value: &str) -> Option<i64> {
    if value.is_empty() {
        return Some(0);
    }
    let mut digits = String::new();
    for ch in value.chars().take(3) {
        if !ch.is_ascii_digit() {
            return None;
        }
        digits.push(ch);
    }
    while digits.len() < 3 {
        digits.push('0');
    }
    digits.parse::<i64>().ok()
}

fn strip_vtt_tags(line: &str) -> String {
    let mut out = String::new();
    let mut inside = false;
    for ch in line.chars() {
        match ch {
            '<' => inside = true,
            '>' => inside = false,
            _ if !inside => out.push(ch),
            _ => {}
        }
    }
    out
}

pub fn clean_text(text: &str) -> String {
    let mut out = String::new();
    let mut depth = 0usize;
    for ch in text.chars() {
        match ch {
            '[' => depth += 1,
            ']' if depth > 0 => depth -= 1,
            '\u{200b}' | '\u{200c}' | '\u{200d}' | '\u{feff}' => {}
            _ if depth == 0 => out.push(ch),
            _ => {}
        }
    }
    let normalized = out
        .replace(['\u{2018}', '\u{2019}'], "'")
        .replace(['\u{201c}', '\u{201d}'], "\"")
        .replace(['\u{2013}', '\u{2014}'], "-");
    normalized.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn segment(cues: &[Cue], language: &str) -> Vec<String> {
    let mut sentences: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut previous_end = 0i64;
    for cue in cues {
        let gap = cue.start_ms - previous_end;
        previous_end = cue.end_ms;
        let text = clean_text(&cue.text);
        if text.is_empty() {
            continue;
        }
        if !current.is_empty() && gap >= GAP_THRESHOLD_MS {
            push_sentence(&mut sentences, &mut current);
        }
        for part in split_punctuation(&text) {
            if current.is_empty() {
                current = capitalize_first(&part, language);
            } else if ends_sentence(&current) {
                push_sentence(&mut sentences, &mut current);
                current = capitalize_first(&part, language);
            } else {
                current.push(' ');
                current.push_str(&part);
            }
        }
    }
    push_sentence(&mut sentences, &mut current);
    sentences
}

fn split_punctuation(text: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        current.push(ch);
        if matches!(ch, '.' | '!' | '?' | '…') {
            let next_is_boundary = chars.peek().is_none_or(|next| next.is_whitespace());
            let abbreviation = ch == '.' && is_abbreviation(&current);
            if next_is_boundary && !abbreviation {
                let part = current.trim();
                if !part.is_empty() {
                    parts.push(part.to_string());
                }
                current.clear();
                while chars.peek().is_some_and(|next| next.is_whitespace()) {
                    chars.next();
                }
            }
        }
    }
    let tail = current.trim();
    if !tail.is_empty() {
        parts.push(tail.to_string());
    }
    if parts.is_empty() {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            Vec::new()
        } else {
            vec![trimmed.to_string()]
        }
    } else {
        parts
    }
}

fn is_abbreviation(current: &str) -> bool {
    const ABBREVIATIONS: &[&str] = &[
        "mr.", "mrs.", "ms.", "dr.", "prof.", "sr.", "jr.", "st.", "vs.", "etc.", "e.g.", "i.e.",
        "tp.", "ths.", "ts.", "p.", "v.",
    ];
    let token = current
        .split_whitespace()
        .last()
        .unwrap_or(current)
        .trim_start_matches(['"', '\'', '(', '[', '“', '‘']);
    let lower = token.to_ascii_lowercase();
    if ABBREVIATIONS
        .iter()
        .any(|abbreviation| lower.ends_with(abbreviation))
    {
        return true;
    }
    let core = token.trim_end_matches('.');
    core.contains('.')
        && core.chars().all(|ch| ch.is_ascii_uppercase() || ch == '.')
        && core.chars().filter(|ch| ch.is_ascii_uppercase()).count() >= 2
}

fn ends_sentence(text: &str) -> bool {
    text.trim_end()
        .trim_end_matches(['"', '\'', '”', '’', ')', ']', '}'])
        .ends_with(['.', '!', '?', '…'])
}

fn push_sentence(sentences: &mut Vec<String>, current: &mut String) {
    let value = current.trim();
    if value.chars().filter(|c| c.is_alphanumeric()).count() >= MIN_SENTENCE_CHARS {
        sentences.push(value.to_string());
    }
    current.clear();
}

fn capitalize_first(text: &str, language: &str) -> String {
    if language == "ko" {
        return text.to_string();
    }
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

pub fn stopwords(language: &str) -> &'static [&'static str] {
    static VI: OnceLock<Vec<&'static str>> = OnceLock::new();
    static EN: OnceLock<Vec<&'static str>> = OnceLock::new();
    static KO: OnceLock<Vec<&'static str>> = OnceLock::new();
    match language {
        "vi" => VI.get_or_init(|| {
            "và của là có trong một những các cho với để không được này đó thì mà như khi từ về bạn chúng ta mình ".split_whitespace().collect()
        }),
        "en" => EN.get_or_init(|| {
            "the a an and or of to in on for with is are was were be been this that these those it its as at by from we you they i he she ".split_whitespace().collect()
        }),
        _ => KO.get_or_init(|| {
            "그리고 하지만 그래서 이것 저것 우리 당신 그 저 것 수 있다 하다 위한 대한 에서 으로 로 ".split_whitespace().collect()
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json3_drops_duplicate_append_events() {
        let cues = parse_json3(include_str!("../../tests/fixtures/captions_vi.json3")).unwrap();
        assert_eq!(cues.len(), 3);
        assert_eq!(cues[0].text, "Xin chào các bạn.");
        assert_eq!(cues[1].text, "Hôm nay chúng ta nói về Rust.");
    }

    #[test]
    fn vtt_strips_tags_and_deduplicates_rolling_captions() {
        let cues = parse_vtt(include_str!("../../tests/fixtures/captions_en.vtt")).unwrap();
        assert_eq!(cues[0].text, "Welcome back.");
        assert_eq!(cues[1].text, "Today we build a podcast.");
        assert_eq!(cues.len(), 2);
    }

    #[test]
    fn segments_on_punctuation_and_short_gaps() {
        let cues = vec![
            Cue {
                start_ms: 0,
                end_ms: 2000,
                text: "Xin chào các bạn.".into(),
            },
            Cue {
                start_ms: 2300,
                end_ms: 4000,
                text: "Hôm nay chúng ta nói về Rust".into(),
            },
            Cue {
                start_ms: 4500,
                end_ms: 7000,
                text: "và cách viết ứng dụng an toàn.".into(),
            },
        ];
        let sentences = segment(&cues, "vi");
        assert_eq!(sentences.len(), 2);
        assert_eq!(sentences[0], "Xin chào các bạn.");
        assert_eq!(
            sentences[1],
            "Hôm nay chúng ta nói về Rust và cách viết ứng dụng an toàn."
        );
    }

    #[test]
    fn long_gap_starts_a_new_segment() {
        let cues = vec![
            Cue {
                start_ms: 0,
                end_ms: 2000,
                text: "Xin chào các bạn.".into(),
            },
            Cue {
                start_ms: 2300,
                end_ms: 4000,
                text: "Hôm nay chúng ta nói về Rust".into(),
            },
            Cue {
                start_ms: 5100,
                end_ms: 7000,
                text: "và cách viết ứng dụng an toàn.".into(),
            },
        ];
        let sentences = segment(&cues, "vi");
        assert_eq!(sentences.len(), 3);
        assert_eq!(sentences[1], "Hôm nay chúng ta nói về Rust");
    }

    #[test]
    fn clean_text_removes_bracketed_sounds_and_zero_width() {
        assert_eq!(
            clean_text("[Music]  Xin   chào\u{200b} [Applause] bạn. "),
            "Xin chào bạn."
        );
    }
}
