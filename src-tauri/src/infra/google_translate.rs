use std::time::Duration;

use reqwest::blocking::Client;
use serde_json::Value;

use super::srt::SubCue;

const DEFAULT_GOOGLE_WEB_URL: &str = "https://translate.googleapis.com/translate_a/single";

pub struct GoogleWebTranslateJob<'a> {
    pub provider_url: &'a str,
    pub min_interval_ms: u64,
    pub source_lang: &'a str,
    pub target_lang: &'a str,
}

pub fn build_google_client(use_proxy: bool) -> Result<Client, String> {
    let mut builder = Client::builder().timeout(Duration::from_secs(30));
    if !use_proxy {
        builder = builder.no_proxy();
    }
    builder
        .build()
        .map_err(|e| format!("Google 翻译 HTTP 客户端初始化失败: {e}"))
}

pub fn translate_all_cues_google(
    client: &Client,
    job: &GoogleWebTranslateJob<'_>,
    cues: &[SubCue],
    mut pause_requested: impl FnMut() -> bool,
    mut on_progress: impl FnMut(usize, usize),
) -> Result<Vec<String>, String> {
    if cues.is_empty() {
        return Ok(Vec::new());
    }
    let url = effective_google_url(job.provider_url);
    let sl = normalize_lang(job.source_lang);
    let tl = normalize_lang(job.target_lang);
    if tl == "auto" {
        return Err("Google 翻译目标语言不能为空".into());
    }

    let mut out = Vec::with_capacity(cues.len());
    for (idx, cue) in cues.iter().enumerate() {
        if pause_requested() {
            return Err("__pause__".into());
        }
        if idx > 0 && job.min_interval_ms > 0 {
            std::thread::sleep(Duration::from_millis(job.min_interval_ms));
        }
        let translated = translate_one(client, &url, &sl, &tl, &cue.text)?;
        out.push(translated);
        on_progress(idx + 1, cues.len());
    }
    Ok(out)
}

fn effective_google_url(provider_url: &str) -> String {
    let trimmed = provider_url.trim();
    if trimmed.is_empty() {
        DEFAULT_GOOGLE_WEB_URL.to_string()
    } else {
        trimmed.to_string()
    }
}

fn normalize_lang(lang: &str) -> String {
    let trimmed = lang.trim().to_ascii_lowercase();
    if trimmed.is_empty() {
        "auto".into()
    } else {
        trimmed
    }
}

fn translate_one(
    client: &Client,
    url: &str,
    source_lang: &str,
    target_lang: &str,
    text: &str,
) -> Result<String, String> {
    let resp = client
        .get(format!(
            "{}?client=gtx&sl={}&tl={}&dt=t&q={}",
            url,
            urlencoding::encode(source_lang),
            urlencoding::encode(target_lang),
            urlencoding::encode(text)
        ))
        .send()
        .map_err(|e| format!("Google 翻译请求失败: {e}"))?;
    let status = resp.status();
    let body = resp.text().unwrap_or_default();
    if !status.is_success() {
        return Err(format!(
            "Google 翻译 HTTP {}: {}",
            status.as_u16(),
            body.chars().take(300).collect::<String>()
        ));
    }
    parse_google_body(&body)
}

fn parse_google_body(body: &str) -> Result<String, String> {
    let v: Value =
        serde_json::from_str(body).map_err(|e| format!("Google 翻译响应非 JSON: {e}"))?;
    let Some(items) = v.get(0).and_then(|x| x.as_array()) else {
        return Err("Google 翻译响应格式异常".into());
    };
    let mut out = String::new();
    for item in items {
        if let Some(seg) = item.get(0).and_then(|x| x.as_str()) {
            out.push_str(seg);
        }
    }
    if out.trim().is_empty() {
        return Err("Google 翻译响应为空".into());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::parse_google_body;

    #[test]
    fn parse_google_nested_array() {
        let text = parse_google_body(r#"[[["你好","hello",null,null,10]],null,"en"]"#).unwrap();
        assert_eq!(text, "你好");
    }
}
