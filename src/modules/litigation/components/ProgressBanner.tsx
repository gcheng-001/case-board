import { Loader2 } from "lucide-react";

import { type ProgressEvent } from "@/lib/types";
import { cn } from "@/lib/utils";

/* ------------------------------------------------------------------ */
/* 进度条:全局浮在顶部,显示后台 LLM 抽取进度                          */
/* ------------------------------------------------------------------ */

export function ProgressBanner({
  progress,
  minimized,
  onToggleMinimize,
  onClose,
}: {
  progress: ProgressEvent;
  minimized: boolean;
  onToggleMinimize: () => void;
  onClose: () => void;
}) {
  let percent = 0;
  let label = "";
  let filename: string | null = null;
  let ocrProvider: "local" | "cloud" | null = null;
  let llmProvider: "local" | "cloud" | null = null;

  switch (progress.stage) {
    case "started":
      percent = 0;
      label = `开始处理 ${progress.total} 份文档…`;
      ocrProvider = progress.ocr_provider;
      llmProvider = progress.llm_provider;
      break;
    case "doc_started":
      // 2026-05-24 i:并发场景下 index 不能算 percent(回退 bug),DocStarted 没 completed_count,
      // 这里不更新 percent(沿用上一个 DocFinished 的 percent),只更新 filename / providers
      label = `处理中 · ${progress.filename}`;
      filename = progress.filename;
      ocrProvider = progress.ocr_provider;
      llmProvider = progress.llm_provider;
      break;
    case "doc_finished":
      // 用 completed_count(单调递增),不要用 index(并发顺序乱)
      percent = Math.round((progress.completed_count / progress.total) * 100);
      label = `已完成 ${progress.completed_count} / ${progress.total}`;
      filename = progress.filename;
      break;
    case "completed":
      percent = 100;
      label = `✓ 全部完成 · 抽出 ${progress.extracted} · 跳过 ${progress.skipped} · 失败 ${progress.failed} · 用时 ${(progress.elapsed_ms / 1000).toFixed(1)} s`;
      break;
    case "error":
      percent = 0;
      label = `❌ 抽取失败:${progress.error}`;
      break;
  }

  const done = progress.stage === "completed";
  const errored = progress.stage === "error";
  const currentIndex =
    progress.stage === "doc_finished"
      ? progress.completed_count
      : progress.stage === "doc_started"
        ? null
        : 0;
  const totalCount =
    progress.stage === "doc_started" ||
    progress.stage === "doc_finished" ||
    progress.stage === "started" ||
    progress.stage === "completed"
      ? progress.total
      : 0;

  // 最小化:右下角小卡片,只显示 N/M 进度 + 百分比
  if (minimized && !errored) {
    return (
      <div className="pointer-events-auto fixed bottom-4 right-4 z-40 animate-in fade-in-0 zoom-in-90 duration-200">
        <button
          type="button"
          onClick={onToggleMinimize}
          className={cn(
            "flex items-center gap-2 rounded-full border px-3 py-2 shadow-lg backdrop-blur transition-colors",
            done
              ? "border-emerald-200/70 bg-emerald-50/95 text-emerald-800 hover:bg-emerald-100"
              : "border-border bg-card/95 hover:bg-muted",
          )}
          title="点击展开进度条"
        >
          {!done && <Loader2 className="size-3.5 animate-spin" />}
          <span className="font-mono text-xs font-medium">
            {done ? "✓" : `${currentIndex ?? "…"}/${totalCount}`}
          </span>
          <span className="font-mono text-caption text-muted-foreground">
            {percent}%
          </span>
        </button>
      </div>
    );
  }

  return (
    <div className="pointer-events-none fixed inset-x-0 top-0 z-40 flex justify-center pt-3 px-4 animate-in fade-in-0 duration-300">
      <div
        className={cn(
          "pointer-events-auto w-full max-w-3xl rounded-xl border px-4 py-3 shadow-lg backdrop-blur",
          done
            ? "border-emerald-200/70 bg-emerald-50/95"
            : errored
              ? "border-destructive/50 bg-destructive/5"
              : "border-border bg-card/95",
        )}
      >
        {/* 顶行:状态 + 百分比 */}
        <div className="flex items-center gap-2 text-xs">
          {!done && !errored && (
            <Loader2 className="size-3.5 animate-spin text-foreground shrink-0" />
          )}
          <span
            className={cn(
              "flex-1 truncate font-medium",
              done
                ? "text-emerald-800"
                : errored
                  ? "text-destructive"
                  : "text-foreground",
            )}
          >
            {label}
          </span>
          {!errored && (
            <span className="shrink-0 font-mono text-muted-foreground">
              {percent}%
            </span>
          )}
          {/* 最小化 / 关闭按钮 */}
          <div className="ml-1 flex shrink-0 items-center gap-0.5">
            {!errored && !done && (
              <button
                type="button"
                onClick={onToggleMinimize}
                className="rounded p-0.5 text-muted-foreground hover:bg-accent hover:text-foreground"
                title="最小化到右下角"
              >
                <svg className="size-3" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5">
                  <path d="M5 12h14"/>
                </svg>
              </button>
            )}
            <button
              type="button"
              onClick={onClose}
              className="rounded p-0.5 text-muted-foreground hover:bg-accent hover:text-foreground"
              title="关闭"
            >
              <svg className="size-3" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5">
                <path d="M6 6l12 12M6 18L18 6"/>
              </svg>
            </button>
          </div>
        </div>

        {/* 当前文件 */}
        {filename && (
          <div className="mt-1.5 truncate text-label text-muted-foreground">
            📄 {filename}
          </div>
        )}

        {/* 后端标签 */}
        {(ocrProvider || llmProvider) && (
          <div className="mt-2 flex flex-wrap gap-1.5">
            {ocrProvider && (
              <BackendChip type="OCR" provider={ocrProvider} />
            )}
            {llmProvider && (
              <BackendChip type="LLM" provider={llmProvider} />
            )}
          </div>
        )}

        {/* 进度条 */}
        <div className="mt-2 h-1 overflow-hidden rounded-full bg-muted">
          <div
            className={cn(
              "h-full transition-all duration-300",
              done
                ? "bg-emerald-500"
                : errored
                  ? "bg-destructive"
                  : "bg-foreground",
            )}
            style={{ width: `${percent}%` }}
          />
        </div>
      </div>
    </div>
  );
}

function BackendChip({
  type,
  provider,
}: {
  type: "OCR" | "LLM";
  provider: "local" | "cloud";
}) {
  const isLocal = provider === "local";
  const label =
    type === "OCR"
      ? isLocal
        ? "🖥️ 本机 MiniCPM-V"
        : "☁️ 云端 MinerU"
      : isLocal
        ? "🖥️ 本机 MiniCPM-V"
        : "☁️ 云端 AI";
  return (
    <span
      className={cn(
        "inline-flex items-center gap-1 rounded-full px-2 py-0.5 text-caption font-medium",
        isLocal
          ? "bg-blue-100 text-blue-900 dark:bg-blue-900/30 dark:text-blue-100"
          : "bg-amber-100 text-amber-900 dark:bg-amber-900/30 dark:text-amber-100",
      )}
    >
      <span className="font-mono text-[9px]">{type}</span>
      <span>{label}</span>
    </span>
  );
}
