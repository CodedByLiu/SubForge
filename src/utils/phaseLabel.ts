const PHASE_MAP: Record<string, string> = {
  extract_audio: "抽取音频",
  vad_detect: "检测语音",
  transcribe: "识别字幕",
  segment_subtitles: "分句优化",
  translate_google: "翻译中（Google）",
  translate_llm: "翻译中（LLM）",
  export_srt: "导出字幕",
  retry: "重试中",
};

export function phaseLabel(raw: string | undefined | null): string {
  if (!raw) return "";
  if (PHASE_MAP[raw]) return PHASE_MAP[raw];
  if (raw.startsWith("retry_")) return "重试中";
  return raw;
}
