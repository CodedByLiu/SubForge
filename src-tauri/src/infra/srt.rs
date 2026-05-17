//! Minimal SRT parsing and formatting, plus subtitle post-processing helpers.

use crate::domain::config::GlossaryEntry;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SubCue {
    pub index: u32,
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
}

const MIN_CUE_DURATION_MS: i64 = 1;
const SOURCE_FRAGMENT_MAX_DURATION_MS: i64 = 1200;
const SOURCE_FRAGMENT_MAX_CHARS: usize = 14;
const SOURCE_FRAGMENT_MAX_WORDS: usize = 3;
const SOURCE_MERGE_MAX_DURATION_MS: i64 = 6500;
const SOURCE_MERGE_MAX_CHARS: usize = 84;
const TARGET_FRAGMENT_MAX_DURATION_MS: i64 = 1200;
const TARGET_FRAGMENT_HARD_MAX_CHARS: usize = 4;
const TARGET_FRAGMENT_SOFT_MAX_CHARS: usize = 6;
const TARGET_MERGE_MAX_DURATION_MS: i64 = 6500;
const TARGET_MERGE_MAX_CHARS: usize = 26;
const BILINGUAL_SOURCE_MERGE_MAX_CHARS: usize = 84;
const BILINGUAL_TARGET_MERGE_MAX_CHARS: usize = 26;
const LEADING_SOURCE_WORDS: &[&str] = &["and", "or", "but", "so", "then", "because"];
const TRAILING_SOURCE_WORDS: &[&str] = &[
    "a", "an", "and", "as", "at", "be", "by", "for", "from", "gonna", "if", "in", "into", "of",
    "on", "or", "so", "the", "to", "up", "with",
];
const LEADING_TARGET_WORDS: &[&str] = &["而且", "并且", "然后", "所以", "但是", "以及", "通过"];
const TRAILING_TARGET_WORDS: &[&str] = &[
    "的", "了", "在", "把", "和", "与", "并", "将", "会", "去", "到", "对", "给", "通过",
];

fn parse_ts(part: &str) -> Option<i64> {
    let p = part.trim();
    let (hms, ms_part) = if let Some((a, b)) = p.split_once(',') {
        (a, b)
    } else if let Some((a, b)) = p.split_once('.') {
        (a, b)
    } else {
        return None;
    };
    let mut it = hms.split(':');
    let h: i64 = it.next()?.parse().ok()?;
    let m: i64 = it.next()?.parse().ok()?;
    let s: i64 = it.next()?.parse().ok()?;
    let ms: i64 = ms_part.trim().parse().ok()?;
    Some(((h * 60 + m) * 60 + s) * 1000 + ms)
}

fn parse_time_line(line: &str) -> Option<(i64, i64)> {
    let (a, b) = line.split_once("-->")?;
    let start = parse_ts(a)?;
    let end = parse_ts(b.split_whitespace().next()?)?;
    Some((start, end))
}

fn format_ts(ms: i64) -> String {
    let h = ms / 3_600_000;
    let m = (ms % 3_600_000) / 60_000;
    let s = (ms % 60_000) / 1000;
    let milli = ms % 1000;
    format!("{h:02}:{m:02}:{s:02},{milli:03}")
}

pub fn parse_srt(raw: &str) -> Result<Vec<SubCue>, String> {
    let normalized = raw.replace('\r', "");
    let mut out = Vec::new();
    for block in normalized.split("\n\n") {
        let lines: Vec<&str> = block
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect();
        if lines.len() < 2 {
            continue;
        }
        let idx: u32 = lines[0]
            .parse()
            .map_err(|_| format!("Invalid cue index: {}", lines[0]))?;
        let (start_ms, end_ms) =
            parse_time_line(lines[1]).ok_or_else(|| format!("Invalid time line: {}", lines[1]))?;
        let text = lines[2..].join("\n");
        out.push(SubCue {
            index: idx,
            start_ms,
            end_ms,
            text,
        });
    }
    if out.is_empty() {
        return Err("No subtitle cues found".into());
    }
    Ok(out)
}

fn cue_duration(cue: &SubCue) -> i64 {
    (cue.end_ms - cue.start_ms).max(0)
}

fn visible_len(text: &str) -> usize {
    text.chars().filter(|ch| !ch.is_whitespace()).count()
}

fn word_count(text: &str) -> usize {
    text.split_whitespace()
        .filter(|part| !part.is_empty())
        .count()
}

fn ends_with_terminal(text: &str) -> bool {
    let trimmed = text.trim_end();
    trimmed.ends_with('.')
        || trimmed.ends_with('!')
        || trimmed.ends_with('?')
        || trimmed.ends_with('。')
        || trimmed.ends_with('！')
        || trimmed.ends_with('？')
}

fn normalize_token(token: &str) -> String {
    token
        .trim_matches(|ch: char| !ch.is_alphanumeric())
        .to_ascii_lowercase()
}

fn first_token(text: &str) -> Option<String> {
    text.split_whitespace()
        .find(|part| !part.is_empty())
        .map(normalize_token)
        .filter(|part| !part.is_empty())
}

fn last_token(text: &str) -> Option<String> {
    text.split_whitespace()
        .rev()
        .find(|part| !part.is_empty())
        .map(normalize_token)
        .filter(|part| !part.is_empty())
}

fn starts_with_any(text: &str, needles: &[&str]) -> bool {
    let trimmed = text.trim_start();
    needles.iter().any(|needle| trimmed.starts_with(needle))
}

fn ends_with_any(text: &str, needles: &[&str]) -> bool {
    let trimmed = text.trim_end();
    needles.iter().any(|needle| trimmed.ends_with(needle))
}

fn should_insert_space(left: &str, right: &str) -> bool {
    let last = left.trim_end().chars().last();
    let first = right.trim_start().chars().next();
    matches!(
        (last, first),
        (Some(a), Some(b)) if a.is_ascii_alphanumeric() && b.is_ascii_alphanumeric()
    )
}

fn join_texts(left: &str, right: &str) -> String {
    let left = left.trim();
    let right = right.trim();
    if left.is_empty() {
        return right.to_string();
    }
    if right.is_empty() {
        return left.to_string();
    }
    if should_insert_space(left, right) {
        format!("{left} {right}")
    } else {
        format!("{left}{right}")
    }
}

fn can_merge_texts(left: &SubCue, right: &SubCue, max_duration_ms: i64, max_chars: usize) -> bool {
    let combined_duration = right.end_ms - left.start_ms;
    if combined_duration > max_duration_ms {
        return false;
    }
    visible_len(&join_texts(&left.text, &right.text)) <= max_chars
}

fn renumber(cues: &mut [SubCue]) {
    for (idx, cue) in cues.iter_mut().enumerate() {
        cue.index = (idx + 1) as u32;
    }
}

fn merge_backward_pass(
    cues: &[SubCue],
    is_fragment: impl Fn(&SubCue) -> bool,
    max_duration_ms: i64,
    max_chars: usize,
) -> Vec<SubCue> {
    let mut out = Vec::<SubCue>::new();
    for cue in cues.iter().cloned() {
        if let Some(prev) = out.last_mut() {
            let prev_fragment = is_fragment(prev);
            let cue_fragment = is_fragment(&cue);
            if !ends_with_terminal(&prev.text)
                && (prev_fragment || cue_fragment)
                && can_merge_texts(prev, &cue, max_duration_ms, max_chars)
            {
                prev.end_ms = cue.end_ms;
                prev.text = join_texts(&prev.text, &cue.text);
                continue;
            }
        }
        out.push(cue);
    }
    out
}

fn merge_forward_pass(
    cues: &[SubCue],
    is_fragment: impl Fn(&SubCue) -> bool + Copy,
    max_duration_ms: i64,
    max_chars: usize,
) -> Vec<SubCue> {
    let mut out = Vec::<SubCue>::new();
    for cue in cues.iter().cloned().rev() {
        if let Some(next) = out.last_mut() {
            let cue_fragment = is_fragment(&cue);
            let next_fragment = is_fragment(next);
            if !ends_with_terminal(&cue.text)
                && (cue_fragment || next_fragment)
                && can_merge_texts(&cue, next, max_duration_ms, max_chars)
            {
                next.start_ms = cue.start_ms;
                next.text = join_texts(&cue.text, &next.text);
                continue;
            }
        }
        out.push(cue);
    }
    out.reverse();
    out
}

fn is_source_fragment(cue: &SubCue) -> bool {
    let text = cue.text.trim();
    if text.is_empty() {
        return true;
    }
    if ends_with_terminal(text) {
        return false;
    }
    let words = word_count(text);
    let len = visible_len(text);
    if cue_duration(cue) <= SOURCE_FRAGMENT_MAX_DURATION_MS && len <= SOURCE_FRAGMENT_MAX_CHARS {
        return true;
    }
    if words <= SOURCE_FRAGMENT_MAX_WORDS && len <= SOURCE_FRAGMENT_MAX_CHARS {
        return true;
    }
    if first_token(text)
        .as_deref()
        .map(|token| LEADING_SOURCE_WORDS.contains(&token))
        .unwrap_or(false)
    {
        return true;
    }
    last_token(text)
        .as_deref()
        .map(|token| TRAILING_SOURCE_WORDS.contains(&token))
        .unwrap_or(false)
}

fn is_target_fragment(cue: &SubCue) -> bool {
    let text = cue.text.trim();
    if text.is_empty() {
        return true;
    }
    if ends_with_terminal(text) {
        return false;
    }
    let len = visible_len(text);
    if len <= TARGET_FRAGMENT_HARD_MAX_CHARS {
        return true;
    }
    if len <= TARGET_FRAGMENT_SOFT_MAX_CHARS && cue_duration(cue) <= TARGET_FRAGMENT_MAX_DURATION_MS
    {
        return true;
    }
    starts_with_any(text, LEADING_TARGET_WORDS) || ends_with_any(text, TRAILING_TARGET_WORDS)
}

pub fn optimize_source_cues(cues: &[SubCue]) -> Vec<SubCue> {
    let first = merge_backward_pass(
        cues,
        is_source_fragment,
        SOURCE_MERGE_MAX_DURATION_MS,
        SOURCE_MERGE_MAX_CHARS,
    );
    let mut merged = merge_forward_pass(
        &first,
        is_source_fragment,
        SOURCE_MERGE_MAX_DURATION_MS,
        SOURCE_MERGE_MAX_CHARS,
    );
    renumber(&mut merged);
    merged
}

pub fn build_translated_cues(
    sources: &[SubCue],
    translated_lines: &[String],
) -> Result<Vec<SubCue>, String> {
    if sources.len() != translated_lines.len() {
        return Err(format!(
            "Source/translation cue count mismatch: {} / {}",
            sources.len(),
            translated_lines.len()
        ));
    }
    Ok(sources
        .iter()
        .zip(translated_lines.iter())
        .map(|(c, t)| SubCue {
            index: c.index,
            start_ms: c.start_ms,
            end_ms: c.end_ms,
            text: t.trim().to_string(),
        })
        .collect())
}

pub fn optimize_translated_cues(cues: &[SubCue]) -> Vec<SubCue> {
    let first = merge_backward_pass(
        cues,
        is_target_fragment,
        TARGET_MERGE_MAX_DURATION_MS,
        TARGET_MERGE_MAX_CHARS,
    );
    let mut merged = merge_forward_pass(
        &first,
        is_target_fragment,
        TARGET_MERGE_MAX_DURATION_MS,
        TARGET_MERGE_MAX_CHARS,
    );
    renumber(&mut merged);
    merged
}

pub fn normalize_cues_for_srt(cues: &[SubCue]) -> Vec<SubCue> {
    let mut out: Vec<SubCue> = cues.to_vec();
    out.sort_by_key(|c| (c.start_ms, c.end_ms, c.index));

    for cue in &mut out {
        if cue.start_ms < 0 {
            cue.start_ms = 0;
        }
        if cue.end_ms <= cue.start_ms {
            cue.end_ms = cue.start_ms + MIN_CUE_DURATION_MS;
        }
    }

    for i in 1..out.len() {
        let prev = i - 1;
        if out[i].start_ms < out[prev].end_ms {
            let desired_prev_end = out[i].start_ms;
            if desired_prev_end > out[prev].start_ms {
                out[prev].end_ms = desired_prev_end;
            } else {
                out[i].start_ms = out[prev].end_ms;
            }
        }
        if out[i].end_ms <= out[i].start_ms {
            out[i].end_ms = out[i].start_ms + MIN_CUE_DURATION_MS;
        }
    }

    renumber(&mut out);
    out
}

/// 按识别术语表对转录文本做强制替换。
///
/// - 长 `source` 优先替换，避免短词破坏已经替换好的长术语。
/// - 全 ASCII 的 `source` 走"词边界"匹配（前后字符若是 ASCII alphanumeric/`_` 则视为词内，不替换），
///   避免 `Claud → Claude` 时把 `Claudia` 改成 `Claudeia`。
/// - 含 CJK 等非 ASCII 字符的 `source` 直接子串替换（CJK 无词边界概念）。
/// - 空 `source` / 空 `target` 跳过，避免误删文本。
pub fn apply_recognition_glossary(
    cues: &mut [SubCue],
    glossary: &[GlossaryEntry],
    case_sensitive: bool,
) {
    let mut entries: Vec<(String, String, bool)> = glossary
        .iter()
        .filter_map(|e| {
            let src = e.source.trim();
            let tgt = e.target.trim();
            if src.is_empty() || tgt.is_empty() || src == tgt {
                return None;
            }
            let ascii = src.chars().all(|c| c.is_ascii());
            Some((src.to_string(), tgt.to_string(), ascii))
        })
        .collect();
    if entries.is_empty() {
        return;
    }
    entries.sort_by(|a, b| b.0.chars().count().cmp(&a.0.chars().count()));

    for cue in cues.iter_mut() {
        for (src, tgt, ascii) in &entries {
            if *ascii {
                cue.text = replace_ascii_word(&cue.text, src, tgt, case_sensitive);
            } else {
                cue.text = replace_substring(&cue.text, src, tgt, case_sensitive);
            }
        }
    }
}

fn is_ascii_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

fn replace_ascii_word(haystack: &str, needle: &str, replacement: &str, case_sensitive: bool) -> String {
    if needle.is_empty() {
        return haystack.to_string();
    }
    let hay_cmp = if case_sensitive {
        haystack.to_string()
    } else {
        haystack.to_ascii_lowercase()
    };
    let needle_cmp = if case_sensitive {
        needle.to_string()
    } else {
        needle.to_ascii_lowercase()
    };
    let mut out = String::with_capacity(haystack.len());
    let mut cursor = 0usize;
    let bytes = haystack.as_bytes();
    while cursor <= hay_cmp.len() {
        match hay_cmp[cursor..].find(&needle_cmp) {
            Some(rel) => {
                let abs = cursor + rel;
                let before_ok = if abs == 0 {
                    true
                } else {
                    // 前一个字符若是 ASCII 词字符，则在词中部
                    !bytes
                        .get(abs - 1)
                        .map(|&b| (b as char).is_ascii() && is_ascii_word_char(b as char))
                        .unwrap_or(false)
                };
                let end = abs + needle_cmp.len();
                let after_ok = if end >= bytes.len() {
                    true
                } else {
                    !bytes
                        .get(end)
                        .map(|&b| (b as char).is_ascii() && is_ascii_word_char(b as char))
                        .unwrap_or(false)
                };
                out.push_str(&haystack[cursor..abs]);
                if before_ok && after_ok {
                    out.push_str(replacement);
                } else {
                    out.push_str(&haystack[abs..end]);
                }
                cursor = end;
                if rel == 0 && needle_cmp.is_empty() {
                    break;
                }
            }
            None => {
                out.push_str(&haystack[cursor..]);
                break;
            }
        }
    }
    out
}

fn replace_substring(haystack: &str, needle: &str, replacement: &str, case_sensitive: bool) -> String {
    if needle.is_empty() {
        return haystack.to_string();
    }
    if case_sensitive {
        return haystack.replace(needle, replacement);
    }
    let hay_lower = haystack.to_lowercase();
    let needle_lower = needle.to_lowercase();
    let mut out = String::with_capacity(haystack.len());
    let mut cursor = 0usize;
    while cursor <= hay_lower.len() {
        match hay_lower[cursor..].find(&needle_lower) {
            Some(rel) => {
                let abs = cursor + rel;
                let end = abs + needle_lower.len();
                out.push_str(&haystack[cursor..abs]);
                out.push_str(replacement);
                cursor = end;
            }
            None => {
                out.push_str(&haystack[cursor..]);
                break;
            }
        }
    }
    out
}

/// 把识别术语表拼成 whisper.cpp 用的 initial prompt。
/// 按 UTF-8 字符长度裁剪到 ~800（粗略对应 ~200 tokens 安全线），避免超过模型上下文上限。
pub fn build_recognition_prompt(glossary: &[GlossaryEntry]) -> Option<String> {
    const PROMPT_PREFIX: &str = "Vocabulary: ";
    const MAX_CHARS: usize = 800;
    let mut parts: Vec<String> = Vec::new();
    for e in glossary {
        let s = e.source.trim();
        let t = e.target.trim();
        if s.is_empty() || t.is_empty() {
            continue;
        }
        if !parts.iter().any(|p| p == t) {
            parts.push(t.to_string());
        }
    }
    if parts.is_empty() {
        return None;
    }
    let mut body = String::new();
    let prefix_len = PROMPT_PREFIX.chars().count();
    for term in parts {
        let sep_len = if body.is_empty() { 0 } else { 2 };
        let term_len = term.chars().count();
        if prefix_len + body.chars().count() + sep_len + term_len > MAX_CHARS {
            break;
        }
        if !body.is_empty() {
            body.push_str(", ");
        }
        body.push_str(&term);
    }
    if body.is_empty() {
        None
    } else {
        Some(format!("{}{}", PROMPT_PREFIX, body))
    }
}

pub fn format_srt(cues: &[SubCue]) -> String {
    let normalized = normalize_cues_for_srt(cues);
    let mut s = String::new();
    for c in &normalized {
        s.push_str(&format!("{}\n", c.index));
        s.push_str(&format!(
            "{} --> {}\n",
            format_ts(c.start_ms),
            format_ts(c.end_ms)
        ));
        s.push_str(&c.text);
        s.push_str("\n\n");
    }
    s
}

pub fn build_bilingual_cues(
    sources: &[SubCue],
    translated_lines: &[String],
) -> Result<Vec<SubCue>, String> {
    if sources.len() != translated_lines.len() {
        return Err(format!(
            "Bilingual cue count mismatch: {} / {}",
            sources.len(),
            translated_lines.len()
        ));
    }
    Ok(sources
        .iter()
        .zip(translated_lines.iter())
        .map(|(c, t)| SubCue {
            index: c.index,
            start_ms: c.start_ms,
            end_ms: c.end_ms,
            text: format!("{}\n{}", t.trim(), c.text.trim()),
        })
        .collect())
}

pub fn build_bilingual_cues_optimized(
    sources: &[SubCue],
    translated_lines: &[String],
) -> Result<Vec<SubCue>, String> {
    if sources.len() != translated_lines.len() {
        return Err(format!(
            "Bilingual cue count mismatch: {} / {}",
            sources.len(),
            translated_lines.len()
        ));
    }

    let mut merged_sources = Vec::<SubCue>::new();
    let mut merged_targets = Vec::<String>::new();

    for (source, target) in sources
        .iter()
        .cloned()
        .zip(translated_lines.iter().cloned())
    {
        let current_target = target.trim().to_string();
        if let (Some(prev_source), Some(prev_target)) =
            (merged_sources.last_mut(), merged_targets.last_mut())
        {
            let prev_target_cue = SubCue {
                index: prev_source.index,
                start_ms: prev_source.start_ms,
                end_ms: prev_source.end_ms,
                text: prev_target.clone(),
            };
            let current_target_cue = SubCue {
                index: source.index,
                start_ms: source.start_ms,
                end_ms: source.end_ms,
                text: current_target.clone(),
            };
            let should_merge = !ends_with_terminal(&prev_source.text)
                && can_merge_texts(
                    prev_source,
                    &source,
                    TARGET_MERGE_MAX_DURATION_MS,
                    BILINGUAL_SOURCE_MERGE_MAX_CHARS,
                )
                && visible_len(&join_texts(prev_target, &current_target))
                    <= BILINGUAL_TARGET_MERGE_MAX_CHARS
                && (is_source_fragment(prev_source)
                    || is_source_fragment(&source)
                    || is_target_fragment(&prev_target_cue)
                    || is_target_fragment(&current_target_cue));
            if should_merge {
                prev_source.end_ms = source.end_ms;
                prev_source.text = join_texts(&prev_source.text, &source.text);
                *prev_target = join_texts(prev_target, &current_target);
                continue;
            }
        }
        merged_sources.push(source);
        merged_targets.push(current_target);
    }

    let mut bilingual = build_bilingual_cues(&merged_sources, &merged_targets)?;
    renumber(&mut bilingual);
    Ok(bilingual)
}

#[cfg(test)]
mod tests {
    use super::{
        apply_recognition_glossary, build_bilingual_cues_optimized, build_recognition_prompt,
        normalize_cues_for_srt, optimize_source_cues, optimize_translated_cues, SubCue,
    };
    use crate::domain::config::GlossaryEntry;

    fn cue(text: &str) -> SubCue {
        SubCue {
            index: 1,
            start_ms: 0,
            end_ms: 1000,
            text: text.into(),
        }
    }

    fn entry(source: &str, target: &str) -> GlossaryEntry {
        GlossaryEntry {
            source: source.into(),
            target: target.into(),
            note: String::new(),
        }
    }

    #[test]
    fn recognition_glossary_replaces_ascii_with_word_boundary() {
        let mut cues = vec![cue("I love Claud but Claudia is here")];
        apply_recognition_glossary(&mut cues, &[entry("Claud", "Claude")], false);
        assert_eq!(cues[0].text, "I love Claude but Claudia is here");
    }

    #[test]
    fn recognition_glossary_prefers_longer_source() {
        let mut cues = vec![cue("ClaudCode and Claud are different")];
        apply_recognition_glossary(
            &mut cues,
            &[entry("Claud", "Claude"), entry("ClaudCode", "ClaudeCode")],
            false,
        );
        assert_eq!(cues[0].text, "ClaudeCode and Claude are different");
    }

    #[test]
    fn recognition_glossary_case_insensitive_by_default() {
        let mut cues = vec![cue("claud and CLAUD here")];
        apply_recognition_glossary(&mut cues, &[entry("Claud", "Claude")], false);
        assert_eq!(cues[0].text, "Claude and Claude here");
    }

    #[test]
    fn recognition_glossary_case_sensitive_when_enabled() {
        let mut cues = vec![cue("claud and Claud here")];
        apply_recognition_glossary(&mut cues, &[entry("Claud", "Claude")], true);
        assert_eq!(cues[0].text, "claud and Claude here");
    }

    #[test]
    fn recognition_glossary_replaces_cjk_substring() {
        let mut cues = vec![cue("克劳德是个好工具，但克劳德也有限")];
        apply_recognition_glossary(&mut cues, &[entry("克劳德", "Claude")], false);
        assert_eq!(cues[0].text, "Claude是个好工具，但Claude也有限");
    }

    #[test]
    fn recognition_glossary_skips_empty_entries() {
        let mut cues = vec![cue("hello world")];
        apply_recognition_glossary(
            &mut cues,
            &[entry("", "X"), entry("hello", ""), entry("world", "world")],
            false,
        );
        assert_eq!(cues[0].text, "hello world");
    }

    #[test]
    fn build_prompt_basic() {
        let glossary = vec![
            entry("Claud", "Claude"),
            entry("ClaudCode", "ClaudeCode"),
            entry("", "Skipped"),
        ];
        let prompt = build_recognition_prompt(&glossary).unwrap();
        assert_eq!(prompt, "Vocabulary: Claude, ClaudeCode");
    }

    #[test]
    fn build_prompt_dedupes_target() {
        let glossary = vec![entry("a", "Claude"), entry("b", "Claude")];
        let prompt = build_recognition_prompt(&glossary).unwrap();
        assert_eq!(prompt, "Vocabulary: Claude");
    }

    #[test]
    fn build_prompt_truncates_to_safe_length() {
        let glossary: Vec<GlossaryEntry> = (0..200)
            .map(|i| entry(&format!("s{i}"), &format!("Term{i:04}")))
            .collect();
        let prompt = build_recognition_prompt(&glossary).unwrap();
        assert!(prompt.chars().count() <= 800);
    }

    #[test]
    fn build_prompt_returns_none_when_empty() {
        assert!(build_recognition_prompt(&[]).is_none());
        assert!(build_recognition_prompt(&[entry("a", "")]).is_none());
    }

    #[test]
    fn normalize_removes_adjacent_overlaps() {
        let cues = vec![
            SubCue {
                index: 1,
                start_ms: 1000,
                end_ms: 2000,
                text: "Hello guys,".into(),
            },
            SubCue {
                index: 2,
                start_ms: 1800,
                end_ms: 2600,
                text: "my name is Pavel".into(),
            },
            SubCue {
                index: 3,
                start_ms: 2400,
                end_ms: 3200,
                text: "Krupalov from blenderfreak.com".into(),
            },
        ];
        let out = normalize_cues_for_srt(&cues);
        assert_eq!(out.len(), 3);
        assert!(out[0].end_ms <= out[1].start_ms);
        assert!(out[1].end_ms <= out[2].start_ms);
        assert_eq!(out[0].index, 1);
        assert_eq!(out[2].index, 3);
    }

    #[test]
    fn normalize_repairs_non_positive_durations() {
        let cues = vec![
            SubCue {
                index: 9,
                start_ms: 500,
                end_ms: 500,
                text: "a".into(),
            },
            SubCue {
                index: 3,
                start_ms: 500,
                end_ms: 700,
                text: "b".into(),
            },
        ];
        let out = normalize_cues_for_srt(&cues);
        assert!(out[0].end_ms > out[0].start_ms);
        assert!(out[1].end_ms > out[1].start_ms);
        assert!(out[0].end_ms <= out[1].start_ms);
        assert_eq!(out[0].index, 1);
        assert_eq!(out[1].index, 2);
    }

    #[test]
    fn optimize_source_cues_merges_short_english_fragments() {
        let cues = vec![
            SubCue {
                index: 1,
                start_ms: 0,
                end_ms: 4200,
                text: "as you can see here the previous version we had just directly".into(),
            },
            SubCue {
                index: 2,
                start_ms: 4200,
                end_ms: 5000,
                text: "instancing".into(),
            },
        ];
        let out = optimize_source_cues(&cues);
        assert_eq!(out.len(), 1);
        assert!(out[0].text.contains("instancing"));
    }

    #[test]
    fn optimize_translated_cues_merges_short_chinese_fragments() {
        let cues = vec![
            SubCue {
                index: 1,
                start_ms: 0,
                end_ms: 4200,
                text: "覆盖你的 Node Editor 小部件，你可以".into(),
            },
            SubCue {
                index: 2,
                start_ms: 4200,
                end_ms: 5000,
                text: "通过".into(),
            },
        ];
        let out = optimize_translated_cues(&cues);
        assert_eq!(out.len(), 1);
        assert!(out[0].text.contains("通过"));
    }

    #[test]
    fn bilingual_optimization_merges_joint_fragments() {
        let sources = vec![
            SubCue {
                index: 1,
                start_ms: 0,
                end_ms: 4200,
                text: "your Node Editor widget with your own graphic view you can".into(),
            },
            SubCue {
                index: 2,
                start_ms: 4200,
                end_ms: 5000,
                text: "do that by".into(),
            },
        ];
        let translated = vec!["覆盖你的 Node Editor 小部件，你可以".into(), "通过".into()];
        let out = build_bilingual_cues_optimized(&sources, &translated).unwrap();
        assert_eq!(out.len(), 1);
        assert!(out[0].text.contains("通过"));
        assert!(out[0].text.contains("do that by"));
    }
}
