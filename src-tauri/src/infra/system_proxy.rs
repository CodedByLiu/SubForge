#[derive(Debug, Clone)]
pub struct ProxyResolution {
    pub url: String,
    pub display: String,
}

pub fn apply_to_blocking_builder(
    mut builder: reqwest::blocking::ClientBuilder,
    use_proxy: bool,
) -> Result<(reqwest::blocking::ClientBuilder, Option<String>), String> {
    if !use_proxy {
        builder = builder.no_proxy();
        return Ok((builder, None));
    }

    let proxy = resolve_system_proxy()?.ok_or_else(|| {
        "已勾选“使用代理”，但未检测到系统代理。请先在 Windows 代理设置中启用代理，或取消勾选。"
            .to_string()
    })?;
    let reqwest_proxy = reqwest::Proxy::all(&proxy.url)
        .map_err(|e| format!("系统代理配置无效: {} ({e})", proxy.display))?;
    builder = builder.proxy(reqwest_proxy);
    Ok((builder, Some(proxy.display)))
}

pub fn apply_to_async_builder(
    mut builder: reqwest::ClientBuilder,
    use_proxy: bool,
) -> Result<(reqwest::ClientBuilder, Option<String>), String> {
    if !use_proxy {
        builder = builder.no_proxy();
        return Ok((builder, None));
    }

    let proxy = resolve_system_proxy()?.ok_or_else(|| {
        "已勾选“使用代理”，但未检测到系统代理。请先在 Windows 代理设置中启用代理，或取消勾选。"
            .to_string()
    })?;
    let reqwest_proxy = reqwest::Proxy::all(&proxy.url)
        .map_err(|e| format!("系统代理配置无效: {} ({e})", proxy.display))?;
    builder = builder.proxy(reqwest_proxy);
    Ok((builder, Some(proxy.display)))
}

#[cfg(target_os = "windows")]
fn resolve_system_proxy() -> Result<Option<ProxyResolution>, String> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let internet = hkcu
        .open_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings")
        .map_err(|e| format!("读取 Windows 代理设置失败: {e}"))?;

    let enabled = internet.get_value::<u32, _>("ProxyEnable").unwrap_or(0);
    if enabled == 0 {
        return Ok(None);
    }

    let server = internet
        .get_value::<String, _>("ProxyServer")
        .unwrap_or_default();
    let server = server.trim();
    if server.is_empty() {
        return Ok(None);
    }

    parse_windows_proxy_server(server)
}

#[cfg(not(target_os = "windows"))]
fn resolve_system_proxy() -> Result<Option<ProxyResolution>, String> {
    for key in ["HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"] {
        if let Ok(value) = std::env::var(key) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return Ok(Some(ProxyResolution {
                    url: trimmed.to_string(),
                    display: format!("环境变量 {key}={trimmed}"),
                }));
            }
        }
    }
    Ok(None)
}

#[cfg(target_os = "windows")]
fn parse_windows_proxy_server(raw: &str) -> Result<Option<ProxyResolution>, String> {
    let mut default_endpoint: Option<&str> = None;
    let mut endpoints = std::collections::HashMap::<String, &str>::new();

    for part in raw.split(';') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some((scheme, value)) = part.split_once('=') {
            endpoints.insert(scheme.trim().to_ascii_lowercase(), value.trim());
        } else {
            default_endpoint = Some(part);
        }
    }

    let selected = endpoints
        .get("https")
        .map(|v| ("https", *v))
        .or_else(|| endpoints.get("http").map(|v| ("http", *v)))
        .or_else(|| endpoints.get("socks").map(|v| ("socks", *v)))
        .or_else(|| endpoints.get("socks5").map(|v| ("socks5", *v)))
        .or_else(|| default_endpoint.map(|v| ("http", v)));

    let Some((kind, value)) = selected else {
        return Ok(None);
    };

    let url = normalize_proxy_url(kind, value)?;
    Ok(Some(ProxyResolution {
        url,
        display: format!("Windows 系统代理 {value}"),
    }))
}

#[cfg(target_os = "windows")]
fn normalize_proxy_url(kind: &str, value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("系统代理地址为空".into());
    }
    if value.contains("://") {
        return Ok(value.to_string());
    }

    let prefix = match kind {
        "socks" | "socks5" => "socks5://",
        _ => "http://",
    };
    Ok(format!("{prefix}{value}"))
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::parse_windows_proxy_server;

    #[test]
    fn parse_simple_windows_proxy() {
        let parsed = parse_windows_proxy_server("127.0.0.1:7890")
            .unwrap()
            .expect("proxy should exist");
        assert_eq!(parsed.url, "http://127.0.0.1:7890");
    }

    #[test]
    fn parse_https_specific_proxy() {
        let parsed = parse_windows_proxy_server("http=127.0.0.1:8080;https=127.0.0.1:7890")
            .unwrap()
            .expect("proxy should exist");
        assert_eq!(parsed.url, "http://127.0.0.1:7890");
    }

    #[test]
    fn parse_socks_proxy() {
        let parsed = parse_windows_proxy_server("socks=127.0.0.1:1080")
            .unwrap()
            .expect("proxy should exist");
        assert_eq!(parsed.url, "socks5://127.0.0.1:1080");
    }
}
