use crate::domain::transcript::stopwords;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone)]
pub struct SummaryOptions {
    pub target_chars: usize,
    pub title: String,
}

#[derive(Debug, Clone)]
pub struct SummaryResult {
    pub selected_indices: Vec<usize>,
    pub script: String,
    pub source_text: String,
    pub short_source: bool,
}

pub fn summarize(sentences: &[String], options: &SummaryOptions) -> SummaryResult {
    let usable: Vec<(usize, &String)> = sentences
        .iter()
        .enumerate()
        .filter(|(_, sentence)| sentence.chars().filter(|c| c.is_alphanumeric()).count() >= 8)
        .collect();
    if usable.is_empty() {
        return SummaryResult {
            selected_indices: Vec::new(),
            script: String::new(),
            source_text: sentences.join("\n"),
            short_source: true,
        };
    }

    let docs: Vec<HashSet<String>> = usable
        .iter()
        .map(|(_, sentence)| tokens(sentence))
        .collect();
    let mut df: HashMap<String, usize> = HashMap::new();
    for doc in &docs {
        for token in doc {
            *df.entry(token.clone()).or_default() += 1;
        }
    }
    let n = docs.len() as f64;
    let title_tokens = tokens(&options.title);
    let mut scored: Vec<(f64, usize)> = usable
        .iter()
        .enumerate()
        .map(|(i, (original, sentence))| {
            let doc = &docs[i];
            let mut score = 0.0;
            for token in doc {
                let tf =
                    doc.iter().filter(|t| *t == token).count() as f64 / doc.len().max(1) as f64;
                let idf = ((n + 1.0) / (*df.get(token).unwrap_or(&0) as f64 + 1.0)).ln() + 1.0;
                score += tf * idf;
            }
            if title_tokens.iter().any(|token| doc.contains(token)) {
                score *= 1.15;
            }
            let position = *original as f64 / sentences.len().max(1) as f64;
            if position < 0.15 || position > 0.85 {
                score *= 1.03;
            }
            if sentence.chars().count() < 30 {
                score *= 0.8;
            }
            (score, *original)
        })
        .collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.cmp(&b.1)));

    let mut selected: Vec<usize> = Vec::new();
    let mut used_chars = 0usize;
    let mut candidates = scored;
    while !candidates.is_empty() {
        let mut best: Option<(usize, f64)> = None;
        for (position, (relevance, index)) in candidates.iter().enumerate() {
            let doc = &docs[usable.iter().position(|(i, _)| i == index).unwrap()];
            let similarity = selected
                .iter()
                .map(|chosen| {
                    let chosen_doc = &docs[usable.iter().position(|(i, _)| i == chosen).unwrap()];
                    cosine(doc, chosen_doc)
                })
                .fold(0.0f64, f64::max);
            let score = 0.7 * relevance - 0.3 * similarity;
            if best.is_none_or(|(_, best_score)| score > best_score) {
                best = Some((position, score));
            }
        }
        let (position, _) = best.unwrap();
        let (_, index) = candidates.remove(position);
        let sentence = &sentences[index];
        if !selected.is_empty() && used_chars + sentence.len() > options.target_chars {
            break;
        }
        used_chars += sentence.len();
        selected.push(index);
        if used_chars >= options.target_chars {
            break;
        }
    }
    selected.sort_unstable();

    let source_text = sentences.join("\n");
    let script = selected
        .iter()
        .map(|index| sentences[*index].clone())
        .collect::<Vec<_>>()
        .join("\n\n");
    let short_source = source_text.len() < options.target_chars;
    SummaryResult {
        selected_indices: selected,
        script,
        source_text,
        short_source,
    }
}

fn tokens(text: &str) -> HashSet<String> {
    let stop = stopwords(detect_language(text));
    let mut out = HashSet::new();
    for word in text.split(|c: char| !c.is_alphanumeric() && c != '\'' && c != '-') {
        let word = word.trim().to_lowercase();
        if word.len() < 2 || stop.contains(&word.as_str()) {
            continue;
        }
        out.insert(word);
    }
    out
}

fn cosine(a: &HashSet<String>, b: &HashSet<String>) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let intersection = a.intersection(b).count() as f64;
    intersection / ((a.len() as f64).sqrt() * (b.len() as f64).sqrt())
}

fn detect_language(text: &str) -> &'static str {
    if text.chars().any(|c| ('\u{ac00}'..='\u{d7a3}').contains(&c)) {
        "ko"
    } else if text.chars().any(|c| ('\u{00c0}'..='\u{1ef9}').contains(&c)) {
        "vi"
    } else {
        "en"
    }
}

pub fn build_script(
    show_title: &str,
    video_title: &str,
    _sentences: &[String],
    body: &str,
    language: &str,
) -> String {
    match language {
        "vi" => format!(
            "Chào mừng bạn đến với {show_title}. Trong tập này, chúng ta cùng điểm qua những ý chính từ video: {video_title}.\n\n{body}\n\nĐó là những ý chính. Cảm ơn bạn đã lắng nghe."
        ),
        "en" => format!(
            "Welcome to {show_title}. In this episode, we look at the key ideas from the video: {video_title}.\n\n{body}\n\nThose are the main points. Thanks for listening."
        ),
        _ => format!(
            "{show_title}에 오신 것을 환영합니다. 이번 에피소드에서는 {video_title} 영상의 핵심 내용을 살펴봅니다.\n\n{body}\n\n이상으로 핵심 내용을 마칩니다. 들어주셔서 감사합니다."
        ),
    }
}

pub fn sanitize_for_tts(text: &str) -> String {
    let mut out = String::new();
    for line in text.lines() {
        let mut line = line.trim().to_string();
        if line.starts_with('#') || line.starts_with('>') {
            continue;
        }
        line = line.replace("**", "").replace("__", "").replace('`', "");
        while let Some(start) = line.find("http://").or_else(|| line.find("https://")) {
            let end = line[start..]
                .find(char::is_whitespace)
                .map(|i| start + i)
                .unwrap_or(line.len());
            line.replace_range(start..end, "");
        }
        line = remove_bracketed(&line);
        line = line
            .split_whitespace()
            .filter(|word| !word.starts_with('#'))
            .collect::<Vec<_>>()
            .join(" ");
        line = line
            .chars()
            .filter(|ch| {
                !ch.is_control()
                    && !matches!(
                        *ch,
                        '😀' | '😁' | '😂' | '🤣' | '😊' | '👍' | '🔥' | '🎧' | '🎙'
                    )
            })
            .collect();
        if let Some((prefix, rest)) = line.split_once(':') {
            if prefix.split_whitespace().count() <= 3
                && prefix
                    .chars()
                    .all(|c| c.is_alphanumeric() || c.is_whitespace())
            {
                line = rest.trim().to_string();
            }
        }
        let line = line.trim();
        if !line.is_empty() {
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(line);
        }
    }
    out.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace(" .", ".")
        .replace(" ,", ",")
        .replace(" !", "!")
        .replace(" ?", "?")
}

fn remove_bracketed(text: &str) -> String {
    let mut out = String::new();
    let mut depth = 0usize;
    for ch in text.chars() {
        match ch {
            '[' => depth += 1,
            ']' if depth > 0 => depth -= 1,
            _ if depth == 0 => out.push(ch),
            _ => {}
        }
    }
    out
}

pub fn split_chunks(text: &str, limit: usize) -> Vec<String> {
    let limit = limit.max(1);
    let mut chunks = Vec::new();
    let mut current = String::new();
    for sentence in split_sentences(text) {
        if sentence.len() <= limit {
            if !current.is_empty() && current.len() + 1 + sentence.len() > limit {
                chunks.push(std::mem::take(&mut current));
            }
            if !current.is_empty() {
                current.push(' ');
            }
            current.push_str(&sentence);
        } else {
            if !current.is_empty() {
                chunks.push(std::mem::take(&mut current));
            }
            chunks.extend(split_long_sentence(&sentence, limit));
        }
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

fn split_sentences(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    for ch in text.chars() {
        current.push(ch);
        if matches!(ch, '.' | '!' | '?' | '…') {
            let value = current.trim();
            if !value.is_empty() {
                out.push(value.to_string());
            }
            current.clear();
        }
    }
    let value = current.trim();
    if !value.is_empty() {
        out.push(value.to_string());
    }
    out
}

fn split_long_sentence(sentence: &str, limit: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    for word in sentence.split_whitespace() {
        if !current.is_empty() && current.len() + 1 + word.len() > limit {
            out.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sentences() -> Vec<String> {
        vec![
            "Rust là ngôn ngữ lập trình an toàn và nhanh.".into(),
            "Bài nói hôm nay giải thích cách trình biên dịch kiểm tra bộ nhớ.".into(),
            "Chúng ta sẽ xem ví dụ về ownership và borrowing.".into(),
            "Ownership giúp tránh lỗi con trỏ null và data race.".into(),
            "Cuối cùng, chúng ta thảo luận về hiệu năng và công cụ.".into(),
        ]
    }

    #[test]
    fn summary_is_deterministic_and_preserves_source_order() {
        let first = summarize(
            &sentences(),
            &SummaryOptions {
                target_chars: 260,
                title: "Rust".into(),
            },
        );
        let second = summarize(
            &sentences(),
            &SummaryOptions {
                target_chars: 260,
                title: "Rust".into(),
            },
        );
        assert_eq!(first.script, second.script);
        assert!(first
            .selected_indices
            .windows(2)
            .all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn short_transcript_uses_every_sentence() {
        let result = summarize(
            &["Một câu ngắn.".into(), "Một câu khác.".into()],
            &SummaryOptions {
                target_chars: 5000,
                title: "Ngắn".into(),
            },
        );
        assert_eq!(result.selected_indices.len(), 2);
        assert!(result.short_source);
    }

    #[test]
    fn sanitizer_removes_markdown_urls_emojis_and_speaker_labels() {
        let input = "**Xin chào** https://example.com #tag 😀\nNgười nói: Nội dung [Music].";
        assert_eq!(sanitize_for_tts(input), "Xin chào Nội dung.");
    }

    #[test]
    fn chunker_respects_limit_and_sentence_boundaries() {
        let text = "Câu một. Câu hai. Câu ba! Câu bốn?";
        let chunks = split_chunks(text, 12);
        assert!(chunks.iter().all(|chunk| chunk.chars().count() <= 12));
        assert_eq!(chunks, vec!["Câu một.", "Câu hai.", "Câu ba!", "Câu bốn?"]);
    }
}
