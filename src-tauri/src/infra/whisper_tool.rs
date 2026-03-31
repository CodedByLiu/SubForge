use std::path::{Path, PathBuf};

use serde_json::Value;
use which::which;

use super::whisper_runtime::managed_whisper_dir;

pub fn resolve_whisper_cli(app_dir: &Path, preferred: &str) -> Result<PathBuf, String> {
    let p = preferred.trim();
    if !p.is_empty() {
        let pb = PathBuf::from(p);
        if pb.is_file() {
            return Ok(pb);
        }
        return Err(format!("Whisper 可执行文件不存在: {}", pb.display()));
    }

    for pb in bundled_whisper_candidates(app_dir) {
        if pb.is_file() {
            return Ok(pb);
        }
    }

    for name in path_search_names() {
        if let Ok(pb) = which(name) {
            if is_valid_whisper_candidate(&pb) {
                return Ok(pb);
            }
        }
    }

    Err("未找到 whisper.cpp 可执行文件（请在配置中填写 whisper-cli / main 的路径）".into())
}

fn bundled_whisper_candidates(app_dir: &Path) -> Vec<PathBuf> {
    let dir = managed_whisper_dir(app_dir);
    if cfg!(target_os = "windows") {
        vec![dir.join("whisper-cli.exe"), dir.join("main.exe")]
    } else {
        vec![dir.join("whisper-cli"), dir.join("main")]
    }
}

fn path_search_names() -> &'static [&'static str] {
    if cfg!(target_os = "windows") {
        &["whisper-cli.exe", "main.exe", "whisper.exe"]
    } else {
        &["whisper-cli", "main", "whisper"]
    }
}

fn is_valid_whisper_candidate(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();

    if cfg!(target_os = "windows") {
        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .map(|s| s.to_ascii_lowercase())
            .unwrap_or_default();
        ext == "exe" && matches!(stem.as_str(), "whisper-cli" | "main" | "whisper")
    } else {
        matches!(stem.as_str(), "whisper-cli" | "main" | "whisper")
    }
}

/// whisper.cpp: `-of` 为不含扩展名的前缀；在工作目录下生成 `.srt` / `.json`
pub fn run_whisper_srt_json(
    cli: &Path,
    model: &Path,
    wav: &Path,
    lang: &str,
    threads: u32,
    force_cpu: bool,
    out_prefix: &Path,
) -> Result<(), String> {
    let work_dir = out_prefix
        .parent()
        .ok_or_else(|| "临时输出目录无效".to_string())?;
    let prefix_name = out_prefix
        .file_name()
        .ok_or_else(|| "临时输出前缀无效".to_string())?
        .to_string_lossy()
        .to_string();

    std::fs::create_dir_all(work_dir).map_err(|e| format!("创建目录失败: {e}"))?;

    let lang_arg = {
        let t = lang.trim();
        if t.is_empty() { "auto" } else { t }
    };

    let mut cmd = std::process::Command::new(cli);
    cmd.current_dir(work_dir);
    cmd.arg("-m").arg(model);
    cmd.arg("-f").arg(wav);
    cmd.arg("-l").arg(lang_arg);
    cmd.arg("-t").arg(threads.to_string());
    if force_cpu {
        cmd.arg("-ngl").arg("0");
    }
    cmd.arg("-oj");
    cmd.arg("-osrt");
    cmd.arg("-of").arg(&prefix_name);

    let out = cmd
        .output()
        .map_err(|e| format!("启动 Whisper 失败: {e}"))?;
    if !out.status.success() {
        let mut msg = String::from_utf8_lossy(&out.stderr).to_string();
        if msg.trim().is_empty() {
            msg = String::from_utf8_lossy(&out.stdout).to_string();
        }
        let msg: String = msg.chars().take(4000).collect();
        return Err(format!("Whisper 识别失败: {msg}"));
    }
    Ok(())
}

pub fn read_language_from_whisper_json(json_path: &Path, fallback: &str) -> String {
    let Ok(raw) = std::fs::read_to_string(json_path) else {
        return normalize_lang_token(fallback);
    };
    let Ok(v) = serde_json::from_str::<Value>(&raw) else {
        return normalize_lang_token(fallback);
    };
    v.get("language")
        .or_else(|| v.get("result").and_then(|r| r.get("language")))
        .and_then(|x| x.as_str())
        .map(normalize_lang_token)
        .unwrap_or_else(|| normalize_lang_token(fallback))
}

fn normalize_lang_token(s: &str) -> String {
    let t = s.trim();
    if t.is_empty() || t.eq_ignore_ascii_case("auto") {
        "und".to_string()
    } else {
        t.chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
            .take(16)
            .collect::<String>()
            .to_ascii_lowercase()
    }
}

pub fn expected_whisper_sidecar_paths(prefix: &Path) -> (PathBuf, PathBuf) {
    let srt = prefix.with_extension("srt");
    let json = prefix.with_extension("json");
    (srt, json)
}
