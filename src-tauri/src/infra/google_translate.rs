use std::error::Error as StdError;
use std::time::Duration;

use reqwest::blocking::Client;
use serde_json::Value;

use super::srt::SubCue;
use super::system_proxy;

const DEFAULT_GOOGLE_WEB_URL: &str = "https://translate.googleapis.com/translate_a/single";
pub(crate) const GOOGLE_WEB_USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";
const GOOGLE_TRANSLATE_MAX_RETRIES: u32 = 3;
const GOOGLE_TRANSLATE_BACKOFF_BASE_MS: u64 = 500;

pub struct GoogleWebTranslateJob<'a> {
    pub provider_url: &'a str,
    pub use_proxy: bool,
    pub min_interval_ms: u64,
    pub source_lang: &'a str,
    pub target_lang: &'a str,
}

pub fn build_google_client(use_proxy: bool, timeout_sec: u64) -> Result<Client, String> {
    let builder = Client::builder()
        .use_native_tls()
        .http1_only()
        .timeout(Duration::from_secs(timeout_sec.max(1)))
        .pool_max_idle_per_host(0)
        .user_agent(GOOGLE_WEB_USER_AGENT);
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
    mut on_item_done: impl FnMut(usize, &str),
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
            if sleep_with_abort(
                Duration::from_millis(job.min_interval_ms),
                &mut pause_requested,
            ) {
                return Err("__pause__".into());
            }
        }

        let mut last_err: Option<String> = None;
        let mut translated: Option<String> = None;
        for attempt in 0..=GOOGLE_TRANSLATE_MAX_RETRIES {
            if attempt > 0 {
                let wait = GOOGLE_TRANSLATE_BACKOFF_BASE_MS
                    .saturating_mul(1u64 << (attempt - 1).min(5));
                log::warn!(
                    target: "subforge_google",
                    "google translate retry cue#{} attempt={} wait_ms={} last_err={}",
                    cue.index,
                    attempt,
                    wait,
                    last_err.as_deref().unwrap_or("")
                );
                if sleep_with_abort(Duration::from_millis(wait), &mut pause_requested) {
                    return Err("__pause__".into());
                }
            }
            match translate_one(client, &url, job.use_proxy, &sl, &tl, &cue.text) {
                Ok(t) => {
                    translated = Some(t);
                    break;
                }
                Err(GoogleSendError::Retriable(msg)) => last_err = Some(msg),
                Err(GoogleSendError::Fatal(msg)) => return Err(msg),
            }
        }
        let translated = translated.ok_or_else(|| {
            last_err
                .map(|m| format!("Google 翻译重试 {} 次仍失败：{}", GOOGLE_TRANSLATE_MAX_RETRIES, m))
                .unwrap_or_else(|| "Google 翻译失败".into())
        })?;
        on_item_done(idx, &translated);
        out.push(translated);
        on_progress(idx + 1, cues.len());
    }
    Ok(out)
}

fn sleep_with_abort(total: Duration, pause_requested: &mut impl FnMut() -> bool) -> bool {
    let step = Duration::from_millis(100);
    let mut remaining = total;
    while remaining > Duration::ZERO {
        if pause_requested() {
            return true;
        }
        let chunk = remaining.min(step);
        std::thread::sleep(chunk);
        remaining = remaining.saturating_sub(chunk);
    }
    pause_requested()
}

enum GoogleSendError {
    Retriable(String),
    Fatal(String),
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
) -> Result<String, GoogleSendError> {
    let resp = client
        .get(format!(
            "{}?client=gtx&sl={}&tl={}&dt=t&q={}",
            url,
            urlencoding::encode(source_lang),
            urlencoding::encode(target_lang),
            urlencoding::encode(text)
        ))
        .send()
        .map_err(|e| GoogleSendError::Retriable(format_google_request_error(&e, url, use_proxy)))?;
    let status = resp.status();
    let body = resp
        .text()
        .map_err(|e| GoogleSendError::Retriable(format!("Google 翻译读取响应失败: {}", format_error_chain(&e))))?;
    if !status.is_success() {
        let msg = format!(
            "Google 翻译 HTTP {}: {}",
            status.as_u16(),
            body.chars().take(300).collect::<String>()
        );
        if status.as_u16() == 429 || status.is_server_error() {
            return Err(GoogleSendError::Retriable(msg));
        }
        return Err(GoogleSendError::Fatal(msg));
    }
    parse_google_body(&body).map_err(GoogleSendError::Fatal)
}

pub(crate) fn format_error_chain<E: StdError + ?Sized>(err: &E) -> String {
    let mut parts: Vec<String> = vec![err.to_string()];
    let mut src: Option<&dyn StdError> = err.source();
    while let Some(s) = src {
        let msg = s.to_string();
        if parts.last().map(|prev| prev != &msg).unwrap_or(true) {
            parts.push(msg);
        }
        src = s.source();
    }
    parts.join(" | 原因: ")
}

pub(crate) fn format_google_request_error(
    err: &reqwest::Error,
    url: &str,
    use_proxy: bool,
) -> String {
    let detail = format_error_chain(err);
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
