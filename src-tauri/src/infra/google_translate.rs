use std::time::Duration;

use reqwest::blocking::Client;
use serde_json::Value;

use super::srt::SubCue;
use super::system_proxy;

const DEFAULT_GOOGLE_WEB_URL: &str = "https://translate.googleapis.com/translate_a/single";

pub struct GoogleWebTranslateJob<'a> {
    pub provider_url: &'a str,
    pub use_proxy: bool,
    pub min_interval_ms: u64,
    pub source_lang: &'a str,
    pub target_lang: &'a str,
}

pub fn build_google_client(use_proxy: bool, timeout_sec: u64) -> Result<Client, String> {
    let builder = Client::builder().timeout(Duration::from_secs(timeout_sec.max(1)));
    let (builder, proxy_display) = system_proxy::apply_to_blocking_builder(builder, use_proxy)?;
    if let Some(proxy) = proxy_display {
        log::info!(target: "subforge_google", "google_web_proxy={proxy}");
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
        let translated = translate_one(client, &url, job.use_proxy, &sl, &tl, &cue.text)?;
        out.push(translated);
        on_progress(idx + 1, cues.len());
    }
    Ok(out)
}

pub(crate) fn effective_google_url(provider_url: &str) -> String {
    let trimmed = provider_url.trim();
    if trimmed.is_empty() {
        DEFAULT_GOOGLE_WEB_URL.to_string()
    } else {
        trimmed.to_string()
    }
}

pub(crate) fn normalize_lang(lang: &str) -> String {
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
    use_proxy: bool,
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
        .map_err(|e| format_google_request_error(&e, url, use_proxy))?;
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

pub(crate) fn format_google_request_error(
    err: &reqwest::Error,
    url: &str,
    use_proxy: bool,
) -> String {
    let detail = err.to_string();
    if err.is_timeout() {
        if use_proxy {
            return format!(
                "Google 翻译请求超时：已开启代理，但访问 {} 超时。请检查代理服务或当前网络。原始错误：{}",
                url, detail
            );
        }
        return format!(
            "Google 翻译请求超时：当前未开启代理，访问 {} 超时。若当前网络访问 Google 需要代理，请在设置中开启“使用代理”。原始错误：{}",
            url, detail
        );
    }

    if err.is_connect() || detail.to_ascii_lowercase().contains("dns") {
        if use_proxy {
            return format!(
                "Google 翻译网络不可达：已开启代理，但仍无法连接 {}。请检查代理配置、代理软件或中转服务是否可用。原始错误：{}",
                url, detail
            );
        }
        return format!(
            "Google 翻译网络不可达：当前未开启代理，无法直连 {}。如果当前网络访问 Google 需要代理，请在设置中开启“使用代理”。原始错误：{}",
            url, detail
        );
    }

    format!("Google 翻译请求失败：{}。原始错误：{}", url, detail)
}

pub(crate) fn parse_google_body(body: &str) -> Result<String, String> {
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
