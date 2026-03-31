use std::path::Path;
use std::time::Duration;

use reqwest::blocking::Client;
use serde_json::{json, Value};

use super::openai_compat::chat_completions_url;
use super::srt::SubCue;

#[derive(Debug, Clone, PartialEq, Eq)]
struct TimeUnit {
    start_ms: i64,
    end_ms: i64,
    text: String,
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

    let mut next_index = 1u32;
    let mut out = Vec::new();
    let mut notes = Vec::<String>::new();

    for cue in cues {
        let should_split = cue_should_split(
            cue,
            job.max_chars_per_segment.max(1),
            job.max_duration_ms.max(1) as i64,
        );
        let parts = if should_split {
            match split_text_parts(client, job, cue) {
                Ok(parts) => {
                    parts
                }
                Err(e) => return Err(e),
            }
        } else {
            vec![cue.text.clone()]
        };

        let segmented = if parts.len() <= 1 {
            vec![SubCue {
                index: next_index,
                start_ms: cue.start_ms,
                end_ms: cue.end_ms,
                text: cue.text.clone(),
            }]
        } else if let Some(cues_with_time) =
            apply_word_timing(cue, &parts, &timing_units, &mut next_index)
        {
            cues_with_time
        } else {
            if job.timing_mode == "word_timestamps_first" {
                notes.push("分段时间已降级为近似对齐".into());
            }
            apply_approximate_timing(cue, &parts, &mut next_index)
        };

        out.extend(segmented);
    }

    Ok(SegmentationResult {
        cues: out,
        note: dedupe_notes(notes),
    })
}

fn dedupe_notes(notes: Vec<String>) -> Option<String> {
    let mut uniq = Vec::<String>::new();
    for note in notes {
        if !uniq.iter().any(|x| x == &note) {
            uniq.push(note);
        }
    }
    (!uniq.is_empty()).then(|| uniq.join("；"))
}

fn cue_should_split(cue: &SubCue, max_chars: u32, max_duration_ms: i64) -> bool {
    let char_count = cue
        .text
        .chars()
        .filter(|ch| !ch.is_control())
        .count() as u32;
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
                        let mut fallback = split_by_rules(
                            &cue.text,
                            desired_parts(cue, job.max_chars_per_segment, job.max_duration_ms),
                        );
                        if fallback.is_empty() {
                            fallback.push(cue.text.clone());
                        }
                        log::warn!("LLM 分段失败，回退规则分段: {e}");
                        Ok(fallback)
                    }
                }
            } else if job.strategy == "auto" {
                Ok(split_by_rules(
                    &cue.text,
                    desired_parts(cue, job.max_chars_per_segment, job.max_duration_ms),
                ))
            } else {
                Err("未配置可用 LLM，无法执行 LLM 优先分段".into())
            }
        }
        _ => Ok(vec![cue.text.clone()]),
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
    by_chars.max(by_duration).max(1)
}

fn split_by_rules(text: &str, desired_parts: usize) -> Vec<String> {
    let char_count = text.chars().count();
    if desired_parts <= 1 || char_count < 2 {
        return vec![text.to_string()];
    }
    let chars: Vec<char> = text.chars().collect();
    let mut cuts = Vec::<usize>::new();
    for part_idx in 1..desired_parts {
        let target = char_count * part_idx / desired_parts;
        let cut = find_cut_near(&chars, target, &cuts);
        if cut > 0 && cut < char_count && cuts.last().copied() != Some(cut) {
            cuts.push(cut);
        }
    }
    cuts.sort_unstable();
    split_by_char_positions(text, &cuts)
}

fn find_cut_near(chars: &[char], target: usize, existing: &[usize]) -> usize {
    let min_bound = existing.last().copied().unwrap_or(0) + 1;
    let max_bound = chars.len().saturating_sub(1);
    if min_bound >= max_bound {
        return target.clamp(min_bound, max_bound);
    }

    let scan_radius = (chars.len() / 8).max(6);
    let start = target.saturating_sub(scan_radius).max(min_bound);
    let end = (target + scan_radius).min(max_bound);
    let mut best = None::<(usize, usize)>;

    for i in start..=end {
        let prev = chars[i - 1];
        let curr = chars[i];
        let strong = is_strong_boundary(prev, curr);
        let soft = strong || is_soft_boundary(prev, curr);
        if soft {
            let dist = i.abs_diff(target);
            let score = if strong { dist } else { dist + 4 };
            if best.map(|(_, s)| score < s).unwrap_or(true) {
                best = Some((i, score));
            }
        }
    }

    best.map(|(i, _)| i)
        .unwrap_or_else(|| target.clamp(min_bound, max_bound))
}

fn is_strong_boundary(prev: char, curr: char) -> bool {
    matches!(prev, '，' | '。' | '！' | '？' | '；' | ',' | '.' | '!' | '?' | ';' | ':' | '：')
        || (prev == '\n' || curr == '\n')
}

fn is_soft_boundary(prev: char, curr: char) -> bool {
    prev.is_whitespace() || curr.is_whitespace()
}

fn split_by_char_positions(text: &str, cuts: &[usize]) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut start = 0usize;
    for &cut in cuts {
        if cut <= start || cut >= chars.len() {
            continue;
        }
        out.push(chars[start..cut].iter().collect::<String>());
        start = cut;
    }
    out.push(chars[start..].iter().collect::<String>());
    out.into_iter().filter(|s| !s.is_empty()).collect()
}

fn split_by_llm(
    client: &Client,
    job: &SegmentationJob<'_>,
    cue: &SubCue,
) -> Result<Vec<String>, String> {
    let char_len = cue.text.chars().count();
    if char_len < 2 {
        return Ok(vec![cue.text.clone()]);
    }
    let desired = desired_parts(cue, job.max_chars_per_segment, job.max_duration_ms);
    let sys = "你是字幕断句助手。你的任务是仅返回断句切分位置，不改写任何文本，不解释。";
    let user = format!(
        "给定一段字幕文本，请返回 JSON 数组，元素为从 0 开始的 Unicode 字符切分位置索引。\n\
要求：\n\
1. 只在自然语义边界切分。\n\
2. 不要返回 0，也不要返回总长度 {char_len}。\n\
3. 目标切成 {desired} 段左右；若原句无需强切，可返回更少切点。\n\
4. 输出只能是 JSON 数组，例如 [12, 24]。\n\
5. 不得改写原文。\n\
原文：\n{}",
        cue.text
    );
    let body = json!({
        "model": job.llm_model,
        "messages": [
            {"role": "system", "content": sys},
            {"role": "user", "content": user}
        ],
        "temperature": 0.1,
        "max_tokens": 512,
    });
    let url = chat_completions_url(job.llm_base_url);
    let resp = client
        .post(url)
        .timeout(Duration::from_secs(job.llm_timeout_sec.max(5)))
        .header("Authorization", format!("Bearer {}", job.llm_api_key.trim()))
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .map_err(|e| format!("LLM 分段请求失败: {e}"))?;
    let status = resp.status();
    let text = resp.text().unwrap_or_default();
    if !status.is_success() {
        return Err(format!(
            "LLM 分段 HTTP {}: {}",
            status.as_u16(),
            text.chars().take(300).collect::<String>()
        ));
    }
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("LLM 分段响应非 JSON: {e}"))?;
    let content = v
        .pointer("/choices/0/message/content")
        .and_then(|x| x.as_str())
        .ok_or_else(|| "LLM 分段响应缺少 content".to_string())?;
    let cuts = parse_cut_positions(content, char_len)?;
    let parts = split_by_char_positions(&cue.text, &cuts);
    if parts.len() <= 1 {
        return Ok(vec![cue.text.clone()]);
    }
    Ok(parts)
}

fn parse_cut_positions(content: &str, char_len: usize) -> Result<Vec<usize>, String> {
    let trimmed = strip_json_fence(content);
    let mut cuts: Vec<usize> =
        serde_json::from_str(&trimmed).map_err(|e| format!("LLM 分段 JSON 解析失败: {e}"))?;
    cuts.sort_unstable();
    cuts.dedup();
    if cuts.iter().any(|&c| c == 0 || c >= char_len) {
        return Err("LLM 分段切点超出范围".into());
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

fn apply_word_timing(
    cue: &SubCue,
    parts: &[String],
    units: &[TimeUnit],
    next_index: &mut u32,
) -> Option<Vec<SubCue>> {
    let overlapping: Vec<&TimeUnit> = units
        .iter()
        .filter(|u| u.start_ms >= cue.start_ms && u.end_ms <= cue.end_ms)
        .collect();
    if overlapping.len() < 2 {
        return None;
    }
    let total_chars: usize = overlapping
        .iter()
        .map(|u| visible_chars(&u.text).max(1))
        .sum();
    if total_chars == 0 {
        return None;
    }
    let part_chars: Vec<usize> = parts.iter().map(|p| visible_chars(p).max(1)).collect();
    let sum_part_chars: usize = part_chars.iter().sum();
    if sum_part_chars == 0 {
        return None;
    }

    let mut boundaries = Vec::<i64>::new();
    let mut part_acc = 0usize;
    let mut unit_chars_acc = 0usize;
    let mut unit_idx = 0usize;
    for part_char in part_chars.iter().take(part_chars.len().saturating_sub(1)) {
        part_acc += *part_char;
        let target = part_acc * total_chars / sum_part_chars;
        while unit_idx < overlapping.len() {
            unit_chars_acc += visible_chars(&overlapping[unit_idx].text).max(1);
            let boundary = clamp_boundary(cue, overlapping[unit_idx].end_ms);
            unit_idx += 1;
            if unit_chars_acc >= target || unit_idx == overlapping.len() {
                boundaries.push(boundary);
                break;
            }
        }
    }
    if boundaries.len() + 1 != parts.len() {
        return None;
    }
    let mut start = cue.start_ms;
    let mut out = Vec::new();
    for (idx, text) in parts.iter().enumerate() {
        let end = if idx + 1 == parts.len() {
            cue.end_ms
        } else {
            boundaries[idx].max(start + 1)
        };
        out.push(SubCue {
            index: *next_index,
            start_ms: start,
            end_ms: end,
            text: text.clone(),
        });
        *next_index += 1;
        start = end;
    }
    Some(out)
}

fn apply_approximate_timing(
    cue: &SubCue,
    parts: &[String],
    next_index: &mut u32,
) -> Vec<SubCue> {
    let total = (cue.end_ms - cue.start_ms).max(1);
    let weights: Vec<i64> = parts
        .iter()
        .map(|p| visible_chars(p).max(1) as i64)
        .collect();
    let weight_sum: i64 = weights.iter().sum::<i64>().max(1);
    let mut start = cue.start_ms;
    let mut out = Vec::new();
    for (idx, text) in parts.iter().enumerate() {
        let end = if idx + 1 == parts.len() {
            cue.end_ms
        } else {
            let piece = (total * weights[idx] / weight_sum).max(80);
            (start + piece).min(cue.end_ms - ((parts.len() - idx - 1) as i64))
        };
        out.push(SubCue {
            index: *next_index,
            start_ms: start,
            end_ms: end.max(start + 1),
            text: text.clone(),
        });
        *next_index += 1;
        start = end.max(start + 1);
    }
    if let Some(last) = out.last_mut() {
        last.end_ms = cue.end_ms;
    }
    out
}

fn visible_chars(s: &str) -> usize {
    s.chars().filter(|ch| !ch.is_whitespace() && !ch.is_control()).count()
}

fn clamp_boundary(cue: &SubCue, boundary: i64) -> i64 {
    let min = cue.start_ms + 1;
    let max = (cue.end_ms - 1).max(min);
    boundary.clamp(min, max)
}

fn extract_time_units_from_whisper_json(path: &Path) -> Result<Vec<TimeUnit>, String> {
    let raw = std::fs::read_to_string(path).map_err(|e| format!("读取 Whisper JSON 失败: {e}"))?;
    let value: Value = serde_json::from_str(&raw).map_err(|e| format!("解析 Whisper JSON 失败: {e}"))?;
    let mut units = Vec::<TimeUnit>::new();
    collect_time_units(&value, &mut units);
    units.sort_by_key(|u| (u.start_ms, u.end_ms));
    units.dedup_by(|a, b| a.start_ms == b.start_ms && a.end_ms == b.end_ms && a.text == b.text);
    Ok(units)
}

fn collect_time_units(value: &Value, out: &mut Vec<TimeUnit>) {
    match value {
        Value::Array(items) => {
            for item in items {
                collect_time_units(item, out);
            }
        }
        Value::Object(map) => {
            if let Some(unit) = parse_time_unit_object(value) {
                out.push(unit);
            }
            for child in map.values() {
                collect_time_units(child, out);
            }
        }
        _ => {}
    }
}

fn parse_time_unit_object(value: &Value) -> Option<TimeUnit> {
    let Value::Object(map) = value else {
        return None;
    };
    let text = map
        .get("word")
        .and_then(|v| v.as_str())
        .or_else(|| map.get("text").and_then(|v| v.as_str()))?
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
    use super::{apply_approximate_timing, split_by_rules};
    use crate::infra::srt::SubCue;

    #[test]
    fn rules_split_long_text() {
        let parts = split_by_rules("今天我们来讲这个问题，然后再看下一部分内容。", 2);
        assert!(parts.len() >= 2);
    }

    #[test]
    fn approximate_timing_preserves_range() {
        let cue = SubCue {
            index: 1,
            start_ms: 1000,
            end_ms: 5000,
            text: "hello world this is subtitle".into(),
        };
        let mut next = 1;
        let out = apply_approximate_timing(
            &cue,
            &["hello world".into(), "this is subtitle".into()],
            &mut next,
        );
        assert_eq!(out.len(), 2);
        assert_eq!(out.first().unwrap().start_ms, 1000);
        assert_eq!(out.last().unwrap().end_ms, 5000);
        assert!(out[0].end_ms <= out[1].start_ms);
    }
}
