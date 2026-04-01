//! 极简 SRT 解析与写出（与 Whisper 导出格式兼容）

#[derive(Debug, Clone)]
pub struct SubCue {
    pub index: u32,
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
}

fn parse_ts(part: &str) -> Option<i64> {
    let p = part.trim();
    let (hms, ms_part) = if let Some((a, b)) = p.split_once(',') {
        (a, b)
    } else if let Some((a, b)) = p.split_once('.') {
        (a, b)
    } else {
        return None;
    };
    let mut it = hms.split(':');
    let h: i64 = it.next()?.parse().ok()?;
    let m: i64 = it.next()?.parse().ok()?;
    let s: i64 = it.next()?.parse().ok()?;
    let ms: i64 = ms_part.trim().parse().ok()?;
    Some(((h * 60 + m) * 60 + s) * 1000 + ms)
}

fn parse_time_line(line: &str) -> Option<(i64, i64)> {
    let (a, b) = line.split_once("-->")?;
    let start = parse_ts(a)?;
    let end = parse_ts(b.split_whitespace().next()?)?;
    Some((start, end))
}

fn format_ts(ms: i64) -> String {
    let h = ms / 3_600_000;
    let m = (ms % 3_600_000) / 60_000;
    let s = (ms % 60_000) / 1000;
    let milli = ms % 1000;
    format!("{h:02}:{m:02}:{s:02},{milli:03}")
}

pub fn parse_srt(raw: &str) -> Result<Vec<SubCue>, String> {
    let normalized = raw.replace('\r', "");
    let mut out = Vec::new();
    for block in normalized.split("\n\n") {
        let lines: Vec<&str> = block
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect();
        if lines.len() < 2 {
            continue;
        }
        let idx: u32 = lines[0]
            .parse()
            .map_err(|_| format!("无效序号行: {}", lines[0]))?;
        let (start_ms, end_ms) =
            parse_time_line(lines[1]).ok_or_else(|| format!("无效时间轴: {}", lines[1]))?;
        let text = lines[2..].join("\n");
        out.push(SubCue {
            index: idx,
            start_ms,
            end_ms,
            text,
        });
    }
    if out.is_empty() {
        return Err("未解析到任何字幕块".into());
    }
    Ok(out)
}

pub fn format_srt(cues: &[SubCue]) -> String {
    let mut s = String::new();
    for c in cues {
        s.push_str(&format!("{}\n", c.index));
        s.push_str(&format!(
            "{} --> {}\n",
            format_ts(c.start_ms),
            format_ts(c.end_ms)
        ));
        s.push_str(&c.text);
        s.push_str("\n\n");
    }
    s
}

/// 双语：上行译文、下行原文；时间轴与 `sources` 一致
pub fn build_bilingual_cues(
    sources: &[SubCue],
    translated_lines: &[String],
) -> Result<Vec<SubCue>, String> {
    if sources.len() != translated_lines.len() {
        return Err(format!(
            "双语行数不一致: {} / {}",
            sources.len(),
            translated_lines.len()
        ));
    }
    Ok(sources
        .iter()
        .zip(translated_lines.iter())
        .map(|(c, t)| SubCue {
            index: c.index,
            start_ms: c.start_ms,
            end_ms: c.end_ms,
            text: format!("{}\n{}", t.trim(), c.text.trim()),
        })
        .collect())
}
