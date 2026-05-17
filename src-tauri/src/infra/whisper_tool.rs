use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;

use serde_json::Value;
use which::which;

use super::process::apply_windows_no_window;
use super::whisper_runtime::managed_whisper_dir;

#[derive(Debug, Clone)]
pub struct WhisperVadOptions<'a> {
    pub model_path: &'a Path,
    pub threshold: f32,
    pub min_speech_ms: u32,
    pub min_silence_ms: u32,
    pub max_segment_ms: u32,
}

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

pub type ProgressCallback = Arc<Mutex<dyn FnMut(u8) + Send>>;

/// 包装一个 FnMut 闭包成可在多线程间共享的 ProgressCallback。
pub fn boxed_progress<F>(f: F) -> ProgressCallback
where
    F: FnMut(u8) + Send + 'static,
{
    Arc::new(Mutex::new(f))
}

/// whisper.cpp: `-of` 为不含扩展名的前缀；在工作目录下生成 `.srt` / `.json`。
/// 第一次运行（SRT）映射进度到 0-50，第二次（JSON）映射到 50-100。
pub fn run_whisper_srt_json(
    cli: &Path,
    model: &Path,
    wav: &Path,
    lang: &str,
    threads: u32,
    force_cpu: bool,
    out_prefix: &Path,
    vad: Option<&WhisperVadOptions<'_>>,
    initial_prompt: Option<&str>,
    progress: Option<ProgressCallback>,
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
        if t.is_empty() {
            "auto"
        } else {
            t
        }
    };

    let mut cmd = Command::new(cli);
    cmd.current_dir(work_dir);
    cmd.arg("-m").arg(model);
    cmd.arg("-f").arg(wav);
    cmd.arg("-l").arg(lang_arg);
    cmd.arg("-t").arg(threads.to_string());
    if force_cpu {
        cmd.arg("-ngl").arg("0");
    }
    if let Some(vad) = vad {
        cmd.arg("--vad");
        cmd.arg("-vm").arg(vad.model_path);
        cmd.arg("-vt").arg(format!("{:.2}", vad.threshold));
        cmd.arg("-vspd").arg(vad.min_speech_ms.to_string());
        cmd.arg("-vsd").arg(vad.min_silence_ms.to_string());
        cmd.arg("-vmsd")
            .arg(format!("{:.3}", vad.max_segment_ms as f64 / 1000.0));
    }
    if progress.is_some() {
        cmd.arg("--print-progress");
    }
    if let Some(prompt) = initial_prompt {
        let trimmed = prompt.trim();
        if !trimmed.is_empty() {
            cmd.arg("--prompt").arg(trimmed);
        }
    }
    cmd.arg("-osrt");
    cmd.arg("-of").arg(&prefix_name);

    run_whisper_command(cmd, progress.clone(), 0, 50)?;

    let mut json_cmd = Command::new(cli);
    json_cmd.current_dir(work_dir);
    json_cmd.arg("-m").arg(model);
    json_cmd.arg("-f").arg(wav);
    json_cmd.arg("-l").arg(lang_arg);
    json_cmd.arg("-t").arg(threads.to_string());
    if force_cpu {
        json_cmd.arg("-ngl").arg("0");
    }
    if let Some(vad) = vad {
        json_cmd.arg("--vad");
        json_cmd.arg("-vm").arg(vad.model_path);
        json_cmd.arg("-vt").arg(format!("{:.2}", vad.threshold));
        json_cmd.arg("-vspd").arg(vad.min_speech_ms.to_string());
        json_cmd.arg("-vsd").arg(vad.min_silence_ms.to_string());
        json_cmd
            .arg("-vmsd")
            .arg(format!("{:.3}", vad.max_segment_ms as f64 / 1000.0));
    }
    if progress.is_some() {
        json_cmd.arg("--print-progress");
    }
    if let Some(prompt) = initial_prompt {
        let trimmed = prompt.trim();
        if !trimmed.is_empty() {
            json_cmd.arg("--prompt").arg(trimmed);
        }
    }
    json_cmd.arg("-ml").arg("1");
    json_cmd.arg("-sow");
    json_cmd.arg("-ojf");
    json_cmd.arg("-of").arg(&prefix_name);

    run_whisper_command(json_cmd, progress, 50, 50)?;
    Ok(())
}

/// 在 stderr 行中解析 whisper.cpp 的进度输出：
/// `whisper_print_progress_callback: progress = 50%`
fn parse_progress_line(line: &str) -> Option<u8> {
    let idx = line.find("progress")?;
    let after = &line[idx + "progress".len()..];
    let eq = after.find('=')?;
    let tail = after[eq + 1..].trim_start();
    let mut digits = String::new();
    for c in tail.chars() {
        if c.is_ascii_digit() {
            digits.push(c);
        } else {
            break;
        }
    }
    if digits.is_empty() {
        return None;
    }
    digits.parse::<u32>().ok().map(|n| n.min(100) as u8)
}

fn run_whisper_command(
    mut cmd: Command,
    progress: Option<ProgressCallback>,
    base_pct: u8,
    span_pct: u8,
) -> Result<(), String> {
    apply_windows_no_window(&mut cmd);

    if progress.is_none() {
        // 无回调：保持原一次性输出捕获行为
        let out = cmd
            .output()
            .map_err(|e| format!("启动 Whisper 失败: {e}"))?;
        if out.status.success() {
            return Ok(());
        }
        let mut msg = String::from_utf8_lossy(&out.stderr).to_string();
        if msg.trim().is_empty() {
            msg = String::from_utf8_lossy(&out.stdout).to_string();
        }
        let msg: String = msg.chars().take(4000).collect();
        return Err(format!("Whisper 识别失败: {msg}"));
    }

    let progress = progress.unwrap();
    let mut child = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("启动 Whisper 失败: {e}"))?;

    let stderr = child.stderr.take();
    let stdout = child.stdout.take();

    let progress_cb = progress.clone();
    let err_thread = stderr.map(|stderr| {
        thread::spawn(move || {
            let reader = BufReader::new(stderr);
            let mut last_emit: i16 = -1;
            let mut tail = String::new();
            for line in reader.lines().map_while(Result::ok) {
                if let Some(pct) = parse_progress_line(&line) {
                    if (pct as i16) > last_emit {
                        last_emit = pct as i16;
                        let mapped = base_pct.saturating_add(
                            ((pct as u32 * span_pct as u32) / 100u32) as u8,
                        );
                        if let Ok(mut cb) = progress_cb.lock() {
                            cb(mapped.min(base_pct.saturating_add(span_pct)));
                        }
                    }
                }
                tail.push_str(&line);
                tail.push('\n');
                if tail.len() > 32 * 1024 {
                    let cut = tail.len() - 16 * 1024;
                    tail = tail.split_off(cut);
                }
            }
            tail
        })
    });

    let out_thread = stdout.map(|mut stdout| {
        thread::spawn(move || {
            let mut buf = String::new();
            let _ = stdout.read_to_string(&mut buf);
            buf
        })
    });

    let status = child
        .wait()
        .map_err(|e| format!("等待 Whisper 退出失败: {e}"))?;

    let stderr_text = err_thread
        .and_then(|h| h.join().ok())
        .unwrap_or_default();
    let stdout_text = out_thread
        .and_then(|h| h.join().ok())
        .unwrap_or_default();

    if status.success() {
        if let Ok(mut cb) = progress.lock() {
            cb(base_pct.saturating_add(span_pct).min(100));
        }
        return Ok(());
    }

    let mut msg = stderr_text;
    if msg.trim().is_empty() {
        msg = stdout_text;
    }
    let msg: String = msg.chars().take(4000).collect();
    Err(format!("Whisper 识别失败: {msg}"))
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

#[cfg(test)]
mod tests {
    use super::parse_progress_line;

    #[test]
    fn parse_progress_basic() {
        assert_eq!(
            parse_progress_line("whisper_print_progress_callback: progress = 50%"),
            Some(50)
        );
    }

    #[test]
    fn parse_progress_with_spaces() {
        assert_eq!(
            parse_progress_line("foo bar: progress =   7%"),
            Some(7)
        );
    }

    #[test]
    fn parse_progress_ignores_non_progress_lines() {
        assert_eq!(parse_progress_line("whisper: loading model"), None);
    }

    #[test]
    fn parse_progress_clamps_to_100() {
        assert_eq!(parse_progress_line("progress = 123%"), Some(100));
    }
}
