use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// §7.1：源视频路径的稳定短哈希（8 位十六进制）
pub fn stable_path_hash(path_str: &str) -> String {
    let h = Sha256::digest(path_str.as_bytes());
    format!("{:02x}{:02x}{:02x}{:02x}", h[0], h[1], h[2], h[3])
}

fn sanitize_lang_code(raw: &str) -> String {
    let t = raw.trim();
    if t.is_empty() || t.eq_ignore_ascii_case("auto") {
        return "und".to_string();
    }
    t.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .take(16)
        .collect::<String>()
        .to_ascii_lowercase()
}

/// 原文字幕单文件：`{stem}_{lang}.srt`；统一输出目录下 `{stem}_{hash8}_{lang}.srt`
pub fn resolve_original_srt_path(
    video_path: &Path,
    video_path_key: &str,
    output_dir_mode: &str,
    custom_output_dir: &str,
    lang_code: &str,
) -> Result<PathBuf, String> {
    let stem = video_path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "subtitle".into());
    let lang = sanitize_lang_code(lang_code);

    let (dir, use_hash) = if output_dir_mode == "custom" {
        let d = custom_output_dir.trim();
        if d.is_empty() {
            return Err("自定义输出目录为空".into());
        }
        (PathBuf::from(d), true)
    } else {
        let parent = video_path
            .parent()
            .map(|p| p.to_path_buf())
            .ok_or_else(|| "无法解析视频所在目录".to_string())?;
        (parent, false)
    };

    let file_name = if use_hash {
        let h = stable_path_hash(video_path_key);
        format!("{stem}_{h}_{lang}.srt")
    } else {
        format!("{stem}_{lang}.srt")
    };

    Ok(dir.join(file_name))
}

/// 单文件双语：`{stem}.srt`；统一输出目录：`{stem}_{hash8}.srt`（§7.5）
pub fn resolve_bilingual_srt_path(
    video_path: &Path,
    video_path_key: &str,
    output_dir_mode: &str,
    custom_output_dir: &str,
) -> Result<PathBuf, String> {
    let stem = video_path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "subtitle".into());

    let (dir, use_hash) = if output_dir_mode == "custom" {
        let d = custom_output_dir.trim();
        if d.is_empty() {
            return Err("自定义输出目录为空".into());
        }
        (PathBuf::from(d), true)
    } else {
        let parent = video_path
            .parent()
            .map(|p| p.to_path_buf())
            .ok_or_else(|| "无法解析视频所在目录".to_string())?;
        (parent, false)
    };

    let file_name = if use_hash {
        let h = stable_path_hash(video_path_key);
        format!("{stem}_{h}.srt")
    } else {
        format!("{stem}.srt")
    };

    Ok(dir.join(file_name))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{
        resolve_bilingual_srt_path, resolve_original_srt_path, stable_path_hash,
    };

    #[test]
    fn stable_hash_is_deterministic() {
        assert_eq!(
            stable_path_hash("C:/videos/demo.mp4"),
            stable_path_hash("C:/videos/demo.mp4")
        );
    }

    #[test]
    fn original_path_uses_video_dir_without_hash() {
        let video = Path::new("C:/videos/demo.mp4");
        let output =
            resolve_original_srt_path(video, "C:/videos/demo.mp4", "video_dir", "", "en")
                .unwrap();
        assert_eq!(output, Path::new("C:/videos/demo_en.srt"));
    }

    #[test]
    fn original_path_uses_hash_in_custom_dir() {
        let video = Path::new("C:/videos/demo.mp4");
        let output = resolve_original_srt_path(
            video,
            "C:/videos/demo.mp4",
            "custom",
            "D:/subs",
            "zh",
        )
        .unwrap();
        let file_name = output.file_name().unwrap().to_string_lossy();
        assert!(output.starts_with("D:/subs"));
        assert!(file_name.starts_with("demo_"));
        assert!(file_name.ends_with("_zh.srt"));
    }

    #[test]
    fn bilingual_path_uses_plain_name_in_video_dir() {
        let video = Path::new("C:/videos/demo.mp4");
        let output =
            resolve_bilingual_srt_path(video, "C:/videos/demo.mp4", "video_dir", "").unwrap();
        assert_eq!(output, Path::new("C:/videos/demo.srt"));
    }
}
