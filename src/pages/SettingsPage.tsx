import { listen } from "@tauri-apps/api/event";
import { confirm } from "@tauri-apps/plugin-dialog";
import { useCallback, useEffect, useRef, useState } from "react";
import { Link } from "react-router-dom";
import {
  deleteWhisperModel,
  downloadWhisperModel,
  getAppInfo,
  getConfig,
  getHardwareInfo,
  listWhisperModels,
  saveConfig,
  testLlmConnection,
  type SaveConfigPayload,
} from "@/services/ipc";
import { checkTranscribeDeps } from "@/services/tasksIpc";
import {
  defaultAppConfig,
  type AppConfig,
  type AppConfigView,
  type GlossaryEntry,
  type LlmTestResult,
} from "@/types/config";
import type {
  HardwareInfoDto,
  WhisperDownloadProgress,
  WhisperRuntimeProgress,
  WhisperModelsListDto,
} from "@/types/hardware";
import type { TranscribeDepsCheck } from "@/types/tasks";

function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / (1024 * 1024)).toFixed(1)} MB`;
}

function mergeLoaded(view: AppConfigView): AppConfig {
  const { api_key_configured: _a, ...rest } = view;
  return rest;
}

const FIXED_LANG_OPTIONS = ["auto", "zh", "en"] as const;

function normalizeFixedLang(
  value: string,
  fallback: (typeof FIXED_LANG_OPTIONS)[number],
): (typeof FIXED_LANG_OPTIONS)[number] {
  const normalized = value.trim().toLowerCase();
  return (FIXED_LANG_OPTIONS as readonly string[]).includes(normalized)
    ? (normalized as (typeof FIXED_LANG_OPTIONS)[number])
    : fallback;
}

export function SettingsPage() {
  const [cfg, setCfg] = useState<AppConfig>(() => defaultAppConfig());
  const [apiKeyConfigured, setApiKeyConfigured] = useState(false);
  const [apiKeyInput, setApiKeyInput] = useState("");
  const [appDir, setAppDir] = useState("");
  const [version, setVersion] = useState("");
  const [loadErr, setLoadErr] = useState<string | null>(null);
  const [saveErr, setSaveErr] = useState<string | null>(null);
  const [saveOk, setSaveOk] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [testBusy, setTestBusy] = useState(false);
  const [testResult, setTestResult] = useState<LlmTestResult | null>(null);
  const [hw, setHw] = useState<HardwareInfoDto | null>(null);
  const [whisperList, setWhisperList] = useState<WhisperModelsListDto | null>(null);
  const [dlProgress, setDlProgress] = useState<WhisperDownloadProgress | null>(null);
  const [modelBusy, setModelBusy] = useState(false);
  const [modelErr, setModelErr] = useState<string | null>(null);
  const [depsBusy, setDepsBusy] = useState(false);
  const [depsErr, setDepsErr] = useState<string | null>(null);
  const [depsResult, setDepsResult] = useState<TranscribeDepsCheck | null>(null);
  const [runtimeProgress, setRuntimeProgress] =
    useState<WhisperRuntimeProgress | null>(null);
  const useGpuRef = useRef(cfg.whisper.use_gpu);
  useGpuRef.current = cfg.whisper.use_gpu;

  const refreshWhisperPanel = useCallback(async (useGpu: boolean) => {
    try {
      const [h, wlm] = await Promise.all([
        getHardwareInfo(useGpu),
        listWhisperModels(),
      ]);
      setHw(h);
      setWhisperList(wlm);
      setModelErr(null);
    } catch (e) {
      setModelErr(String(e));
    }
  }, []);

  const load = useCallback(async () => {
    setLoadErr(null);
    try {
      const [info, view] = await Promise.all([getAppInfo(), getConfig()]);
      setAppDir(info.app_dir);
      setVersion(info.version);
      const merged = mergeLoaded(view);
      setCfg(merged);
      setApiKeyConfigured(view.api_key_configured);
      setApiKeyInput("");
      setTestResult(null);
      await refreshWhisperPanel(merged.whisper.use_gpu);
    } catch (e) {
      setLoadErr(String(e));
    }
  }, [refreshWhisperPanel]);

  useEffect(() => {
    void load();
  }, [load]);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    void listen<WhisperDownloadProgress>("whisper-model-progress", (ev) => {
      setDlProgress(ev.payload);
      if (ev.payload.phase === "done") {
        void refreshWhisperPanel(useGpuRef.current);
      }
    }).then((fn) => {
      unlisten = fn;
    });
    return () => {
      unlisten?.();
    };
  }, [refreshWhisperPanel]);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    void listen<WhisperRuntimeProgress>("whisper-runtime-progress", (ev) => {
      setRuntimeProgress(ev.payload);
    }).then((fn) => {
      unlisten = fn;
    });
    return () => {
      unlisten?.();
    };
  }, []);

  const updateCfg = (patch: Partial<AppConfig>) => {
    setCfg((c) => ({ ...c, ...patch }));
  };

  const setTranslator = (patch: Partial<AppConfig["translator"]>) => {
    setCfg((c) => ({ ...c, translator: { ...c.translator, ...patch } }));
  };

  const setLlm = (patch: Partial<AppConfig["llm"]>) => {
    setCfg((c) => ({ ...c, llm: { ...c.llm, ...patch } }));
  };

  const setWhisper = (patch: Partial<AppConfig["whisper"]>) => {
    setCfg((c) => ({ ...c, whisper: { ...c.whisper, ...patch } }));
  };

  const setTranslate = (patch: Partial<AppConfig["translate"]>) => {
    setCfg((c) => ({ ...c, translate: { ...c.translate, ...patch } }));
  };

  const setSegmentation = (patch: Partial<AppConfig["segmentation"]>) => {
    setCfg((c) => ({ ...c, segmentation: { ...c.segmentation, ...patch } }));
  };

  const setSubtitle = (patch: Partial<AppConfig["subtitle"]>) => {
    setCfg((c) => ({ ...c, subtitle: { ...c.subtitle, ...patch } }));
  };

  const setRuntime = (patch: Partial<AppConfig["runtime"]>) => {
    setCfg((c) => ({ ...c, runtime: { ...c.runtime, ...patch } }));
  };

  const onSave = async (clearKey: boolean) => {
    setSaveErr(null);
    setSaveOk(null);
    setBusy(true);
    try {
      const payload: SaveConfigPayload = {
        ...cfg,
        clear_llm_api_key: clearKey,
      };
      if (!clearKey && apiKeyInput.trim()) {
        payload.llm_api_key = apiKeyInput.trim();
      }
      const view = await saveConfig(payload);
      const merged = mergeLoaded(view);
      setCfg(merged);
      setApiKeyConfigured(view.api_key_configured);
      setApiKeyInput("");
      await refreshWhisperPanel(merged.whisper.use_gpu);
      setSaveOk("已保存");
    } catch (e) {
      setSaveErr(String(e));
    } finally {
      setBusy(false);
    }
  };

  const onTest = async () => {
    setTestResult(null);
    setTestBusy(true);
    try {
      const r = await testLlmConnection({
        base_url: cfg.llm.base_url,
        model: cfg.llm.model,
        timeout_sec: cfg.llm.timeout_sec,
        api_key: apiKeyInput.trim() || null,
      });
      setTestResult(r);
    } catch (e) {
      setTestResult({
        ok: false,
        code: "invoke_error",
        message: String(e),
      });
    } finally {
      setTestBusy(false);
    }
  };

  const translateSectionVisible =
    cfg.translator.engine !== "none" && cfg.subtitle.mode !== "original_only";
  const recommendedWhisperModel = hw?.whisper_recommended_models?.[0] ?? null;

  const addGlossaryRow = () => {
    setTranslate({
      glossary: [...cfg.translate.glossary, { source: "", target: "", note: "" }],
    });
  };

  const patchGlossary = (i: number, row: Partial<GlossaryEntry>) => {
    const g = [...cfg.translate.glossary];
    g[i] = { ...g[i], ...row };
    setTranslate({ glossary: g });
  };

  const removeGlossary = (i: number) => {
    const g = cfg.translate.glossary.filter((_, j) => j !== i);
    setTranslate({ glossary: g });
  };

  return (
    <div className="page settings-layout">
      <div className="settings-header">
        <div>
          <h1 className="title">设置</h1>
          <p className="muted" style={{ margin: "0.25rem 0 0" }}>
            软件目录：{appDir || "…"} · v{version || "…"}
          </p>
        </div>
        <div className="row-actions">
          <Link className="link-btn" to="/">
            返回主界面
          </Link>
        </div>
      </div>

      {loadErr && (
        <div className="test-result err">加载配置失败：{loadErr}</div>
      )}
      {saveErr && <div className="test-result err">{saveErr}</div>}
      {saveOk && (
        <div className="test-result ok">
          {saveOk}
        </div>
      )}

      <section className="card">
        <h2>翻译引擎与 LLM</h2>
        <div className="field-grid two">
          <label className="field whisper-model-select">
            <span>翻译引擎</span>
            <select
              value={cfg.translator.engine}
              onChange={(e) => setTranslator({ engine: e.target.value })}
            >
              <option value="none">关闭（仅提取原字幕）</option>
              <option value="llm">LLM（OpenAI 兼容）</option>
              <option value="google_web">Google Web 免费模式（实验性）</option>
            </select>
          </label>
          <label className="field">
            <span>翻译源语言</span>
            <select
              value={normalizeFixedLang(cfg.translate.source_lang, "auto")}
              onChange={(e) => setTranslate({ source_lang: e.target.value })}
            >
              {FIXED_LANG_OPTIONS.map((lang) => (
                <option key={lang} value={lang}>
                  {lang}
                </option>
              ))}
            </select>
          </label>
          <label className="field">
            <span>翻译目标语言</span>
            <select
              value={normalizeFixedLang(cfg.translate.target_lang, "zh")}
              onChange={(e) => setTranslate({ target_lang: e.target.value })}
            >
              {FIXED_LANG_OPTIONS.map((lang) => (
                <option key={lang} value={lang}>
                  {lang}
                </option>
              ))}
            </select>
          </label>
          <label className="field">
            <span>超时（秒）</span>
            <input
              type="number"
              min={1}
              value={cfg.llm.timeout_sec}
              onChange={(e) =>
                setLlm({ timeout_sec: Number(e.target.value) || 1 })
              }
            />
          </label>
          <label className="field">
            <span>最大重试</span>
            <input
              type="number"
              min={0}
              max={20}
              value={cfg.llm.max_retries}
              onChange={(e) =>
                setLlm({ max_retries: Number(e.target.value) })
              }
            />
          </label>
          <label className="field">
            <span>翻译并发</span>
            <input
              type="number"
              min={1}
              max={64}
              value={cfg.llm.translate_concurrency}
              onChange={(e) =>
                setLlm({ translate_concurrency: Number(e.target.value) || 1 })
              }
            />
          </label>
        </div>
        {cfg.translator.engine === "llm" && (
          <div className="field-grid" style={{ marginTop: "0.75rem" }}>
            <label className="field">
              <span>Base URL</span>
              <input
                value={cfg.llm.base_url}
                onChange={(e) => setLlm({ base_url: e.target.value })}
                placeholder="https://api.openai.com/v1"
              />
            </label>
            <label className="field">
              <span>
                API Key
                {apiKeyConfigured ? "（已保存，留空则不修改）" : ""}
              </span>
              <input
                type="password"
                autoComplete="off"
                value={apiKeyInput}
                onChange={(e) => setApiKeyInput(e.target.value)}
                placeholder={apiKeyConfigured ? "••••••••" : "填写后保存"}
              />
            </label>
            <label className="field">
              <span>模型名称</span>
              <input
                value={cfg.llm.model}
                onChange={(e) => setLlm({ model: e.target.value })}
                placeholder="gpt-4o-mini"
              />
            </label>
            <div className="row-actions">
              <button type="button" disabled={testBusy} onClick={() => void onTest()}>
                {testBusy ? "测试中…" : "连接测试"}
              </button>
            </div>
            {testResult && (
              <div
                className={`test-result ${testResult.ok ? "ok" : "err"}`}
              >
                {testResult.message}
                {testResult.detail ? `\n${testResult.detail}` : ""}
              </div>
            )}
          </div>
        )}
        {cfg.translator.engine === "google_web" && (
          <div className="field-grid two" style={{ marginTop: "0.75rem" }}>
            <label className="field">
              <span>服务地址（可选）</span>
              <input
                value={cfg.translator.provider_url}
                onChange={(e) => setTranslator({ provider_url: e.target.value })}
              />
            </label>
            <label className="field" style={{ flexDirection: "row", alignItems: "center", gap: "0.5rem" }}>
              <input
                type="checkbox"
                checked={cfg.translator.use_proxy}
                onChange={(e) => setTranslator({ use_proxy: e.target.checked })}
              />
              <span>使用代理</span>
            </label>
            <label className="field">
              <span>最小请求间隔（毫秒）</span>
              <input
                type="number"
                min={0}
                value={cfg.translator.min_request_interval_ms}
                onChange={(e) =>
                  setTranslator({
                    min_request_interval_ms: Number(e.target.value) || 0,
                  })
                }
              />
            </label>
            <label className="field" style={{ flexDirection: "row", alignItems: "center", gap: "0.5rem" }}>
              <input
                type="checkbox"
                checked={cfg.translator.experimental_acknowledged}
                onChange={(e) =>
                  setTranslator({ experimental_acknowledged: e.target.checked })
                }
              />
              <span>已知悉实验性风险</span>
            </label>
            <p className="muted" style={{ gridColumn: "1 / -1", margin: 0 }}>
              实验性功能可能因接口变更、限流或区域限制不可用；首版以 LLM 为主路径。
            </p>
          </div>
        )}
      </section>

      <section className="card">
        <h2>Whisper 与模型下载</h2>
        {!hw && !loadErr ? (
          <p className="muted" style={{ margin: "0 0 0.75rem" }}>
            正在加载 CPU / 内存 / GPU 信息与模型列表…
          </p>
        ) : null}
        {hw && (
          <div
            style={{
              marginBottom: "0.85rem",
              padding: "0.65rem 0.75rem",
              borderRadius: 8,
              border: "1px solid var(--border)",
              background: "var(--input-bg)",
              fontSize: "0.85rem",
            }}
          >
            <div>
              <span className="muted">CPU：</span>
              {hw.cpu_brand}（{hw.cpu_physical_cores} 物理核 / {hw.cpu_logical_cores} 逻辑线程）
            </div>
            <div style={{ marginTop: "0.35rem" }}>
              <span className="muted">内存：</span>
              共 {hw.memory_total_mb} MB，可用 {hw.memory_available_mb} MB
            </div>
            <div style={{ marginTop: "0.35rem" }}>
              <span className="muted">GPU：</span>
              {hw.gpus.length === 0
                ? "未检测到显示适配器（或仅软件渲染）"
                : hw.gpus
                    .map(
                      (g) =>
                        `${g.name}${
                          g.memory_total_mb != null
                            ? `（约 ${g.memory_total_mb} MB 显存）`
                            : ""
                        }`,
                    )
                    .join("；")}
            </div>
            {!hw.nvidia_nvml_available && hw.gpus.length > 0 ? (
              <div className="muted" style={{ marginTop: "0.35rem" }}>
                未加载 NVIDIA NVML：非 N 卡或驱动未就绪时可能无显存数值；已尝试列出系统显卡名称。
              </div>
            ) : null}
            <div style={{ marginTop: "0.5rem" }}>
              <span className="muted">推荐 Whisper 档位（仅供参考）：</span>
              {hw.whisper_recommended_models.join("、")}
            </div>
            <div className="muted" style={{ marginTop: "0.25rem" }}>
              推荐档位只做参考，不会自动改写当前“推理选用模型”；当前保存的默认值仍可能是 base。
            </div>
            <div className="row-actions" style={{ marginTop: "0.5rem" }}>
              <button
                type="button"
                disabled={!recommendedWhisperModel || cfg.whisper.model === recommendedWhisperModel}
                onClick={() => {
                  if (!recommendedWhisperModel) return;
                  setWhisper({ model: recommendedWhisperModel });
                  setSaveErr(null);
                  setSaveOk(`已应用推荐模型 ${recommendedWhisperModel}，记得保存配置`);
                }}
              >
                应用推荐模型
              </button>
              <button
                type="button"
                disabled={modelBusy}
                onClick={() => void refreshWhisperPanel(cfg.whisper.use_gpu)}
              >
                刷新硬件与模型列表
              </button>
            </div>
          </div>
        )}
        {modelErr && (
          <div className="test-result err" style={{ marginBottom: "0.5rem" }}>
            {modelErr}
          </div>
        )}
        <div className="field-grid two">
          <label className="field">
            <span>识别语言</span>
            <select
              value={normalizeFixedLang(cfg.whisper.recognition_lang, "auto")}
              onChange={(e) => setWhisper({ recognition_lang: e.target.value })}
            >
              {FIXED_LANG_OPTIONS.map((lang) => (
                <option key={lang} value={lang}>
                  {lang}
                </option>
              ))}
            </select>
          </label>
          <label className="field whisper-gpu-field">
            <span className="whisper-gpu-spacer" aria-hidden="true" />
            <div className="whisper-gpu-check-row">
              <input
                type="checkbox"
                checked={cfg.whisper.use_gpu}
                onChange={(e) => setWhisper({ use_gpu: e.target.checked })}
              />
              <span className="whisper-gpu-check-caption">
                使用 GPU 进行识别（有 GPU 时推荐）
              </span>
            </div>
          </label>
          <label
            className="field"
            style={{ flexDirection: "row", alignItems: "center", gap: "0.5rem" }}
          >
            <input
              type="checkbox"
              checked={cfg.whisper.enable_vad}
              onChange={(e) => setWhisper({ enable_vad: e.target.checked })}
            />
            <span>启用 VAD（过滤音乐 / 静音 / 非语音）</span>
          </label>
          <label className="field">
            <span>VAD 阈值</span>
            <input
              type="number"
              min={0.1}
              max={0.9}
              step={0.05}
              value={cfg.whisper.vad_threshold}
              onChange={(e) =>
                setWhisper({ vad_threshold: Number(e.target.value) || 0.1 })
              }
            />
          </label>
          <label className="field">
            <span>最小语音时长（毫秒）</span>
            <input
              type="number"
              min={100}
              max={5000}
              step={50}
              value={cfg.whisper.vad_min_speech_ms}
              onChange={(e) =>
                setWhisper({ vad_min_speech_ms: Number(e.target.value) || 100 })
              }
            />
          </label>
          <label className="field">
            <span>最小静音时长（毫秒）</span>
            <input
              type="number"
              min={50}
              max={3000}
              step={50}
              value={cfg.whisper.vad_min_silence_ms}
              onChange={(e) =>
                setWhisper({ vad_min_silence_ms: Number(e.target.value) || 50 })
              }
            />
          </label>
          <label className="field">
            <span>单段最大语音时长（毫秒）</span>
            <input
              type="number"
              min={3000}
              max={30000}
              step={500}
              value={cfg.whisper.vad_max_segment_ms}
              onChange={(e) =>
                setWhisper({ vad_max_segment_ms: Number(e.target.value) || 3000 })
              }
            />
          </label>
          {!cfg.whisper.enable_vad ? (
            <div className="field" style={{ gridColumn: "1 / -1" }}>
              <p className="muted" style={{ margin: 0 }}>
                关闭后仍可转写，但片头音乐、长静音场景下时间轴可能更不稳定。
              </p>
            </div>
          ) : null}
          <label className="field">
            <span>ffmpeg 可执行文件（可选）</span>
            <input
              value={cfg.whisper.ffmpeg_path}
              onChange={(e) => setWhisper({ ffmpeg_path: e.target.value })}
              placeholder="留空则从 PATH 查找 ffmpeg / ffmpeg.exe"
            />
          </label>
          <label className="field">
            <span>Whisper CLI（whisper.cpp）</span>
            <input
              value={cfg.whisper.whisper_cli_path}
              onChange={(e) => setWhisper({ whisper_cli_path: e.target.value })}
              placeholder="留空则尝试 whisper-cli / main 等"
            />
          </label>
          <div className="field" style={{ gridColumn: "1 / -1" }}>
            <p className="muted" style={{ margin: "0 0 0.5rem" }}>
              「检测转写环境」使用<strong>已保存</strong>的配置；修改路径后请先保存。
            </p>
            <button
              type="button"
              disabled={depsBusy}
              onClick={() => {
                setDepsBusy(true);
                setDepsErr(null);
                setRuntimeProgress(null);
                void checkTranscribeDeps()
                  .then((r) => {
                    setDepsResult(r);
                  })
                  .catch((e) => {
                    setDepsResult(null);
                    setDepsErr(String(e));
                  })
                  .finally(() => setDepsBusy(false));
              }}
            >
              {depsBusy ? "检测中…" : "检测转写环境"}
            </button>
            {depsBusy && runtimeProgress ? (
              <div
                className="muted"
                style={{ marginTop: "0.65rem", fontSize: "0.85rem" }}
              >
                <div>
                  {runtimeProgress.message}
                  {runtimeProgress.bytes_total
                    ? ` (${runtimeProgress.percent.toFixed(0)}%)`
                    : ""}
                </div>
                <div
                  style={{
                    height: 6,
                    borderRadius: 4,
                    background: "var(--border)",
                    marginTop: 4,
                    overflow: "hidden",
                  }}
                >
                  <div
                    className="progress-bar-fill"
                    style={{
                      height: "100%",
                      width: `${Math.max(
                        4,
                        Math.min(100, runtimeProgress.percent),
                      )}%`,
                      transition: "width 0.2s",
                    }}
                  />
                </div>
              </div>
            ) : null}
            {depsErr && (
              <div className="test-result err" style={{ marginTop: "0.5rem" }}>
                {depsErr}
              </div>
            )}
            {depsResult && (
              <div
                style={{
                  marginTop: "0.65rem",
                  fontSize: "0.85rem",
                  lineHeight: 1.5,
                }}
              >
                <div>
                  <span className="muted">ffmpeg：</span>
                  {depsResult.ffmpeg_ok ? "可用" : "不可用"}
                  {depsResult.ffmpeg_resolved
                    ? `（${depsResult.ffmpeg_resolved}）`
                    : ""}
                  <div className="muted">{depsResult.ffmpeg_detail}</div>
                </div>
                <div style={{ marginTop: "0.35rem" }}>
                  <span className="muted">Whisper CLI：</span>
                  {depsResult.whisper_ok ? "可用" : "不可用"}
                  {depsResult.whisper_resolved
                    ? `（${depsResult.whisper_resolved}）`
                    : ""}
                  <div className="muted">{depsResult.whisper_detail}</div>
                </div>
                <div style={{ marginTop: "0.35rem" }}>
                  <span className="muted">VAD：</span>
                  {depsResult.vad_enabled ? (depsResult.vad_ok ? "可用" : "不可用") : "已关闭"}
                  {depsResult.vad_model_path ? `（${depsResult.vad_model_path}）` : ""}
                  <div className="muted">{depsResult.vad_detail}</div>
                </div>
                <div style={{ marginTop: "0.35rem" }}>
                  <span className="muted">当前配置模型文件：</span>
                  {depsResult.model_ok ? "已下载" : "未就绪"}
                  {depsResult.model_path ? `（${depsResult.model_path}）` : ""}
                  <div className="muted">{depsResult.model_detail}</div>
                </div>
              </div>
            )}
          </div>
          <div className="whisper-model-mirror-row">
            <label className="whisper-mm-label" htmlFor="whisper-infer-model">
              推理选用模型
            </label>
            <div className="whisper-mm-top-right" aria-hidden="true" />
            <div className="whisper-mm-select-wrap">
              <select
                id="whisper-infer-model"
                className="whisper-mm-select"
                value={cfg.whisper.model}
                onChange={(e) => setWhisper({ model: e.target.value })}
              >
                {["tiny", "base", "small", "medium", "large-v3"].map((m) => (
                  <option key={m} value={m}>
                    {m}
                  </option>
                ))}
              </select>
            </div>
            <div className="whisper-mm-mirror">
              <label className="whisper-mm-mirror-inline">
                <input
                  type="checkbox"
                  checked={cfg.whisper.prefer_mirror}
                  onChange={(e) =>
                    setWhisper({ prefer_mirror: e.target.checked })
                  }
                />
                <span>优先使用镜像</span>
              </label>
            </div>
          </div>
          <label className="field">
            <span>下载镜像 / 基础地址</span>
            <input
              value={cfg.whisper.mirror_url}
              onChange={(e) => setWhisper({ mirror_url: e.target.value })}
              placeholder="https://hf-mirror.com/ggerganov/whisper.cpp/resolve/main"
            />
          </label>
          <label className="field">
            <span>备用下载地址（目录，与镜像二选一逻辑见规格）</span>
            <input
              value={cfg.whisper.download_url}
              onChange={(e) => setWhisper({ download_url: e.target.value })}
              placeholder="留空则用 Hugging Face 官方"
            />
          </label>
        </div>
        {whisperList && (
          <div style={{ marginTop: "0.85rem" }}>
            <div className="muted" style={{ fontSize: "0.8rem", marginBottom: "0.35rem" }}>
              模型目录：{whisperList.models_dir}
            </div>
            <div className="muted" style={{ fontSize: "0.8rem", marginBottom: "0.5rem" }}>
              当前解析下载基址：{whisperList.download_base_used}（保存配置后重新打开本页可生效）
            </div>
            {dlProgress && dlProgress.phase !== "done" ? (
              <div style={{ marginBottom: "0.5rem" }}>
                <div className="muted" style={{ fontSize: "0.8rem" }}>
                  {dlProgress.message} · {dlProgress.model_id}
                </div>
                <div
                  style={{
                    height: 6,
                    borderRadius: 4,
                    background: "var(--border)",
                    marginTop: 4,
                    overflow: "hidden",
                  }}
                >
                  <div
                    className="progress-bar-fill"
                    style={{
                      height: "100%",
                      width: `${dlProgress.percent}%`,
                      transition: "width 0.2s",
                    }}
                  />
                </div>
              </div>
            ) : null}
            <div style={{ overflowX: "auto" }}>
              <table className="whisper-model-table">
                <thead>
                  <tr>
                    <th>模型</th>
                    <th>文件名</th>
                    <th>约计大小</th>
                    <th>状态</th>
                    <th />
                  </tr>
                </thead>
                <tbody>
                  {whisperList.models.map((m) => (
                    <tr key={m.id}>
                      <td>{m.id}</td>
                      <td>
                        <code style={{ fontSize: "0.8rem" }}>{m.file_name}</code>
                      </td>
                      <td>{formatBytes(m.size_bytes_estimate)}</td>
                      <td>
                        {m.downloaded
                          ? `已下载${
                              m.local_size_bytes != null
                                ? `（${formatBytes(m.local_size_bytes)}）`
                                : ""
                            }`
                          : "未下载"}
                      </td>
                      <td>
                        <button
                          type="button"
                          disabled={modelBusy || m.downloaded}
                          onClick={() => {
                            setModelErr(null);
                            setDlProgress(null);
                            setModelBusy(true);
                            void downloadWhisperModel(m.id)
                              .catch((e) => setModelErr(String(e)))
                              .finally(() => {
                                setModelBusy(false);
                                setDlProgress(null);
                              });
                          }}
                        >
                          下载
                        </button>
                        <button
                          type="button"
                          disabled={modelBusy || !m.downloaded}
                          onClick={() => {
                            void (async () => {
                              const ok = await confirm(`删除本地文件 ${m.file_name}？`, {
                                title: "SubForge",
                                kind: "warning",
                              });
                              if (!ok) return;
                              setModelErr(null);
                              setModelBusy(true);
                              void deleteWhisperModel(m.id)
                                .then(() => refreshWhisperPanel(cfg.whisper.use_gpu))
                                .catch((e) => setModelErr(String(e)))
                                .finally(() => setModelBusy(false));
                            })();
                          }}
                        >
                          删除
                        </button>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
            <p className="muted" style={{ margin: "0.5rem 0 0", fontSize: "0.8rem" }}>
              权重为 whisper.cpp 所用 ggml 格式（ggerganov/whisper.cpp）。下载大文件时请保持网络稳定；失败时请检查镜像或代理。
            </p>
          </div>
        )}
      </section>

      <section className="card">
        <h2>翻译与原字幕分段</h2>
        <div className="field-grid two">
          <label className="field">
            <span>原字幕分段策略</span>
            <select
              value={cfg.segmentation.strategy}
              onChange={(e) => setSegmentation({ strategy: e.target.value })}
            >
              <option value="disabled">关闭（沿用 Whisper 原始分段）</option>
              <option value="auto">自动</option>
              <option value="rules_only">规则优先</option>
              <option value="llm_preferred">LLM 优先</option>
            </select>
          </label>
          <label className="field">
            <span>分段时间策略</span>
            <select
              value={cfg.segmentation.timing_mode}
              onChange={(e) => setSegmentation({ timing_mode: e.target.value })}
            >
              <option value="word_timestamps_first">词级时间优先</option>
              <option value="approximate_reflow">近似对齐</option>
            </select>
          </label>
          <label className="field">
            <span>单条字幕最大字符数</span>
            <input
              type="number"
              min={8}
              max={500}
              value={cfg.segmentation.max_chars_per_segment}
              onChange={(e) =>
                setSegmentation({
                  max_chars_per_segment: Number(e.target.value) || 42,
                })
              }
            />
          </label>
          <label className="field">
            <span>单条字幕最大时长（秒）</span>
            <input
              type="number"
              min={1}
              max={60}
              step="0.5"
              value={cfg.segmentation.max_duration_seconds}
              onChange={(e) =>
                setSegmentation({
                  max_duration_seconds: Number(e.target.value) || 6,
                })
              }
            />
          </label>
        </div>
        {cfg.segmentation.strategy === "auto" ? (
          <p className="muted" style={{ margin: "0.75rem 0 0" }}>
            自动模式下，已配置 LLM 时优先使用 LLM 断句；未配置时自动回退为规则分段。
          </p>
        ) : null}
        {cfg.segmentation.strategy === "llm_preferred" &&
        cfg.translator.engine !== "llm" ? (
          <p className="muted" style={{ margin: "0.75rem 0 0" }}>
            当前原字幕分段为「LLM 优先」。若未保存可用的 LLM 参数与 API Key，开始任务时会直接提示并阻止入队。
          </p>
        ) : null}

        <div style={{ marginTop: "1rem" }}>
          <h3 style={{ margin: "0 0 0.75rem", fontSize: "1rem" }}>翻译策略与术语</h3>
          {!translateSectionVisible ? (
            <p className="muted" style={{ margin: 0 }}>
              当前为「仅原文字幕」或翻译关闭，以下翻译策略与术语在运行流水线中不会生效。
            </p>
          ) : (
            <>
              <div className="field-grid two">
                <label className="field">
                  <span>风格</span>
                  <select
                    value={cfg.translate.style}
                    onChange={(e) => setTranslate({ style: e.target.value })}
                  >
                    <option value="literal">直译</option>
                    <option value="natural">自然表达</option>
                    <option value="term_first">术语优先</option>
                  </select>
                </label>
                <label className="field">
                  <span>每段最大字符</span>
                  <input
                    type="number"
                    min={64}
                    max={32000}
                    value={cfg.translate.max_segment_chars}
                    onChange={(e) =>
                      setTranslate({
                        max_segment_chars: Number(e.target.value) || 800,
                      })
                    }
                  />
                </label>
                <label
                  className="field"
                  style={{ flexDirection: "row", alignItems: "center", gap: "0.5rem" }}
                >
                  <input
                    type="checkbox"
                    checked={cfg.translate.keep_proper_nouns_in_source}
                    onChange={(e) =>
                      setTranslate({
                        keep_proper_nouns_in_source: e.target.checked,
                      })
                    }
                  />
                  <span>保留原文专有名词</span>
                </label>
                <label
                  className="field"
                  style={{ flexDirection: "row", alignItems: "center", gap: "0.5rem" }}
                >
                  <input
                    type="checkbox"
                    checked={cfg.translate.glossary_case_sensitive}
                    onChange={(e) =>
                      setTranslate({ glossary_case_sensitive: e.target.checked })
                    }
                  />
                  <span>术语整词匹配区分大小写</span>
                </label>
              </div>
              <div style={{ marginTop: "0.75rem" }}>
                <div className="row-actions" style={{ marginBottom: "0.5rem" }}>
                  <button type="button" onClick={addGlossaryRow}>
                    添加术语行
                  </button>
                </div>
                <div style={{ overflowX: "auto" }}>
                  <table
                    style={{ width: "100%", borderCollapse: "collapse", fontSize: "0.85rem" }}
                  >
                    <thead>
                      <tr style={{ textAlign: "left", color: "var(--muted)" }}>
                        <th style={{ padding: "0.35rem" }}>原文</th>
                        <th style={{ padding: "0.35rem" }}>译文</th>
                        <th style={{ padding: "0.35rem" }}>备注</th>
                        <th style={{ padding: "0.35rem" }} />
                      </tr>
                    </thead>
                    <tbody>
                      {cfg.translate.glossary.length === 0 ? (
                        <tr>
                          <td
                            colSpan={4}
                            className="muted"
                            style={{ padding: "0.5rem" }}
                          >
                            暂无术语。点击「添加术语行」可录入；全部留空不影响任务运行。
                          </td>
                        </tr>
                      ) : null}
                      {cfg.translate.glossary.map((row, i) => (
                        <tr key={i}>
                          <td style={{ padding: "0.25rem" }}>
                            <input
                              value={row.source}
                              onChange={(e) =>
                                patchGlossary(i, { source: e.target.value })
                              }
                            />
                          </td>
                          <td style={{ padding: "0.25rem" }}>
                            <input
                              value={row.target}
                              onChange={(e) =>
                                patchGlossary(i, { target: e.target.value })
                              }
                            />
                          </td>
                          <td style={{ padding: "0.25rem" }}>
                            <input
                              value={row.note}
                              onChange={(e) =>
                                patchGlossary(i, { note: e.target.value })
                              }
                            />
                          </td>
                          <td style={{ padding: "0.25rem" }}>
                            <button type="button" onClick={() => removeGlossary(i)}>
                              删除
                            </button>
                          </td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              </div>
            </>
          )}
        </div>
      </section>

      <section className="card">
        <h2>字幕生成</h2>
        <div className="field-grid">
          <label className="field" style={{ flexDirection: "row", alignItems: "center", gap: "0.5rem" }}>
            <input
              type="radio"
              name="submode"
              checked={cfg.subtitle.mode === "original_only"}
              onChange={() => setSubtitle({ mode: "original_only" })}
            />
            <span>仅原文字幕</span>
          </label>
          <label className="field" style={{ flexDirection: "row", alignItems: "center", gap: "0.5rem" }}>
            <input
              type="radio"
              name="submode"
              checked={cfg.subtitle.mode === "dual_files"}
              onChange={() => setSubtitle({ mode: "dual_files" })}
            />
            <span>原文字幕 + 译文字幕（两个文件）</span>
          </label>
          <label className="field" style={{ flexDirection: "row", alignItems: "center", gap: "0.5rem" }}>
            <input
              type="radio"
              name="submode"
              checked={cfg.subtitle.mode === "bilingual_single"}
              onChange={() => setSubtitle({ mode: "bilingual_single" })}
            />
            <span>单文件双语 srt</span>
          </label>
          <label className="field">
            <span>输出格式</span>
            <input value={cfg.subtitle.format} disabled />
          </label>
          <label className="field">
            <span>输出目录模式</span>
            <select
              value={cfg.subtitle.output_dir_mode}
              onChange={(e) => setSubtitle({ output_dir_mode: e.target.value })}
            >
              <option value="video_dir">视频同目录</option>
              <option value="custom">统一自定义目录（由主界面工具栏选择）</option>
            </select>
          </label>
          {cfg.subtitle.output_dir_mode === "custom" && (
            <label className="field">
              <span>自定义输出目录</span>
              <input
                value={cfg.subtitle.custom_output_dir}
                onChange={(e) =>
                  setSubtitle({ custom_output_dir: e.target.value })
                }
              />
            </label>
          )}
          <label className="field" style={{ flexDirection: "row", alignItems: "center", gap: "0.5rem" }}>
            <input
              type="checkbox"
              checked={cfg.subtitle.overwrite}
              onChange={(e) => setSubtitle({ overwrite: e.target.checked })}
            />
            <span>覆盖已存在同名字幕</span>
          </label>
        </div>
      </section>

      <section className="card">
        <h2>性能与运行</h2>
        <div className="field-grid two">
          <label className="field" style={{ flexDirection: "row", alignItems: "center", gap: "0.5rem" }}>
            <input
              type="checkbox"
              checked={cfg.runtime.auto_detect_hardware}
              onChange={(e) =>
                setRuntime({ auto_detect_hardware: e.target.checked })
              }
            />
            <span>自动检测硬件并设置并发</span>
          </label>
          <label className="field">
            <span>最大并发任务数</span>
            <input
              type="number"
              min={1}
              max={16}
              value={cfg.runtime.max_parallel_tasks}
              onChange={(e) =>
                setRuntime({
                  max_parallel_tasks: Number(e.target.value) || 1,
                })
              }
            />
          </label>
          <label className="field">
            <span>CPU 线程上限</span>
            <input
              type="number"
              min={1}
              max={256}
              value={cfg.runtime.cpu_thread_limit}
              onChange={(e) =>
                setRuntime({ cpu_thread_limit: Number(e.target.value) || 1 })
              }
            />
          </label>
          <label className="field">
            <span>任务失败后自动重试次数</span>
            <input
              type="number"
              min={0}
              max={10}
              value={cfg.runtime.task_auto_retry_max}
              onChange={(e) =>
                setRuntime({
                  task_auto_retry_max: Number(e.target.value) || 0,
                })
              }
            />
          </label>
        </div>
      </section>

      <section className="card">
        <h2>保存</h2>
        <p className="muted" style={{ marginTop: 0 }}>
          API Key 仅写入加密文件 <code>config/secrets.enc.json</code>，不会以明文出现在{" "}
          <code>app-config.json</code>。
        </p>
        <div className="row-actions">
          <button
            type="button"
            className="primary"
            disabled={busy}
            onClick={() => void onSave(false)}
          >
            {busy ? "保存中…" : "保存"}
          </button>
          <button
            type="button"
            disabled={busy || !apiKeyConfigured}
            onClick={() => void onSave(true)}
          >
            清除已保存的 API Key
          </button>
        </div>
      </section>
      <div className="settings-savebar">
        <div className="settings-savebar-text">
          <div className="settings-savebar-title">保存配置</div>
          <div className="muted">
            API Key 仅写入加密文件 <code>config/secrets.enc.json</code>，不会明文写入
            <code>app-config.json</code>。
          </div>
        </div>
        <div className="row-actions">
          <button
            type="button"
            className="primary"
            disabled={busy}
            onClick={() => void onSave(false)}
          >
            {busy ? "保存中…" : "保存"}
          </button>
          <button
            type="button"
            disabled={busy || !apiKeyConfigured}
            onClick={() => void onSave(true)}
          >
            清除已保存的 API Key
          </button>
          <Link className="link-btn" to="/">
            返回主界面
          </Link>
        </div>
      </div>
    </div>
  );
}
