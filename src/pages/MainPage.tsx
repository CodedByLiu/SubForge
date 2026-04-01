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

const STARTABLE_STATUSES = ["pending", "paused", "failed", "pause_requested"];
const PAUSABLE_STATUSES = ["queued", "running"];
const RESUMABLE_STATUSES = ["paused", "pause_requested"];

function pipelineActiveStatus(status: string): boolean {
  return ["queued", "running", "pause_requested"].includes(status);
}

function taskActionLabel(task: TaskRowDto): string {
  if (task.status === "paused" || task.status === "pause_requested") return "继续";
  if (task.status === "failed") return "重试";
  return "开始";
}

export function MainPage() {
  const [panel, setPanel] = useState<TaskListPanel | null>(null);
  const [loadErr, setLoadErr] = useState<string | null>(null);
  const [toast, setToast] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const refresh = useCallback(async () => {
    setLoadErr(null);
    try {
      const next = await listTasks();
      setPanel(next);
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
      const result = await importVideos(paths);
      const parts = [`新增 ${result.added}`];
      if (result.skipped_duplicates) parts.push(`重复 ${result.skipped_duplicates}`);
      if (result.skipped_invalid) parts.push(`无效 ${result.skipped_invalid}`);
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
      const ok = await confirm("存在执行中或排队中的任务，确认清空整个任务列表吗？", {
        title: "SubForge",
        kind: "warning",
      });
      if (!ok) return;
      force = true;
    } else {
      const ok = await confirm("确定清空当前任务列表吗？", {
        title: "SubForge",
        kind: "warning",
      });
      if (!ok) return;
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

  const canStart = (task: TaskRowDto) => STARTABLE_STATUSES.includes(task.status);
  const canPause = (task: TaskRowDto) => PAUSABLE_STATUSES.includes(task.status);
  const showTranslateColumn = panel ? panel.show_translate_column : true;
  const hasTasks = Boolean(panel?.tasks.length);
  const hasAnyRunningLike = Boolean(
    panel?.tasks.some((task) => PAUSABLE_STATUSES.includes(task.status)),
  );
  const hasAnyResumable = Boolean(
    panel?.tasks.some((task) => RESUMABLE_STATUSES.includes(task.status)),
  );
  const canPauseAll = hasTasks && hasAnyRunningLike;
  const canContinueAll = hasTasks && hasAnyResumable;

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
                const next = e.target.value as "video_dir" | "custom";
                void onOutputModeChange(next);
              }}
            >
              <option value="video_dir">视频同目录（输出到视频所在文件夹）</option>
              <option value="custom">统一输出目录</option>
            </select>
          </label>
          {panel?.output_dir_mode === "custom" ? (
            <>
              <span className="path-preview muted" title={panel.custom_output_dir}>
                {panel.custom_output_dir || "未选择"}
              </span>
              <button type="button" disabled={busy} onClick={() => void onBrowseOutput()}>
                浏览
              </button>
            </>
          ) : null}
        </div>

        <div className="main-toolbar-row wrap">
          <button type="button" disabled={busy} onClick={() => void onImport()}>
            导入
          </button>
          <button type="button" disabled={busy || !panel?.tasks.length} onClick={() => void onClear()}>
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
                  const count = await startTasks();
                  await refresh();
                  if (count > 0) showToast(`已将 ${count} 个任务设为排队`);
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
            disabled={busy || !canPauseAll}
            onClick={() => void run(() => pauseAllTasks())}
          >
            暂停全部
          </button>
          <button
            type="button"
            disabled={busy || !canContinueAll}
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

      {loadErr ? <div className="test-result err">{loadErr}</div> : null}
      {toast ? <div className="test-result ok">{toast}</div> : null}
      {panel === null && !loadErr ? (
        <p className="muted" style={{ margin: "0 0 0.65rem" }}>
          正在加载任务列表...
        </p>
      ) : null}

      <div className="table-wrap">
        <table className="task-table">
          <thead>
            <tr>
              <th>视频文件</th>
              <th>原字幕 / 状态</th>
              {showTranslateColumn ? <th>翻译 / 状态</th> : null}
              <th>进度</th>
              <th>操作</th>
            </tr>
          </thead>
          <tbody>
            {!panel?.tasks.length ? (
              <tr>
                <td colSpan={showTranslateColumn ? 5 : 4} className="muted">
                  暂无任务，点击工具栏“导入”添加视频；输出目录可在上方选择“视频同目录”或“统一输出目录”。
                </td>
              </tr>
            ) : (
              panel.tasks.map((task) => (
                <tr key={task.id}>
                  <td>
                    <div className="cell-name">{task.file_name}</div>
                    <div className="cell-meta muted">{formatSize(task.file_size)}</div>
                    {task.snapshot_summary ? (
                      <div className="cell-meta muted" title={task.snapshot_summary}>
                        {task.snapshot_summary}
                      </div>
                    ) : null}
                  </td>

                  <td>
                    <div>{task.original_status_display}</div>
                    {task.original_preview ? (
                      <div className="cell-meta" title={task.original_preview}>
                        {task.original_preview}
                      </div>
                    ) : null}
                    {task.status === "failed" && task.error_message ? (
                      <div className="cell-error" title={task.error_message}>
                        {task.error_message.length > 220
                          ? `${task.error_message.slice(0, 220)}...`
                          : task.error_message}
                      </div>
                    ) : null}
                  </td>

                  {showTranslateColumn ? (
                    <td>
                      <div>{task.will_translate ? task.translate_status_display : "-"}</div>
                      {task.will_translate && task.translated_preview ? (
                        <div className="cell-meta" title={task.translated_preview}>
                          {task.translated_preview}
                        </div>
                      ) : null}
                    </td>
                  ) : null}

                  <td>
                    {task.status === "failed" || task.status === "completed"
                      ? task.status === "completed"
                        ? "100%"
                        : "-"
                      : `${task.progress}%${task.phase ? ` · ${task.phase}` : ""}`}
                    {task.retry_attempts > 0 ? (
                      <div className="cell-meta muted">Retry {task.retry_attempts}</div>
                    ) : null}
                  </td>

                  <td className="cell-actions">
                    <button
                      type="button"
                      disabled={busy || !canStart(task)}
                      onClick={() => void run(() => startTask(task.id))}
                    >
                      {taskActionLabel(task)}
                    </button>
                    <button
                      type="button"
                      disabled={busy || !canPause(task)}
                      onClick={() => void run(() => pauseTask(task.id))}
                    >
                      暂停
                    </button>
                    <button
                      type="button"
                      disabled={busy}
                      onClick={() => {
                        void (async () => {
                          const active = pipelineActiveStatus(task.status);
                          const msg = active
                            ? `任务“${task.file_name}”正在执行或排队中，删除将请求安全取消并从列表移除，确定吗？`
                            : `删除任务“${task.file_name}”？`;
                          const ok = await confirm(msg, {
                            title: "SubForge",
                            kind: "warning",
                          });
                          if (!ok) return;
                          void run(() => deleteTask(task.id));
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
