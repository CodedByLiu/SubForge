//! LLM batch translation with JSON alignment validation, retries, splitting,
//! and per-item fallback to source text.

use std::collections::HashMap;
use std::time::Duration;

use reqwest::blocking::Client;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::domain::config::GlossaryEntry;
use crate::infra::openai_compat::chat_completions_url;
use crate::infra::runner_limits::LlmRequestSlots;
use crate::infra::srt::SubCue;

#[derive(Debug, Deserialize)]
struct LlmSeg {
    id: u64,
    text: String,
}

enum BatchParse {
    Complete(Vec<String>),
    Partial {
        found: Vec<(u32, String)>,
        missing: Vec<u32>,
    },
}

struct PartialBatchAttempt {
    found: Vec<(usize, String)>,
    missing: Vec<usize>,
}

pub struct TranslateJob<'a> {
    pub base_url: &'a str,
    pub model: &'a str,
    pub api_key: &'a str,
    pub timeout_sec: u64,
    pub max_retries_per_batch: u32,
    pub min_interval_ms: u64,
    pub source_lang: &'a str,
    pub target_lang: &'a str,
    pub style: &'a str,
    pub keep_proper_nouns: bool,
    pub glossary: &'a [GlossaryEntry],
    pub glossary_case_sensitive: bool,
}

fn style_instruction(style: &str) -> &'static str {
    match style {
        "literal" => {
            "Use a literal translation style. Preserve wording, syntax, and sentence structure when possible."
        }
        "natural" => {
            "Use a natural spoken style in the target language, but do not omit important meaning."
        }
        _ => {
            "Use a technical tutorial translation style. Prioritize terminology consistency, information completeness, and instructional clarity over brevity."
        }
    }
}

fn fidelity_instruction(style: &str) -> &'static str {
    match style {
        "natural" => {
            "Keep the translation fluent, but do not summarize, soften, or skip technical details."
        }
        "literal" => {
            "Do not rewrite into a summary. Keep the full meaning of every subtitle item."
        }
        _ => {
            "Do not simplify into a summary. Preserve technical details, qualifiers, relationships, and step-by-step explanations. If the source mentions classes, methods, variables, file names, UI labels, or code terms, keep them accurate and stable across items."
        }
    }
}

fn identifier_instruction(style: &str) -> &'static str {
    match style {
        "natural" => {
            "For code identifiers and product names, prefer preserving the original spelling when translation would reduce precision."
        }
        _ => {
            "Preserve code identifiers, API names, class names, method names, variable names, and file names exactly when appropriate. Do not replace precise technical terms with vague Chinese paraphrases."
        }
    }
}

fn glossary_block(entries: &[GlossaryEntry], case_sensitive: bool) -> String {
    if entries.is_empty() {
        return String::new();
    }
    let mut s = if case_sensitive {
        String::from("Glossary rules: match whole words, case-sensitive.\n")
    } else {
        String::from("Glossary rules: match whole words, case-insensitive.\n")
    };
    for e in entries {
        if e.source.trim().is_empty() {
            continue;
        }
        s.push_str(&format!(
            "- \"{}\" -> \"{}\"\n",
            e.source.trim(),
            e.target.trim()
        ));
    }
    s
}

fn strip_json_fence(s: &str) -> String {
    let t = s.trim();
    if let Some(i) = t.find('[') {
        if let Some(j) = t.rfind(']') {
            return t[i..=j].to_string();
        }
    }
    if let Some(rest) = t.strip_prefix("```json") {
        return rest
            .trim()
            .trim_end_matches('`')
            .trim()
            .trim_start_matches("json")
            .trim()
            .to_string();
    }
    if let Some(rest) = t.strip_prefix("```") {
        return rest.trim().trim_end_matches('`').trim().to_string();
    }
    t.to_string()
}

fn parse_segments(content: &str) -> Result<Vec<LlmSeg>, String> {
    let cleaned = strip_json_fence(content);
    serde_json::from_str(&cleaned).map_err(|e| {
        format!(
            "JSON parse failed: {e}; snippet: {}",
            cleaned.chars().take(200).collect::<String>()
        )
    })
}

#[cfg(test)]
fn parse_batch_json(content: &str, expected_ids: &[u32]) -> Result<Vec<String>, String> {
    match parse_batch_partial(content, expected_ids)? {
        BatchParse::Complete(items) => Ok(items),
        BatchParse::Partial { found, missing } => Err(format!(
            "Returned {} items, expected {}; missing ids={missing:?}",
            found.len(),
            expected_ids.len()
        )),
    }
}

fn parse_batch_partial(content: &str, expected_ids: &[u32]) -> Result<BatchParse, String> {
    let items = parse_segments(content)?;
    let mut map: HashMap<u32, String> = HashMap::new();
    for it in items {
        if map.insert(it.id as u32, it.text).is_some() {
            return Err("Duplicate id in JSON response".into());
        }
    }

    let expected_lookup: HashMap<u32, usize> = expected_ids
        .iter()
        .copied()
        .enumerate()
        .map(|(idx, id)| (id, idx))
        .collect();
    let unexpected: Vec<u32> = map
        .keys()
        .copied()
        .filter(|id| !expected_lookup.contains_key(id))
        .collect();
    if !unexpected.is_empty() {
        return Err(format!("Unexpected ids in JSON response: {unexpected:?}"));
    }

    let mut found = Vec::new();
    let mut missing = Vec::new();
    for id in expected_ids {
        if let Some(text) = map.remove(id) {
            found.push((*id, text));
        } else {
            missing.push(*id);
        }
    }

    if missing.is_empty() {
        return Ok(BatchParse::Complete(
            found.into_iter().map(|(_, text)| text).collect(),
        ));
    }

    Ok(BatchParse::Partial { found, missing })
}

fn extract_content(resp_json: &Value) -> Result<String, String> {
    resp_json
        .pointer("/choices/0/message/content")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "Response missing choices[0].message.content".into())
}

fn call_chat(
    client: &Client,
    url: &str,
    api_key: &str,
    model: &str,
    system: &str,
    user: &str,
    timeout: Duration,
) -> Result<String, String> {
    let body = json!({
        "model": model,
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": user}
        ],
        "temperature": 0.2,
        "max_tokens": 8192,
    });
    let resp = client
        .post(url)
        .timeout(timeout)
        .header("Authorization", format!("Bearer {}", api_key.trim()))
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .map_err(|e| format!("Network request failed: {e}"))?;
    let status = resp.status();
    let text = resp.text().unwrap_or_default();
    if !status.is_success() {
        return Err(format!(
            "HTTP {}: {}",
            status.as_u16(),
            text.chars().take(500).collect::<String>()
        ));
    }
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("Response is not JSON: {e}"))?;
    extract_content(&v)
}

fn build_batches(cues: &[SubCue], max_segment_chars: u32) -> Vec<Vec<usize>> {
    let max_c = max_segment_chars.max(64);
    let mut batches: Vec<Vec<usize>> = Vec::new();
    let mut cur: Vec<usize> = Vec::new();
    let mut size = 0u32;
    for (i, c) in cues.iter().enumerate() {
        let n = c.text.chars().count() as u32;
        let add = n.max(1);
        if !cur.is_empty() && size + add > max_c {
            batches.push(cur);
            cur = Vec::new();
            size = 0;
        }
        cur.push(i);
        size += add;
    }
    if !cur.is_empty() {
        batches.push(cur);
    }
    batches
}

fn translate_indices_once(
    client: &Client,
    job: &TranslateJob,
    cues: &[SubCue],
    indices: &[usize],
    context_hint: Option<&str>,
    sleep_before: bool,
    llm_slots: Option<&LlmRequestSlots>,
    llm_cap: u32,
) -> Result<BatchParse, String> {
    if sleep_before && job.min_interval_ms > 0 {
        std::thread::sleep(Duration::from_millis(job.min_interval_ms));
    }
    let _llm_permit = llm_slots.map(|s| s.acquire(llm_cap));

    let ids: Vec<u32> = indices.iter().map(|&i| cues[i].index).collect();
    let payload: Vec<Value> = indices
        .iter()
        .map(|&i| {
            json!({
                "id": cues[i].index,
                "text": cues[i].text
            })
        })
        .collect();

    let sys = format!(
        "You are a subtitle translation assistant. Output JSON only, no markdown, no explanation.\n\
Target language code: {}\n\
Source language hint: {}\n\
{}\n\
{}\n\
{}\n\
{}\n\
{}",
        job.target_lang,
        job.source_lang,
        style_instruction(job.style),
        fidelity_instruction(job.style),
        identifier_instruction(job.style),
        if job.keep_proper_nouns {
            "Preserve proper nouns when appropriate."
        } else {
            ""
        },
        glossary_block(job.glossary, job.glossary_case_sensitive)
    );

    let mut user = String::from(
        "Translate each subtitle item into the target language.\n\
Return a JSON array where each item is {\"id\": number, \"text\": \"translation\"}.\n\
The number of items and every id must match the input exactly.\n\
Translate each item faithfully. Do not omit information just to make it shorter.\n\
Do not merge explanation into a vague summary. Keep the original instructional intent.\n\
Input:\n",
    );
    user.push_str(&serde_json::to_string(&payload).map_err(|e| e.to_string())?);
    if let Some(ctx) = context_hint.filter(|s| !s.trim().is_empty()) {
        user.push_str("\n\nPrevious translated context for tone only:\n");
        user.push_str(ctx);
    }

    let url = chat_completions_url(job.base_url);
    let timeout = Duration::from_secs(job.timeout_sec.max(5));
    let content = call_chat(client, &url, job.api_key, job.model, &sys, &user, timeout)?;
    parse_batch_partial(&content, &ids)
}

fn translate_indices_recursive(
    client: &Client,
    job: &TranslateJob,
    cues: &[SubCue],
    indices: &[usize],
    context_hint: Option<&str>,
    sleep_next: &mut bool,
    llm_slots: Option<&LlmRequestSlots>,
    llm_cap: u32,
) -> (Vec<String>, bool) {
    if indices.is_empty() {
        return (Vec::new(), false);
    }

    if indices.len() == 1 {
        let i = indices[0];
        let orig = cues[i].text.clone();
        for _attempt in 0..=job.max_retries_per_batch {
            match translate_indices_once(
                client,
                job,
                cues,
                indices,
                context_hint,
                *sleep_next,
                llm_slots,
                llm_cap,
            ) {
                Ok(BatchParse::Complete(v)) if v.len() == 1 => {
                    *sleep_next = true;
                    return (v, false);
                }
                Ok(BatchParse::Partial { .. }) => {}
                Err(e) => {
                    log::warn!("Translation batch failed: {e}");
                }
                _ => {}
            }
            *sleep_next = true;
        }
        return (vec![orig], true);
    }

    let mut best_partial: Option<PartialBatchAttempt> = None;
    for _attempt in 0..=job.max_retries_per_batch {
        match translate_indices_once(
            client,
            job,
            cues,
            indices,
            context_hint,
            *sleep_next,
            llm_slots,
            llm_cap,
        ) {
            Ok(BatchParse::Complete(v)) if v.len() == indices.len() => {
                *sleep_next = true;
                return (v, false);
            }
            Ok(BatchParse::Partial { found, missing }) => {
                log::warn!(
                    "Translation batch returned partial result: {} / {}, missing ids={missing:?}",
                    found.len(),
                    indices.len()
                );
                let found_lookup: HashMap<u32, usize> = indices
                    .iter()
                    .map(|&idx| (cues[idx].index, idx))
                    .collect();
                let found_indices: Vec<(usize, String)> = found
                    .into_iter()
                    .filter_map(|(id, text)| found_lookup.get(&id).copied().map(|idx| (idx, text)))
                    .collect();
                let missing_indices: Vec<usize> = missing
                    .into_iter()
                    .filter_map(|id| found_lookup.get(&id).copied())
                    .collect();
                if !found_indices.is_empty()
                    && best_partial
                        .as_ref()
                        .map(|best| found_indices.len() > best.found.len())
                        .unwrap_or(true)
                {
                    best_partial = Some(PartialBatchAttempt {
                        found: found_indices,
                        missing: missing_indices,
                    });
                }
            }
            Err(e) => {
                log::warn!("Translation batch failed: {e}");
            }
            _ => {}
        }
        *sleep_next = true;
    }

    if let Some(best) = best_partial {
        let hint_text_owned = best
            .found
            .iter()
            .max_by_key(|(idx, _)| *idx)
            .map(|(_, text)| text.clone());
        let mut found_map: HashMap<usize, String> = best.found.into_iter().collect();
        let hint_text = hint_text_owned.as_deref().or(context_hint);
        let (repaired, fb) = translate_indices_recursive(
            client,
            job,
            cues,
            &best.missing,
            hint_text,
            sleep_next,
            llm_slots,
            llm_cap,
        );
        for (idx, text) in best.missing.iter().copied().zip(repaired.into_iter()) {
            found_map.insert(idx, text);
        }
        let mut merged = Vec::with_capacity(indices.len());
        for idx in indices {
            if let Some(text) = found_map.remove(idx) {
                merged.push(text);
            } else {
                merged.push(cues[*idx].text.clone());
            }
        }
        return (merged, fb);
    }

    let mid = indices.len() / 2;
    let (left, fb1) = translate_indices_recursive(
        client,
        job,
        cues,
        &indices[..mid],
        context_hint,
        sleep_next,
        llm_slots,
        llm_cap,
    );
    let tail = left.last().map(|s| s.as_str());
    let (right, fb2) = translate_indices_recursive(
        client,
        job,
        cues,
        &indices[mid..],
        tail,
        sleep_next,
        llm_slots,
        llm_cap,
    );
    let mut out = left;
    out.extend(right);
    (out, fb1 || fb2)
}

/// Returns translations with the same length as `cues`.
/// `any_fallback` is true if some items finally fell back to source text.
/// If `pause_requested` returns true between batches, returns `Err("__pause__")`.
pub fn translate_all_cues(
    client: &Client,
    job: &TranslateJob,
    cues: &[SubCue],
    max_segment_chars: u32,
    mut pause_requested: impl FnMut() -> bool,
    mut on_progress: impl FnMut(usize, usize),
    llm_slots: Option<&LlmRequestSlots>,
    llm_concurrency_cap: u32,
) -> Result<(Vec<String>, bool), String> {
    if cues.is_empty() {
        return Ok((Vec::new(), false));
    }
    if job.base_url.trim().is_empty() {
        return Err("LLM Base URL is empty".into());
    }
    if job.model.trim().is_empty() {
        return Err("LLM model is empty".into());
    }
    if job.api_key.trim().is_empty() {
        return Err("LLM API Key is not configured".into());
    }

    let batches = build_batches(cues, max_segment_chars);
    let mut out = vec![String::new(); cues.len()];
    let mut any_fb = false;
    let mut done = 0usize;
    let mut sleep_next = false;
    let mut last_tail: Option<String> = None;

    for batch in batches {
        if pause_requested() {
            return Err("__pause__".into());
        }
        let ctx = last_tail.as_deref();
        let (parts, fb) = translate_indices_recursive(
            client,
            job,
            cues,
            &batch,
            ctx,
            &mut sleep_next,
            llm_slots,
            llm_concurrency_cap,
        );
        any_fb |= fb;
        for (&i, t) in batch.iter().zip(parts.iter()) {
            out[i] = t.clone();
        }
        done += batch.len();
        last_tail = batch.last().map(|&i| out[i].clone());
        on_progress(done, cues.len());
    }

    Ok((out, any_fb))
}

#[cfg(test)]
mod tests {
    use crate::infra::srt::SubCue;

    use super::{build_batches, parse_batch_json, parse_batch_partial, BatchParse};

    #[test]
    fn parse_batch_json_accepts_expected_payload() {
        let parsed = parse_batch_json(
            r#"[{"id":1,"text":"你好"},{"id":2,"text":"世界"}]"#,
            &[1, 2],
        )
        .unwrap();
        assert_eq!(parsed, vec!["你好".to_string(), "世界".to_string()]);
    }

    #[test]
    fn parse_batch_json_rejects_wrong_count() {
        let err = parse_batch_json(r#"[{"id":1,"text":"你好"}]"#, &[1, 2]).unwrap_err();
        assert!(err.contains("expected"));
    }

    #[test]
    fn parse_batch_json_rejects_duplicate_id() {
        let err = parse_batch_json(
            r#"[{"id":1,"text":"你好"},{"id":1,"text":"世界"}]"#,
            &[1, 2],
        )
        .unwrap_err();
        assert!(err.contains("Duplicate"));
    }

    #[test]
    fn parse_batch_partial_accepts_missing_items() {
        let parsed = parse_batch_partial(r#"[{"id":1,"text":"你好"}]"#, &[1, 2]).unwrap();
        match parsed {
            BatchParse::Partial { found, missing } => {
                assert_eq!(found, vec![(1, "你好".to_string())]);
                assert_eq!(missing, vec![2]);
            }
            BatchParse::Complete(_) => panic!("expected partial"),
        }
    }

    #[test]
    fn build_batches_splits_large_segments() {
        let cues = vec![
            SubCue {
                index: 1,
                start_ms: 0,
                end_ms: 1000,
                text: "a".repeat(80),
            },
            SubCue {
                index: 2,
                start_ms: 1000,
                end_ms: 2000,
                text: "b".repeat(80),
            },
            SubCue {
                index: 3,
                start_ms: 2000,
                end_ms: 3000,
                text: "c".repeat(20),
            },
        ];
        let batches = build_batches(&cues, 100);
        assert_eq!(batches, vec![vec![0], vec![1, 2]]);
    }
}
