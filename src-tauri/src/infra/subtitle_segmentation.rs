use std::path::Path;
use std::time::Duration;

use reqwest::blocking::Client;
use serde_json::{json, Value};

use super::openai_compat::chat_completions_url;
use super::srt::SubCue;

const MIN_SEGMENT_DURATION_MS: i64 = 800;
const MIN_SEGMENT_VISIBLE_CHARS: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
struct TimeUnit {
    start_ms: i64,
    end_ms: i64,
    text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TokenKind {
    Word,
    Space,
    Punct,
}

#[derive(Debug, Clone)]
struct TokenSpan {
    start_char: usize,
    kind: TokenKind,
    text: String,
}

#[derive(Debug, Clone, Copy)]
struct BoundaryPoint {
    char_pos: usize,
    strong: bool,
}

pub struct SegmentationJob<'a> {
    pub strategy: &'a str,
    pub timing_mode: &'a str,
    pub max_chars_per_segment: u32,
    pub max_duration_ms: u32,
    pub llm_base_url: &'a str,
    pub llm_model: &'a str,
    pub llm_api_key: &'a str,
    pub llm_timeout_sec: u64,
}

pub struct SegmentationResult {
    pub cues: Vec<SubCue>,
    pub note: Option<String>,
}

pub fn segment_cues(
    client: &Client,
    job: &SegmentationJob<'_>,
    cues: &[SubCue],
    whisper_json_path: Option<&Path>,
) -> Result<SegmentationResult, String> {
    if cues.is_empty() || job.strategy == "disabled" {
        return Ok(SegmentationResult {
            cues: cues.to_vec(),
            note: None,
        });
    }

    let timing_units = if job.timing_mode == "word_timestamps_first" {
        whisper_json_path
            .and_then(|p| extract_time_units_from_whisper_json(p).ok())
            .unwrap_or_default()
    } else {
        Vec::new()
    };

    let mut out = Vec::new();
    let mut notes = Vec::<String>::new();

    for cue in cues {
        let should_split = cue_should_split(
            cue,
            job.max_chars_per_segment.max(1),
            job.max_duration_ms.max(1) as i64,
        );
        let parts = if should_split {
            split_text_parts(client, job, cue)?
        } else {
            vec![normalize_part_text(&cue.text)]
        };

        let segmented = if parts.len() <= 1 {
            vec![SubCue {
                index: 0,
                start_ms: cue.start_ms,
                end_ms: cue.end_ms,
                text: normalize_part_text(&cue.text),
            }]
        } else if let Some(cues_with_time) = apply_word_timing(cue, &parts, &timing_units) {
            cues_with_time
        } else {
            if job.timing_mode == "word_timestamps_first" {
                notes.push("Subtitle timing fell back to approximate reflow.".into());
            }
            apply_approximate_timing(cue, &parts)
        };

        out.extend(merge_short_cues(segmented));
    }

    renumber_cues(&mut out);
    Ok(SegmentationResult {
        cues: out,
        note: dedupe_notes(notes),
    })
}

fn renumber_cues(cues: &mut [SubCue]) {
    for (idx, cue) in cues.iter_mut().enumerate() {
        cue.index = (idx + 1) as u32;
    }
}

fn dedupe_notes(notes: Vec<String>) -> Option<String> {
    let mut uniq = Vec::<String>::new();
    for note in notes {
        if !uniq.iter().any(|x| x == &note) {
            uniq.push(note);
        }
    }
    (!uniq.is_empty()).then(|| uniq.join(", "))
}

fn cue_should_split(cue: &SubCue, max_chars: u32, max_duration_ms: i64) -> bool {
    let char_count = cue.text.chars().filter(|ch| !ch.is_control()).count() as u32;
    let duration = (cue.end_ms - cue.start_ms).max(0);
    char_count > max_chars || duration > max_duration_ms
}

fn split_text_parts(
    client: &Client,
    job: &SegmentationJob<'_>,
    cue: &SubCue,
) -> Result<Vec<String>, String> {
    match job.strategy {
        "rules_only" => Ok(split_by_rules(
            &cue.text,
            desired_parts(cue, job.max_chars_per_segment, job.max_duration_ms),
        )),
        "auto" | "llm_preferred" => {
            if llm_is_available(job) {
                match split_by_llm(client, job, cue) {
                    Ok(parts) => Ok(parts),
                    Err(e) => {
                        let fallback = split_by_rules(
                            &cue.text,
                            desired_parts(cue, job.max_chars_per_segment, job.max_duration_ms),
                        );
                        log::warn!("LLM subtitle segmentation failed, fallback to rules: {e}");
                        Ok(if fallback.is_empty() {
                            vec![normalize_part_text(&cue.text)]
                        } else {
                            fallback
                        })
                    }
                }
            } else if job.strategy == "auto" {
                Ok(split_by_rules(
                    &cue.text,
                    desired_parts(cue, job.max_chars_per_segment, job.max_duration_ms),
                ))
            } else {
                Err("LLM-preferred segmentation requested but no usable LLM configuration is available.".into())
            }
        }
        _ => Ok(vec![normalize_part_text(&cue.text)]),
    }
}

fn llm_is_available(job: &SegmentationJob<'_>) -> bool {
    !job.llm_base_url.trim().is_empty()
        && !job.llm_model.trim().is_empty()
        && !job.llm_api_key.trim().is_empty()
}

fn desired_parts(cue: &SubCue, max_chars: u32, max_duration_ms: u32) -> usize {
    let chars = cue.text.chars().count().max(1);
    let duration_ms = (cue.end_ms - cue.start_ms).max(1) as usize;
    let by_chars = chars.div_ceil(max_chars.max(1) as usize);
    let by_duration = duration_ms.div_ceil(max_duration_ms.max(1) as usize);
    let base = by_chars.max(by_duration).max(1);
    let max_by_duration = (duration_ms / MIN_SEGMENT_DURATION_MS as usize).max(1);
    base.min(max_by_duration.max(1))
}

fn split_by_rules(text: &str, desired_parts: usize) -> Vec<String> {
    if desired_parts <= 1 {
        return vec![normalize_part_text(text)];
    }
    let char_count = text.chars().count();
    if char_count < 2 {
        return vec![normalize_part_text(text)];
    }

    let boundaries = build_boundaries(text);
    if boundaries.is_empty() {
        return vec![normalize_part_text(text)];
    }

    let mut cuts = Vec::<usize>::new();
    let mut after = 0usize;
    let cuts_needed = desired_parts.saturating_sub(1);

    for part_idx in 0..cuts_needed {
        let target = char_count * (part_idx + 1) / desired_parts;
        let reserve_tail = cuts_needed - part_idx - 1;
        if let Some(cut) = choose_boundary(&boundaries, after, target, reserve_tail) {
            if cut > after && cut < char_count {
                cuts.push(cut);
                after = cut;
            }
        }
    }

    let parts = split_by_safe_positions(text, &cuts);
    if parts.len() <= 1 {
        vec![normalize_part_text(text)]
    } else {
        parts
    }
}

fn choose_boundary(
    boundaries: &[BoundaryPoint],
    after_char: usize,
    target: usize,
    reserve_tail: usize,
) -> Option<usize> {
    let start_idx = boundaries.partition_point(|b| b.char_pos <= after_char);
    let end_exclusive = boundaries.len().saturating_sub(reserve_tail);
    if start_idx >= end_exclusive {
        return None;
    }

    let mut best = None::<(usize, usize)>;
    for boundary in &boundaries[start_idx..end_exclusive] {
        let dist = boundary.char_pos.abs_diff(target);
        let penalty = if boundary.strong { 0 } else { 8 };
        let score = dist.saturating_mul(4).saturating_add(penalty);
        if best.map(|(_, s)| score < s).unwrap_or(true) {
            best = Some((boundary.char_pos, score));
        }
    }
    best.map(|(pos, _)| pos)
}

fn build_boundaries(text: &str) -> Vec<BoundaryPoint> {
    let tokens = tokenize_text(text);
    if tokens.len() < 2 {
        return Vec::new();
    }

    let mut out = Vec::<BoundaryPoint>::new();
    for idx in 1..tokens.len() {
        let prev = &tokens[idx - 1];
        let next = &tokens[idx];
        if !can_split_between(prev, next) {
            continue;
        }
        let strong = boundary_is_strong(&tokens, idx);
        if out
            .last()
            .map(|b| b.char_pos == next.start_char)
            .unwrap_or(false)
        {
            continue;
        }
        out.push(BoundaryPoint {
            char_pos: next.start_char,
            strong,
        });
    }
    out
}

fn can_split_between(prev: &TokenSpan, next: &TokenSpan) -> bool {
    matches!(prev.kind, TokenKind::Space | TokenKind::Punct)
        || matches!(next.kind, TokenKind::Space)
}

fn boundary_is_strong(tokens: &[TokenSpan], idx: usize) -> bool {
    let prev = &tokens[idx - 1];
    if prev.text.contains('\n') {
        return true;
    }
    if token_has_pause_punct(&prev.text) {
        return true;
    }
    if matches!(prev.kind, TokenKind::Space) && idx >= 2 {
        return token_has_pause_punct(&tokens[idx - 2].text);
    }
    false
}

fn token_has_pause_punct(text: &str) -> bool {
    text.chars()
        .rev()
        .find(|ch| !ch.is_whitespace())
        .map(is_pause_punct)
        .unwrap_or(false)
}

fn is_pause_punct(ch: char) -> bool {
    matches!(ch, ',' | '.' | '!' | '?' | ';' | ':')
}

fn tokenize_text(text: &str) -> Vec<TokenSpan> {
    let chars: Vec<char> = text.chars().collect();
    let mut tokens = Vec::<TokenSpan>::new();
    let mut i = 0usize;

    while i < chars.len() {
        let start = i;
        let ch = chars[i];

        if ch.is_whitespace() {
            i += 1;
            while i < chars.len() && chars[i].is_whitespace() {
                i += 1;
            }
            tokens.push(TokenSpan {
                start_char: start,
                kind: TokenKind::Space,
                text: chars[start..i].iter().collect(),
            });
            continue;
        }

        if is_word_core(ch) {
            i += 1;
            while i < chars.len() {
                if is_word_core(chars[i]) || is_word_connector_at(&chars, i) {
                    i += 1;
                } else {
                    break;
                }
            }
            tokens.push(TokenSpan {
                start_char: start,
                kind: TokenKind::Word,
                text: chars[start..i].iter().collect(),
            });
            continue;
        }

        i += 1;
        tokens.push(TokenSpan {
            start_char: start,
            kind: TokenKind::Punct,
            text: chars[start..i].iter().collect(),
        });
    }

    tokens
}

fn is_word_core(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

fn is_word_connector_at(chars: &[char], idx: usize) -> bool {
    let Some(&curr) = chars.get(idx) else {
        return false;
    };
    if !matches!(curr, '\'' | '’' | '-' | '.' | '_') {
        return false;
    }
    let Some(prev) = idx.checked_sub(1).and_then(|i| chars.get(i)) else {
        return false;
    };
    let Some(next) = chars.get(idx + 1) else {
        return false;
    };
    is_word_core(*prev) && is_word_core(*next)
}

fn split_by_safe_positions(text: &str, cuts: &[usize]) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut start = 0usize;

    for &cut in cuts {
        if cut <= start || cut >= chars.len() {
            continue;
        }
        let part = normalize_part_text(&chars[start..cut].iter().collect::<String>());
        if !part.is_empty() {
            out.push(part);
        }
        start = cut;
    }

    let tail = normalize_part_text(&chars[start..].iter().collect::<String>());
    if !tail.is_empty() {
        out.push(tail);
    }
    out
}

fn split_by_llm(
    client: &Client,
    job: &SegmentationJob<'_>,
    cue: &SubCue,
) -> Result<Vec<String>, String> {
    let char_len = cue.text.chars().count();
    if char_len < 2 {
        return Ok(vec![normalize_part_text(&cue.text)]);
    }

    let desired = desired_parts(cue, job.max_chars_per_segment, job.max_duration_ms);
    let body = json!({
        "model": job.llm_model,
        "messages": [
            {
                "role": "system",
                "content": "You return only JSON cut positions for subtitle segmentation. Do not rewrite the text."
            },
            {
                "role": "user",
                "content": format!(
                    "Given one subtitle text, return a JSON array of 0-based Unicode character cut positions.\n\
    Requirements:\n\
    1. Cut only at natural phrase boundaries.\n\
    2. Do not return 0 or the full length {char_len}.\n\
    3. Target about {desired} subtitle parts, but you may return fewer cuts if the sentence should stay intact.\n\
    4. Output JSON only, for example [12, 27].\n\
    5. Do not modify the original text.\n\
    Text:\n{}",
                    cue.text
                )
            }
        ],
        "temperature": 0.1,
        "max_tokens": 256,
    });

    let url = chat_completions_url(job.llm_base_url);
    let resp = client
        .post(url)
        .timeout(Duration::from_secs(job.llm_timeout_sec.max(5)))
        .header(
            "Authorization",
            format!("Bearer {}", job.llm_api_key.trim()),
        )
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .map_err(|e| format!("LLM subtitle segmentation request failed: {e}"))?;

    let status = resp.status();
    let text = resp.text().unwrap_or_default();
    if !status.is_success() {
        return Err(format!(
            "LLM subtitle segmentation HTTP {}: {}",
            status.as_u16(),
            text.chars().take(300).collect::<String>()
        ));
    }

    let v: Value = serde_json::from_str(&text)
        .map_err(|e| format!("LLM subtitle response was not JSON: {e}"))?;
    let content = v
        .pointer("/choices/0/message/content")
        .and_then(|x| x.as_str())
        .ok_or_else(|| "LLM subtitle response is missing choices[0].message.content".to_string())?;

    let raw_cuts = parse_cut_positions(content, char_len)?;
    let safe_cuts = snap_llm_cuts_to_boundaries(&cue.text, &raw_cuts);
    let parts = split_by_safe_positions(&cue.text, &safe_cuts);
    if parts.len() <= 1 {
        return Ok(vec![normalize_part_text(&cue.text)]);
    }
    Ok(parts)
}

fn snap_llm_cuts_to_boundaries(text: &str, raw_cuts: &[usize]) -> Vec<usize> {
    let char_count = text.chars().count();
    let boundaries = build_boundaries(text);
    if boundaries.is_empty() {
        return Vec::new();
    }

    let mut out = Vec::<usize>::new();
    let mut after = 0usize;
    for (idx, target) in raw_cuts.iter().copied().enumerate() {
        let reserve_tail = raw_cuts.len() - idx - 1;
        if let Some(cut) = choose_boundary(&boundaries, after, target, reserve_tail) {
            if cut > after && cut < char_count {
                out.push(cut);
                after = cut;
            }
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

fn parse_cut_positions(content: &str, char_len: usize) -> Result<Vec<usize>, String> {
    let trimmed = strip_json_fence(content);
    let mut cuts: Vec<usize> =
        serde_json::from_str(&trimmed).map_err(|e| format!("LLM cut JSON parse failed: {e}"))?;
    cuts.sort_unstable();
    cuts.dedup();
    if cuts.iter().any(|&c| c == 0 || c >= char_len) {
        return Err("LLM cut positions were out of range.".into());
    }
    Ok(cuts)
}

fn strip_json_fence(s: &str) -> String {
    let t = s.trim();
    if let Some(i) = t.find('[') {
        if let Some(j) = t.rfind(']') {
            return t[i..=j].to_string();
        }
    }
    t.trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim()
        .to_string()
}

fn apply_word_timing(cue: &SubCue, parts: &[String], units: &[TimeUnit]) -> Option<Vec<SubCue>> {
    if parts.len() <= 1 {
        return Some(vec![SubCue {
            index: 0,
            start_ms: cue.start_ms,
            end_ms: cue.end_ms,
            text: normalize_part_text(&cue.text),
        }]);
    }

    let overlapping: Vec<&TimeUnit> = units
        .iter()
        .filter(|u| u.end_ms > cue.start_ms && u.start_ms < cue.end_ms)
        .filter(|u| visible_chars(&u.text) > 0)
        .collect();
    if overlapping.len() < parts.len() {
        return None;
    }

    let unit_weights: Vec<usize> = overlapping
        .iter()
        .map(|u| visible_chars(&u.text).max(1))
        .collect();
    let part_weights: Vec<usize> = parts.iter().map(|p| visible_chars(p).max(1)).collect();
    let total_part_weight: usize = part_weights.iter().sum();
    let total_unit_weight: usize = unit_weights.iter().sum();
    if total_part_weight == 0 || total_unit_weight == 0 {
        return None;
    }

    let mut prefix = Vec::with_capacity(unit_weights.len() + 1);
    prefix.push(0usize);
    for weight in &unit_weights {
        let next = prefix.last().copied().unwrap_or(0).saturating_add(*weight);
        prefix.push(next);
    }

    let mut groups = Vec::<(usize, usize)>::new();
    let mut prev_end = 0usize;
    let mut part_acc = 0usize;

    for part_idx in 0..parts.len() - 1 {
        part_acc += part_weights[part_idx];
        let target = part_acc * total_unit_weight / total_part_weight;
        let min_end = prev_end + 1;
        let max_end = overlapping.len() - (parts.len() - part_idx - 1);
        let mut best_end = min_end;
        let mut best_score = usize::MAX;

        for end in min_end..=max_end {
            let dist = prefix[end].abs_diff(target);
            let start_ms = clamp_boundary_start(cue, overlapping[prev_end].start_ms);
            let end_ms = clamp_boundary_end(cue, overlapping[end - 1].end_ms);
            let duration_penalty = if end_ms - start_ms < MIN_SEGMENT_DURATION_MS {
                (MIN_SEGMENT_DURATION_MS - (end_ms - start_ms)) as usize * 4
            } else {
                0
            };
            let score = dist.saturating_mul(4).saturating_add(duration_penalty);
            if score < best_score {
                best_score = score;
                best_end = end;
            }
        }

        groups.push((prev_end, best_end));
        prev_end = best_end;
    }
    groups.push((prev_end, overlapping.len()));

    let mut out = Vec::<SubCue>::new();
    for (part_idx, (start_idx, end_idx)) in groups.into_iter().enumerate() {
        if start_idx >= end_idx {
            return None;
        }
        let start_ms = clamp_boundary_start(cue, overlapping[start_idx].start_ms);
        let end_ms = clamp_boundary_end(cue, overlapping[end_idx - 1].end_ms);
        if end_ms <= start_ms {
            return None;
        }
        out.push(SubCue {
            index: 0,
            start_ms,
            end_ms,
            text: normalize_part_text(&parts[part_idx]),
        });
    }

    Some(out)
}

fn clamp_boundary_start(cue: &SubCue, start_ms: i64) -> i64 {
    start_ms.clamp(cue.start_ms, cue.end_ms.saturating_sub(1))
}

fn clamp_boundary_end(cue: &SubCue, end_ms: i64) -> i64 {
    end_ms.clamp(cue.start_ms.saturating_add(1), cue.end_ms)
}

fn apply_approximate_timing(cue: &SubCue, parts: &[String]) -> Vec<SubCue> {
    let weights: Vec<i64> = parts
        .iter()
        .map(|p| visible_chars(p).max(1) as i64)
        .collect();
    let mut start = cue.start_ms;
    let mut out = Vec::new();
    let mut remaining_weight: i64 = weights.iter().sum::<i64>().max(1);

    for (idx, text) in parts.iter().enumerate() {
        let remaining_parts = parts.len() - idx - 1;
        let remaining_total = cue.end_ms - start;
        let end = if remaining_parts == 0 {
            cue.end_ms
        } else {
            let min_tail = (remaining_parts as i64 * MIN_SEGMENT_DURATION_MS)
                .min(remaining_total.saturating_sub(remaining_parts as i64))
                .max(remaining_parts as i64);
            let max_piece = (remaining_total - min_tail).max(1);
            let ideal = (remaining_total * weights[idx] / remaining_weight).max(1);
            let min_piece = MIN_SEGMENT_DURATION_MS.min(max_piece).max(1);
            start + ideal.clamp(min_piece, max_piece)
        };

        out.push(SubCue {
            index: 0,
            start_ms: start,
            end_ms: end.max(start + 1),
            text: normalize_part_text(text),
        });
        start = end.max(start + 1);
        remaining_weight = (remaining_weight - weights[idx]).max(1);
    }

    if let Some(last) = out.last_mut() {
        last.end_ms = cue.end_ms;
    }
    out
}

fn merge_short_cues(mut cues: Vec<SubCue>) -> Vec<SubCue> {
    let mut idx = 0usize;
    while cues.len() > 1 && idx < cues.len() {
        let short = cue_is_too_short(&cues[idx]);
        if !short {
            idx += 1;
            continue;
        }

        if idx == 0 {
            let next = cues.remove(1);
            cues[0].text = join_text_fragments(&cues[0].text, &next.text);
            cues[0].end_ms = next.end_ms;
        } else {
            let current = cues.remove(idx);
            let prev = &mut cues[idx - 1];
            prev.text = join_text_fragments(&prev.text, &current.text);
            prev.end_ms = current.end_ms;
            idx -= 1;
        }
    }
    cues
}

fn cue_is_too_short(cue: &SubCue) -> bool {
    (cue.end_ms - cue.start_ms) < MIN_SEGMENT_DURATION_MS
        || visible_chars(&cue.text) < MIN_SEGMENT_VISIBLE_CHARS
}

fn join_text_fragments(left: &str, right: &str) -> String {
    let left = normalize_part_text(left);
    let right = normalize_part_text(right);
    if left.is_empty() {
        return right;
    }
    if right.is_empty() {
        return left;
    }
    if right
        .chars()
        .next()
        .map(is_right_attached_punct)
        .unwrap_or(false)
    {
        format!("{left}{right}")
    } else {
        format!("{left} {right}")
    }
}

fn is_right_attached_punct(ch: char) -> bool {
    matches!(ch, ',' | '.' | '!' | '?' | ';' | ':' | ')' | ']' | '}')
}

fn normalize_part_text(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn visible_chars(s: &str) -> usize {
    s.chars()
        .filter(|ch| !ch.is_whitespace() && !ch.is_control())
        .count()
}

fn extract_time_units_from_whisper_json(path: &Path) -> Result<Vec<TimeUnit>, String> {
    let raw =
        std::fs::read_to_string(path).map_err(|e| format!("Failed to read Whisper JSON: {e}"))?;
    let value: Value =
        serde_json::from_str(&raw).map_err(|e| format!("Failed to parse Whisper JSON: {e}"))?;

    let mut units = Vec::<TimeUnit>::new();
    collect_time_units(&value, &mut units, true);
    if units.is_empty() {
        collect_time_units(&value, &mut units, false);
    }

    units.sort_by_key(|u| (u.start_ms, u.end_ms));
    units.dedup_by(|a, b| a.start_ms == b.start_ms && a.end_ms == b.end_ms && a.text == b.text);
    Ok(units)
}

fn collect_time_units(value: &Value, out: &mut Vec<TimeUnit>, words_only: bool) {
    match value {
        Value::Array(items) => {
            for item in items {
                collect_time_units(item, out, words_only);
            }
        }
        Value::Object(map) => {
            if let Some(unit) = parse_time_unit_object(value, words_only) {
                out.push(unit);
            }
            for child in map.values() {
                collect_time_units(child, out, words_only);
            }
        }
        _ => {}
    }
}

fn parse_time_unit_object(value: &Value, words_only: bool) -> Option<TimeUnit> {
    let Value::Object(map) = value else {
        return None;
    };

    let text = if words_only {
        map.get("word").and_then(|v| v.as_str())?
    } else {
        map.get("word")
            .and_then(|v| v.as_str())
            .or_else(|| map.get("text").and_then(|v| v.as_str()))?
    }
    .trim()
    .to_string();

    if text.is_empty() {
        return None;
    }

    let (start_ms, end_ms) = extract_times(value)?;
    (end_ms > start_ms).then_some(TimeUnit {
        start_ms,
        end_ms,
        text,
    })
}

fn extract_times(value: &Value) -> Option<(i64, i64)> {
    let get_num = |v: Option<&Value>| -> Option<f64> { v.and_then(|x| x.as_f64()) };
    let Value::Object(map) = value else {
        return None;
    };

    if let Some(offsets) = map.get("offsets").and_then(|v| v.as_object()) {
        let start = get_num(offsets.get("from").map(|v| v as &Value))
            .or_else(|| get_num(offsets.get("start").map(|v| v as &Value)))?;
        let end = get_num(offsets.get("to").map(|v| v as &Value))
            .or_else(|| get_num(offsets.get("end").map(|v| v as &Value)))?;
        return Some((normalize_time_value(start), normalize_time_value(end)));
    }

    let start = get_num(map.get("start"))
        .or_else(|| get_num(map.get("from")))
        .or_else(|| get_num(map.get("t0")))?;
    let end = get_num(map.get("end"))
        .or_else(|| get_num(map.get("to")))
        .or_else(|| get_num(map.get("t1")))?;
    Some((normalize_time_value(start), normalize_time_value(end)))
}

fn normalize_time_value(v: f64) -> i64 {
    if v.fract().abs() > f64::EPSILON {
        (v * 1000.0).round() as i64
    } else {
        v.round() as i64
    }
}

#[cfg(test)]
mod tests {
    use super::{
        apply_word_timing, normalize_part_text, snap_llm_cuts_to_boundaries, split_by_rules,
        TimeUnit,
    };
    use crate::infra::srt::SubCue;

    #[test]
    fn rules_split_does_not_break_words() {
        let text = "of what we actually did accomplished with the Node Editor tutorial series";
        let parts = split_by_rules(text, 3);
        assert!(parts
            .iter()
            .all(|p| !p.contains("accompl") || p.contains("accomplished")));
        assert_eq!(parts.join(" "), normalize_part_text(text));
    }

    #[test]
    fn llm_cuts_snap_to_safe_word_boundaries() {
        let text = "the series and if you haven't seen that";
        let cuts = snap_llm_cuts_to_boundaries(text, &[24]);
        let parts = super::split_by_safe_positions(text, &cuts);
        assert_eq!(
            parts,
            vec![
                "the series and if you".to_string(),
                "haven't seen that".to_string()
            ]
        );
    }

    #[test]
    fn word_timing_assigns_monotonic_nonzero_ranges() {
        let cue = SubCue {
            index: 1,
            start_ms: 1000,
            end_ms: 5000,
            text: "hello world this is subtitle".into(),
        };
        let parts = vec!["hello world".to_string(), "this is subtitle".to_string()];
        let units = vec![
            TimeUnit {
                start_ms: 1000,
                end_ms: 1500,
                text: "hello".into(),
            },
            TimeUnit {
                start_ms: 1500,
                end_ms: 2100,
                text: "world".into(),
            },
            TimeUnit {
                start_ms: 2100,
                end_ms: 2800,
                text: "this".into(),
            },
            TimeUnit {
                start_ms: 2800,
                end_ms: 3400,
                text: "is".into(),
            },
            TimeUnit {
                start_ms: 3400,
                end_ms: 5000,
                text: "subtitle".into(),
            },
        ];
        let out = apply_word_timing(&cue, &parts, &units).unwrap();
        assert_eq!(out.len(), 2);
        assert!(out[0].start_ms < out[0].end_ms);
        assert!(out[1].start_ms < out[1].end_ms);
        assert!(out[0].end_ms <= out[1].start_ms);
    }
}
