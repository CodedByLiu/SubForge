//! OpenAI 兼容 Chat Completions 基址

pub fn chat_completions_url(base: &str) -> String {
    let t = base.trim_end_matches('/');
    if t.ends_with("/chat/completions") {
        t.to_string()
    } else if t.ends_with("/v1") {
        format!("{t}/chat/completions")
    } else {
        format!("{t}/v1/chat/completions")
    }
}

pub fn truncate_detail(s: &str, max: usize) -> String {
    let t = s.trim();
    if t.len() > max {
        format!("{}…", &t[..max])
    } else {
        t.to_string()
    }
}
