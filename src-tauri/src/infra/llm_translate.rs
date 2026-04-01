//! LLM 分批翻译：JSON 对齐校验、重试、减半、逐条回退原文

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
        "literal" => "采用直译，尽量保持原句结构与语序。",
        "natural" => "采用自然、口语化的目标语言表达，可读性优先。",
        _ => "术语表中的词若出现，译文必须使用术语表给定译法；其余内容准确翻译。",
    }
}

fn glossary_block(entries: &[GlossaryEntry], case_sensitive: bool) -> String {
    if entries.is_empty() {
        return String::new();
    }
    let mut s = String::from("术语表规则：整词匹配");
    s.push_str(if case_sensitive {
        "（区分大小写）。"
    } else {
        "（不区分大小写）。"
    });
    s.push_str("下列术语在原文中以完整词出现时，译文须使用对应译法：\n");
    for e in entries {
        if e.source.trim().is_empty() {
            continue;
        }
        s.push_str(&format!(
            "- \"{}\" → \"{}\"\n",
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

fn parse_batch_json(content: &str, expected_ids: &[u32]) -> Result<Vec<String>, String> {
    let cleaned = strip_json_fence(content);
    let items: Vec<LlmSeg> = serde_json::from_str(&cleaned).map_err(|e| {
        format!(
            "JSON 解析失败: {e}；片段: {}",
            cleaned.chars().take(200).collect::<String>()
        )
    })?;
    if items.len() != expected_ids.len() {
        return Err(format!(
            "返回 {} 条，期望 {} 条",
            items.len(),
            expected_ids.len()
        ));
    }
    let mut map: HashMap<u32, String> = HashMap::new();
    for it in items {
        map.insert(it.id as u32, it.text);
    }
    if map.len() != expected_ids.len() {
        return Err("返回 JSON 中存在重复 id".into());
    }
    let mut exp_sorted = expected_ids.to_vec();
    exp_sorted.sort_unstable();
    let mut got: Vec<u32> = map.keys().copied().collect();
    got.sort_unstable();
    if got != exp_sorted {
        return Err(format!("编号集合不匹配: 期望 {exp_sorted:?} 得到 {got:?}"));
    }
    let mut ordered = Vec::with_capacity(expected_ids.len());
    for id in expected_ids {
        let t = map.remove(id).ok_or_else(|| format!("缺少 id={id}"))?;
        ordered.push(t);
    }
    Ok(ordered)
}

fn extract_content(resp_json: &Value) -> Result<String, String> {
    resp_json
        .pointer("/choices/0/message/content")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "响应缺少 choices[0].message.content".into())
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
        .map_err(|e| format!("网络请求失败: {e}"))?;
    let status = resp.status();
    let text = resp.text().unwrap_or_default();
    if !status.is_success() {
        return Err(format!(
            "HTTP {}: {}",
            status.as_u16(),
            text.chars().take(500).collect::<String>()
        ));
    }
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("响应非 JSON: {e}"))?;
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
) -> Result<Vec<String>, String> {
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
        "你是字幕翻译助手。只输出 JSON 数组，不要 Markdown、不要解释。\n\
         目标语言代码: {}\n\
         源语言参考: {}\n\
         {}\n\
         {}\n\
         {}",
        job.target_lang,
        job.source_lang,
        style_instruction(job.style),
        if job.keep_proper_nouns {
            "保留人名、作品名等专有名词时可保留原文或通用译名，不要臆造。"
        } else {
            ""
        },
        glossary_block(job.glossary, job.glossary_case_sensitive)
    );
    let mut user = String::from(
        "将下列字幕逐条翻译为目标语言。输出格式：JSON 数组，元素为 {\"id\":序号,\"text\":\"译文\"}，\
必须与输入条数、id 完全一致。\n输入：\n",
    );
    user.push_str(&serde_json::to_string(&payload).map_err(|e| e.to_string())?);
    if let Some(ctx) = context_hint.filter(|s| !s.trim().is_empty()) {
        user.push_str("\n\n前文参考（仅连贯语气，不要翻译本字段）：\n");
        user.push_str(ctx);
    }
    let url = chat_completions_url(job.base_url);
    let timeout = Duration::from_secs(job.timeout_sec.max(5));
    let content = call_chat(client, &url, job.api_key, job.model, &sys, &user, timeout)?;
    parse_batch_json(&content, &ids)
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
                Ok(v) if v.len() == 1 => {
                    *sleep_next = true;
                    return (v, false);
                }
                _ => {}
            }
            *sleep_next = true;
        }
        (vec![orig], true)
    } else {
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
                Ok(v) if v.len() == indices.len() => {
                    *sleep_next = true;
                    return (v, false);
                }
                Ok(v) => {
                    log::warn!("翻译批次条数异常: 期望 {} 得到 {}", indices.len(), v.len());
                }
                Err(e) => {
                    log::warn!("翻译批次失败: {e}");
                }
            }
            *sleep_next = true;
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
}

/// 返回与 `cues` 等长的译文；`any_fallback` 表示存在逐条回退原文。  
/// `pause_requested` 在批次间隙调用，返回 `true` 时中止并返回 `Err("__pause__")`。
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
        return Err("LLM Base URL 为空".into());
    }
    if job.model.trim().is_empty() {
        return Err("LLM 模型名为空".into());
    }
    if job.api_key.trim().is_empty() {
        return Err("未配置 LLM API Key".into());
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

    use super::{build_batches, parse_batch_json};

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
        assert!(err.contains("期望"));
    }

    #[test]
    fn parse_batch_json_rejects_duplicate_id() {
        let err = parse_batch_json(
            r#"[{"id":1,"text":"你好"},{"id":1,"text":"世界"}]"#,
            &[1, 2],
        )
        .unwrap_err();
        assert!(err.contains("id"));
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
