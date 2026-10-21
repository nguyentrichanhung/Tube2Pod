use crate::domain::transcript::{split_punctuation, stopwords};
use std::collections::{BTreeSet, HashMap};

const INTRO_OUTRO_RESERVE_CHARS: usize = 256;

#[derive(Debug, Clone)]
pub struct SummaryOptions {
    pub target_chars: usize,
    pub title: String,
}

#[derive(Debug, Clone)]
pub struct SummaryResult {
    pub selected_indices: Vec<usize>,
}

pub fn summarize(sentences: &[String], options: &SummaryOptions) -> SummaryResult {
    let char_counts: Vec<usize> = sentences
        .iter()
        .map(|sentence| sentence.chars().count())
        .collect();
    let usable: Vec<(usize, &String)> = sentences
        .iter()
        .enumerate()
        .filter(|(_, sentence)| sentence.chars().filter(|c| c.is_alphanumeric()).count() >= 8)
        .collect();
    if usable.is_empty() {
        return SummaryResult {
            selected_indices: Vec::new(),
        };
    }

    let usable_index: HashMap<usize, usize> = usable
        .iter()
        .enumerate()
        .map(|(position, (original, _))| (*original, position))
        .collect();
    let docs: Vec<BTreeSet<String>> = usable
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
        .map(|(i, (original, _sentence))| {
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
            if char_counts[*original] < 30 {
                score *= 0.8;
            }
            (score, *original)
        })
        .collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.cmp(&b.1)));

    let budget = options
        .target_chars
        .saturating_sub(INTRO_OUTRO_RESERVE_CHARS);
    let mut selected: Vec<usize> = Vec::new();
    let mut used_chars = 0usize;
    let mut candidates = scored;
    let mut similarities = vec![0.0f64; candidates.len()];
    while !candidates.is_empty() {
        let mut best: Option<(usize, f64)> = None;
        for (position, (relevance, _)) in candidates.iter().enumerate() {
            let score = 0.7 * relevance - 0.3 * similarities[position];
            if best.is_none_or(|(_, best_score)| score > best_score) {
                best = Some((position, score));
            }
        }
        let (position, _) = best.unwrap();
        let (_, index) = candidates.remove(position);
        similarities.remove(position);
        let sentence_chars = char_counts[index];
        if !selected.is_empty() && used_chars + sentence_chars > budget {
            break;
        }
        used_chars += sentence_chars;
        selected.push(index);
        let chosen_doc = &docs[usable_index[&index]];
        for (position, (_, candidate_index)) in candidates.iter().enumerate() {
            let similarity = cosine(&docs[usable_index[candidate_index]], chosen_doc);
            if similarity > similarities[position] {
                similarities[position] = similarity;
            }
        }
        if used_chars >= budget {
            break;
        }
    }
    selected.sort_unstable();

    SummaryResult {
        selected_indices: selected,
    }
}

fn tokens(text: &str) -> BTreeSet<String> {
    let stop = stopwords(detect_language(text));
    let mut out = BTreeSet::new();
    for word in text.split(|c: char| !c.is_alphanumeric() && c != '\'' && c != '-') {
        let word = word.trim().to_lowercase();
        if word.len() < 2 || stop.contains(&word.as_str()) {
            continue;
        }
        out.insert(word);
    }
    out
}

fn cosine(a: &BTreeSet<String>, b: &BTreeSet<String>) -> f64 {
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
    let mut paragraphs: Vec<String> = Vec::new();
    let mut current: Vec<String> = Vec::new();
    for raw in text.split('\n') {
        let line = sanitize_line(raw);
        if line.is_empty() {
            if !current.is_empty() {
                paragraphs.push(current.join(" "));
                current.clear();
            }
        } else {
            current.push(line);
        }
    }
    if !current.is_empty() {
        paragraphs.push(current.join(" "));
    }
    paragraphs
        .iter()
        .map(|paragraph| {
            paragraph
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .replace(" .", ".")
                .replace(" ,", ",")
                .replace(" !", "!")
                .replace(" ?", "?")
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn sanitize_line(raw: &str) -> String {
    let mut line = raw.trim().to_string();
    if line.starts_with('#') || line.starts_with('>') {
        return String::new();
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
        .chars()
        .filter(|ch| !ch.is_control() && !is_zero_width(*ch) && !is_emoji(*ch))
        .collect();
    line = line
        .split_whitespace()
        .filter(|word| !word.starts_with('#'))
        .collect::<Vec<_>>()
        .join(" ");
    if let Some((prefix, rest)) = line.split_once(':') {
        if prefix.split_whitespace().count() <= 3
            && prefix
                .chars()
                .all(|c| c.is_alphanumeric() || c.is_whitespace())
        {
            line = rest.trim().to_string();
        }
    }
    line.trim().to_string()
}

fn is_zero_width(ch: char) -> bool {
    matches!(
        ch as u32,
        0x00AD
            | 0x034F
            | 0x0600..=0x0605
            | 0x061C
            | 0x06DD
            | 0x070F
            | 0x0890..=0x0891
            | 0x08E2
            | 0x17B4..=0x17B5
            | 0x180E
            | 0x200B..=0x200F
            | 0x202A..=0x202E
            | 0x2060..=0x2064
            | 0x2066..=0x206F
            | 0xFE00..=0xFE0F
            | 0xFEFF
            | 0xFFF9..=0xFFFB
            | 0x110BD
            | 0x110CD
            | 0x13430..=0x1343F
            | 0x1BCA0..=0x1BCA3
            | 0x1D173..=0x1D17A
            | 0xE0001
            | 0xE0020..=0xE007F
            | 0xE0100..=0xE01EF
    )
}

fn is_emoji(ch: char) -> bool {
    matches!(
        ch as u32,
        0x1F000..=0x1FAFF
            | 0x2600..=0x27BF
            | 0x2B00..=0x2BFF
            | 0x2300..=0x23FF
            | 0x2190..=0x21FF
            | 0x25A0..=0x25FF
            | 0x2934..=0x2935
            | 0xFE00..=0xFE0F
            | 0x20E3
            | 0x2049
            | 0x203C
            | 0x2122
            | 0x2139
            | 0x24C2
            | 0x3030
            | 0x303D
            | 0x3297
            | 0x3299
    )
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
    let mut current_chars = 0usize;
    for sentence in split_punctuation(text) {
        let sentence_chars = sentence.chars().count();
        if !sentence.chars().any(|ch| ch.is_alphanumeric()) {
            continue;
        }
        if sentence_chars <= limit {
            if !current.is_empty() && current_chars + 1 + sentence_chars > limit {
                chunks.push(std::mem::take(&mut current));
                current_chars = 0;
            }
            if !current.is_empty() {
                current.push(' ');
                current_chars += 1;
            }
            current.push_str(&sentence);
            current_chars += sentence_chars;
        } else {
            if !current.is_empty() {
                chunks.push(std::mem::take(&mut current));
                current_chars = 0;
            }
            chunks.extend(split_long_sentence(&sentence, limit));
        }
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

fn split_long_sentence(sentence: &str, limit: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut current_chars = 0usize;
    for word in sentence.split_whitespace() {
        let word_chars = word.chars().count();
        if !current.is_empty() && current_chars + 1 + word_chars > limit {
            out.push(std::mem::take(&mut current));
            current_chars = 0;
        }
        if !current.is_empty() {
            current.push(' ');
            current_chars += 1;
        }
        current.push_str(word);
        current_chars += word_chars;
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

    fn char_budget_report(sentences: &[String], target: usize, title: &str) -> (usize, usize) {
        let result = summarize(
            sentences,
            &SummaryOptions {
                target_chars: target,
                title: title.into(),
            },
        );
        let chars: usize = result
            .selected_indices
            .iter()
            .map(|index| sentences[*index].chars().count())
            .sum();
        let bytes: usize = result
            .selected_indices
            .iter()
            .map(|index| sentences[*index].len())
            .sum();
        (chars, bytes)
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
        assert_eq!(first.selected_indices, second.selected_indices);
        assert!(first
            .selected_indices
            .windows(2)
            .all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn repeated_summarize_calls_produce_identical_script() {
        let input = sentences();
        let options = SummaryOptions {
            target_chars: 400,
            title: "Rust".into(),
        };
        let baseline = summarize(&input, &options);
        for _ in 0..32 {
            let run = summarize(&input, &options);
            assert_eq!(run.selected_indices, baseline.selected_indices);
        }
    }

    #[test]
    fn short_transcript_uses_every_sentence() {
        let result = summarize(
            &["Một câu ngắn.".into(), "Một câu khác.".into()],
            &SummaryOptions {
                target_chars: 3000,
                title: "Ngắn".into(),
            },
        );
        assert_eq!(result.selected_indices, vec![0, 1]);
    }

    #[test]
    fn long_transcript_summarizes_quickly_and_deterministically() {
        let sentences: Vec<String> = (0..3000)
            .map(|i| {
                format!(
                    "Câu số {i} giải thích chủ đề lập trình an toàn, hiệu năng và ví dụ cụ thể."
                )
            })
            .collect();
        let options = SummaryOptions {
            target_chars: 3000,
            title: "Rust".into(),
        };

        let started = std::time::Instant::now();
        let first = summarize(&sentences, &options);
        let elapsed = started.elapsed();
        let second = summarize(&sentences, &options);

        assert_eq!(first.selected_indices, second.selected_indices);
        assert!(!first.selected_indices.is_empty());
        assert!(first
            .selected_indices
            .windows(2)
            .all(|pair| pair[0] < pair[1]));
        assert!(
            elapsed < std::time::Duration::from_secs(5),
            "summarize took {elapsed:?}"
        );
    }

    #[test]
    fn target_budget_counts_characters_for_korean() {
        let sentences: Vec<String> = (0..120)
            .map(|i| format!("{i}번째 문장은 한국어 요약 테스트를 위한 충분히 긴 문장입니다."))
            .collect();
        let target = 2000;
        let (chars, bytes) = char_budget_report(&sentences, target, "한국어");
        let budget = target - INTRO_OUTRO_RESERVE_CHARS;
        let longest = sentences[0].chars().count();
        assert!(chars <= budget, "used {chars} chars over budget {budget}");
        assert!(
            chars >= budget - longest,
            "underfilled: {chars} vs {budget}"
        );
        assert!(bytes > target, "byte budget would have been used: {bytes}");
    }

    #[test]
    fn target_budget_counts_characters_for_vietnamese() {
        let sentences: Vec<String> = (0..150)
            .map(|i| {
                format!("{i} Đây là một câu tiếng Việt đủ dài để kiểm tra ngân sách ký tự của bản tóm tắt.")
            })
            .collect();
        let target = 3000;
        let (chars, bytes) = char_budget_report(&sentences, target, "Tiếng Việt");
        let budget = target - INTRO_OUTRO_RESERVE_CHARS;
        let longest = sentences[0].chars().count();
        assert!(chars <= budget, "used {chars} chars over budget {budget}");
        assert!(
            chars >= budget - longest,
            "underfilled: {chars} vs {budget}"
        );
        assert!(bytes > chars, "expected multibyte text: {bytes} vs {chars}");
    }

    #[test]
    fn sanitizer_removes_markdown_urls_emojis_and_speaker_labels() {
        let input = "**Xin chào** https://example.com #tag 😀\nNgười nói: Nội dung [Music].";
        assert_eq!(sanitize_for_tts(input), "Xin chào Nội dung.");
    }

    #[test]
    fn sanitizer_removes_zero_width_and_broad_emoji() {
        let input = "Xin chào 🎬❤️ 🇻🇳 3️⃣ zero\u{200b}width";
        assert_eq!(sanitize_for_tts(input), "Xin chào 3 zerowidth");
    }

    #[test]
    fn sanitizer_preserves_paragraph_breaks() {
        let input = "Đoạn một.\n\nĐoạn hai.\nDòng ba.";
        assert_eq!(sanitize_for_tts(input), "Đoạn một.\n\nĐoạn hai. Dòng ba.");
    }

    #[test]
    fn sanitizer_removes_bmp_emoji_and_format_codepoints() {
        let removed = [
            '\u{23F0}',
            '\u{2194}',
            '\u{25AA}',
            '\u{2934}',
            '\u{2935}',
            '\u{2049}',
            '\u{203C}',
            '\u{2122}',
            '\u{2139}',
            '\u{24C2}',
            '\u{FFF9}',
            '\u{FFFA}',
            '\u{FFFB}',
            '\u{E0020}',
            '\u{17B4}',
            '\u{17B5}',
        ];
        for ch in removed {
            let input = format!("trước{ch}sau");
            assert_eq!(
                sanitize_for_tts(&input),
                "trướcsau",
                "not removed: U+{:04X}",
                ch as u32
            );
        }
    }

    #[test]
    fn sanitizer_removes_extended_symbols_and_keeps_paragraphs() {
        let input = "Trước ⏰ ↔ ▪ ⁉ ‼ ™ ℹ Ⓜ ⤴ \u{fff9}\u{fffa}\u{fffb}\u{e0020}\u{17b4}\u{17b5} sau.\n\nĐoạn hai.";
        assert_eq!(sanitize_for_tts(input), "Trước sau.\n\nĐoạn hai.");
    }

    #[test]
    fn chunker_respects_limit_and_sentence_boundaries() {
        let text = "Câu một. Câu hai. Câu ba! Câu bốn?";
        let chunks = split_chunks(text, 12);
        assert!(chunks.iter().all(|chunk| chunk.chars().count() <= 12));
        assert_eq!(chunks, vec!["Câu một.", "Câu hai.", "Câu ba!", "Câu bốn?"]);
    }

    #[test]
    fn chunker_keeps_abbreviations_whole_and_drops_punctuation_only() {
        let text = "TP.HCM là thành phố lớn. Mr. Smith nói 3.14. Wait... what?";
        let chunks = split_chunks(text, 200);
        assert!(chunks
            .iter()
            .all(|chunk| chunk.chars().any(|ch| ch.is_alphanumeric())));
        assert!(chunks.iter().any(|chunk| chunk.contains("TP.HCM")));
        assert!(chunks
            .iter()
            .any(|chunk| chunk.contains("Mr. Smith") && chunk.contains("3.14")));
        assert_eq!(
            chunks.join(" ").split_whitespace().collect::<Vec<_>>(),
            text.split_whitespace().collect::<Vec<_>>()
        );
        assert!(split_chunks("...", 10)
            .iter()
            .all(|chunk| chunk.chars().any(|ch| ch.is_alphanumeric())));
    }
}
