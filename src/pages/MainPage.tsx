import { listen } from "@tauri-apps/api/event";
import { confirm, open } from "@tauri-apps/plugin-dialog";
import { useCallback, useEffect, useState } from "react";
import { Link } from "react-router-dom";
import {
  clearTasks,
  continueAllTasks,
  deleteTask,
  importVideos,
  listTasks,
  openOutputDir,
  pauseAllTasks,
  pauseTask,
  setPanelOutput,
  startTask,
  startTasks,
} from "@/services/tasksIpc";
import type { TaskListPanel, TaskRowDto } from "@/types/tasks";

function formatSize(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / (1024 * 1024)).toFixed(1)} MB`;
}

const S_START = ["pending", "paused", "failed"];
const S_PAUSE = ["queued", "running"];

function pipelineActiveStatus(status: string): boolean {
  return ["queued", "running", "pause_requested"].includes(status);
}

export function MainPage() {
  const [panel, setPanel] = useState<TaskListPanel | null>(null);
  const [loadErr, setLoadErr] = useState<string | null>(null);
  const [toast, setToast] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const refresh = useCallback(async () => {
    setLoadErr(null);
    try {
      const p = await listTasks();
      setPanel(p);
    } catch (e) {
      setLoadErr(String(e));
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  useEffect(() => {
    if (!panel?.needs_progress_refresh) return;
    const id = window.setInterval(() => void refresh(), 900);
    return () => window.clearInterval(id);
  }, [panel?.needs_progress_refresh, refresh]);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    void listen("task-progress", () => {
      void refresh();
    }).then((fn) => {
      unlisten = fn;
    });
    return () => {
      unlisten?.();
    };
  }, [refresh]);

  const showToast = (msg: string) => {
    setToast(msg);
    window.setTimeout(() => setToast(null), 4500);
  };

  const onImport = async () => {
    const selected = await open({
      multiple: true,
      filters: [
        {
          name: "Video",
          extensions: ["mp4", "mkv", "mov", "avi", "webm"],
        },
      ],
    });
    if (selected === null) return;
    const paths = Array.isArray(selected) ? selected : [selected];
    if (paths.length === 0) return;
    setBusy(true);
    try {
      const r = await importVideos(paths);
      const parts = [`新增 ${r.added} 个`];
      if (r.skipped_duplicates) parts.push(`重复 ${r.skipped_duplicates}`);
      if (r.skipped_invalid) parts.push(`无效 ${r.skipped_invalid}`);
      showToast(parts.join("，"));
      await refresh();
    } catch (e) {
      showToast(String(e));
    } finally {
      setBusy(false);
    }
  };

  const onBrowseOutput = async () => {
    const dir = await open({ directory: true });
    if (dir === null || typeof dir !== "string") return;
    setBusy(true);
    try {
      await setPanelOutput({
        output_dir_mode: "custom",
        custom_output_dir: dir,
      });
      await refresh();
    } catch (e) {
      showToast(String(e));
    } finally {
      setBusy(false);
    }
  };

  const onOutputModeChange = async (mode: "video_dir" | "custom") => {
    if (mode === "video_dir") {
      setBusy(true);
      try {
        await setPanelOutput({
          output_dir_mode: "video_dir",
          custom_output_dir: panel?.custom_output_dir ?? "",
        });
        await refresh();
      } catch (e) {
        showToast(String(e));
      } finally {
        setBusy(false);
      }
      return;
    }
    if (!panel?.custom_output_dir?.trim()) {
      await onBrowseOutput();
      return;
    }
    setBusy(true);
    try {
      await setPanelOutput({
        output_dir_mode: "custom",
        custom_output_dir: panel.custom_output_dir,
      });
      await refresh();
    } catch (e) {
      showToast(String(e));
    } finally {
      setBusy(false);
    }
  };

  const onClear = async () => {
    if (!panel || panel.tasks.length === 0) return;
    let force = false;
    if (panel.has_active_pipeline) {
      const ok = await confirm("存在执行中的任务（提取/识别/翻译），确定要清空列表吗？", {
        title: "SubForge",
        kind: "warning",
      });
      if (!ok) {
        return;
      }
      force = true;
    } else {
      const ok = await confirm("确定清空当前任务列表？", {
        title: "SubForge",
        kind: "warning",
      });
      if (!ok) {
        return;
      }
    }
    setBusy(true);
    try {
      await clearTasks(force);
      await refresh();
    } catch (e) {
      showToast(String(e));
    } finally {
      setBusy(false);
    }
  };

  const run = async (fn: () => Promise<unknown>, okMsg?: string) => {
    setBusy(true);
    try {
      await fn();
      if (okMsg) showToast(okMsg);
      await refresh();
    } catch (e) {
      showToast(String(e));
    } finally {
      setBusy(false);
    }
  };

  const canStart = (t: TaskRowDto) => S_START.includes(t.status);
  const canPause = (t: TaskRowDto) => S_PAUSE.includes(t.status);

  const showTransCol = panel ? panel.show_translate_column : true;

  return (
    <div className="main-page">
      <header className="main-toolbar">
        <div className="main-toolbar-row">
          <h1 className="title">SubForge</h1>
          <Link className="link-btn" to="/settings">
            配置
          </Link>
        </div>
        <div className="main-toolbar-row wrap">
          <label className="inline-field">
            <span className="muted">输出目录</span>
            <select
              disabled={busy}
              value={panel?.output_dir_mode ?? "video_dir"}
              onChange={(e) => {
                const v = e.target.value as "video_dir" | "custom";
                void onOutputModeChange(v);
              }}
            >
              <option value="video_dir">视频同目录（各文件输出到其所在文件夹）</option>
              <option value="custom">统一输出目录</option>
            </select>
          </label>
          {panel?.output_dir_mode === "custom" && (
            <>
              <span className="path-preview muted" title={panel.custom_output_dir}>
                {panel.custom_output_dir || "未选择"}
              </span>
              <button type="button" disabled={busy} onClick={() => void onBrowseOutput()}>
                浏览…
              </button>
            </>
          )}
        </div>
        <div className="main-toolbar-row wrap">
          <button type="button" disabled={busy} onClick={() => void onImport()}>
            导入
          </button>
          <button
            type="button"
            disabled={busy || !panel?.tasks.length}
            onClick={() => void onClear()}
          >
            清除
          </button>
          <button
            type="button"
            className="primary"
            disabled={busy || !panel?.tasks.length}
            onClick={() => {
              void (async () => {
                setBusy(true);
                try {
                  const n = await startTasks();
                  await refresh();
                  if (n > 0) showToast(`已将 ${n} 个任务设为排队`);
                } catch (e) {
                  showToast(String(e));
                } finally {
                  setBusy(false);
                }
              })();
            }}
          >
            开始任务
          </button>
          <button
            type="button"
            disabled={busy || !panel?.tasks.length}
            onClick={() => void run(() => pauseAllTasks())}
          >
            暂停全部
          </button>
          <button
            type="button"
            disabled={busy || !panel?.tasks.length}
            onClick={() => void run(() => continueAllTasks())}
          >
            继续全部
          </button>
          <button
            type="button"
            disabled={busy || !panel?.tasks.length}
            onClick={() => void run(() => openOutputDir())}
          >
            打开输出目录
          </button>
        </div>
      </header>

      {loadErr && <div className="test-result err">{loadErr}</div>}
      {toast && <div className="test-result ok">{toast}</div>}
      {panel === null && !loadErr ? (
        <p className="muted" style={{ margin: "0 0 0.65rem" }}>
          正在加载任务列表…
        </p>
      ) : null}

      <div className="table-wrap">
        <table className="task-table">
          <thead>
            <tr>
              <th>视频文件</th>
              <th>原字幕 / 状态</th>
              {showTransCol ? <th>翻译 / 状态</th> : null}
              <th>进度</th>
              <th>操作</th>
            </tr>
          </thead>
          <tbody>
            {!panel?.tasks.length ? (
              <tr>
                <td colSpan={showTransCol ? 5 : 4} className="muted">
                  暂无任务，点击工具栏「导入」添加视频；输出目录可在上方选择「视频同目录」或「统一输出目录」。
                </td>
              </tr>
            ) : (
              panel.tasks.map((t) => (
                <tr key={t.id}>
                  <td>
                    <div className="cell-name">{t.file_name}</div>
                    <div className="cell-meta muted">
                      {formatSize(t.file_size)}
                    </div>
                    {t.snapshot_summary ? (
                      <div className="cell-meta muted" title={t.snapshot_summary}>
                        {t.snapshot_summary}
                      </div>
                    ) : null}
                  </td>
                  <td>
                    <div>{t.original_status_display}</div>
                    {t.original_preview ? (
                      <div className="cell-meta" title={t.original_preview}>
                        {t.original_preview}
                      </div>
                    ) : null}
                    {t.status === "failed" && t.error_message ? (
                      <div className="cell-error" title={t.error_message}>
                        {t.error_message.length > 220
                          ? `${t.error_message.slice(0, 220)}…`
                          : t.error_message}
                      </div>
                    ) : null}
                  </td>
                  {showTransCol ? (
                    <td>
                      <div>{t.will_translate ? t.translate_status_display : "-"}</div>
                      {t.will_translate && t.translated_preview ? (
                        <div className="cell-meta" title={t.translated_preview}>
                          {t.translated_preview}
                        </div>
                      ) : null}
                    </td>
                  ) : null}
                  <td>
                    {t.status === "failed" || t.status === "completed"
                      ? t.status === "completed"
                        ? "100%"
                        : "—"
                      : `${t.progress}%${t.phase ? ` · ${t.phase}` : ""}`}
                    {t.retry_attempts > 0 ? (
                      <div className="cell-meta muted">Retry {t.retry_attempts}</div>
                    ) : null}
                  </td>
                  <td className="cell-actions">
                    <button
                      type="button"
                      disabled={busy || !canStart(t)}
                      onClick={() => void run(() => startTask(t.id))}
                    >
                      开始
                    </button>
                    <button
                      type="button"
                      disabled={busy || !canPause(t)}
                      onClick={() => void run(() => pauseTask(t.id))}
                    >
                      暂停
                    </button>
                    <button
                      type="button"
                      disabled={busy}
                      onClick={() => {
                        void (async () => {
                        const active = pipelineActiveStatus(t.status);
                        const msg = active
                          ? `任务「${t.file_name}」正在执行或排队中，删除将请求安全取消并从列表移除，确定？`
                          : `删除任务「${t.file_name}」？`;
                        const ok = await confirm(msg, {
                          title: "SubForge",
                          kind: "warning",
                        });
                        if (!ok) return;
                        void run(() => deleteTask(t.id));
                        })();
                      }}
                    >
                      删除
                    </button>
                  </td>
                </tr>
              ))
            )}
          </tbody>
        </table>
      </div>
    </div>
  );
}
