import { useEffect, useMemo, useState, type DragEvent } from "react";
import { open as dialogOpen } from "@tauri-apps/plugin-dialog";
import { listen } from "@tauri-apps/api/event";
import {
  AlertTriangle,
  CheckCircle2,
  Download,
  FileText,
  FolderOpen,
  Loader2,
  Play,
  Square,
  Upload,
  Video,
} from "lucide-react";

import { Button } from "@/components/ui/button";
import { toast } from "@/components/ui/toast";
import {
  listCases,
  openInDefaultApp,
  revealInFinder,
  startWechatEvidenceJob,
  stopWechatEvidenceJob,
  type WechatEvidenceJob,
} from "@/lib/api";
import type { Case } from "@/lib/types";

function formatError(e: unknown): string {
  if (typeof e === "string") return e;
  if (e && typeof e === "object" && "message" in e) {
    return String((e as { message: unknown }).message);
  }
  try {
    return JSON.stringify(e);
  } catch {
    return String(e);
  }
}

function caseLabel(c: Case): string {
  const no = c.agg_case_no || c.case_no;
  const cause = c.agg_cause || c.cause;
  return [c.name, no, cause].filter(Boolean).join(" · ");
}

function fileName(path: string): string {
  return path.split(/[\\/]/).filter(Boolean).pop() ?? path;
}

const stageText: Record<string, string> = {
  export: "导出截图/PDF",
  validate: "校验证据",
  ocr: "OCR 和报告",
  ingest: "归档案件",
  done: "完成",
};

const statusTone: Record<string, string> = {
  queued: "border-muted bg-muted/30 text-muted-foreground",
  running: "border-sky-200 bg-sky-50 text-sky-800 dark:border-sky-900/60 dark:bg-sky-950/30 dark:text-sky-200",
  completed:
    "border-emerald-200 bg-emerald-50 text-emerald-800 dark:border-emerald-900/60 dark:bg-emerald-950/30 dark:text-emerald-200",
  failed:
    "border-destructive/30 bg-destructive/10 text-destructive dark:border-destructive/40",
  stopped:
    "border-amber-200 bg-amber-50 text-amber-800 dark:border-amber-900/60 dark:bg-amber-950/30 dark:text-amber-200",
};

export function WechatScreenEvidenceTool() {
  const [cases, setCases] = useState<Case[]>([]);
  const [caseId, setCaseId] = useState("");
  const [targetMode, setTargetMode] = useState<"case" | "folder">("case");
  const [targetFolder, setTargetFolder] = useState("");
  const [videoPath, setVideoPath] = useState("");
  const [strideMode, setStrideMode] = useState<"auto" | "seconds">("auto");
  const [strideSeconds, setStrideSeconds] = useState("2");
  const [runOcr, setRunOcr] = useState(true);
  const [ocrScope, setOcrScope] = useState<"selected" | "raw">("selected");
  const [cloudTextSummary, setCloudTextSummary] = useState(false);
  const [job, setJob] = useState<WechatEvidenceJob | null>(null);
  const [starting, setStarting] = useState(false);
  const [stopping, setStopping] = useState(false);

  useEffect(() => {
    listCases()
      .then((list) => {
        setCases(list);
        if (!caseId && list.length > 0) setCaseId(list[0].id);
      })
      .catch((e) => toast(`读取案件失败:${formatError(e)}`, "error"));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    const unlisten = listen<WechatEvidenceJob>("wechat-evidence-progress", (event) => {
      setJob((prev) => {
        if (!prev || prev.id === event.payload.id) return event.payload;
        return prev;
      });
    });
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, []);

  const selectedCase = useMemo(
    () => cases.find((c) => c.id === caseId) ?? null,
    [cases, caseId],
  );
  const running = job?.status === "queued" || job?.status === "running";
  const hasTarget = targetMode === "case" ? Boolean(caseId) : Boolean(targetFolder);
  const canStart = Boolean(videoPath && hasTarget && !running && !starting);

  async function pickVideo() {
    const picked = await dialogOpen({
      directory: false,
      multiple: false,
      filters: [{ name: "微信录屏", extensions: ["mp4", "mov", "m4v"] }],
      title: "选择微信聊天录屏",
    });
    if (typeof picked === "string" && picked.trim()) {
      setVideoPath(picked);
      setJob(null);
    }
  }

  async function pickTargetFolder() {
    const picked = await dialogOpen({
      directory: true,
      multiple: false,
      title: "选择录屏取证输出文件夹",
    });
    if (typeof picked === "string" && picked.trim()) {
      setTargetFolder(picked);
      setJob(null);
    }
  }

  function acceptVideoPath(path: string) {
    const lower = path.toLowerCase();
    if (!/\.(mp4|mov|m4v)$/.test(lower)) {
      toast("只支持 .mp4/.mov/.m4v 录屏文件", "error");
      return;
    }
    setVideoPath(path);
    setJob(null);
  }

  function handleDrop(e: DragEvent<HTMLDivElement>) {
    e.preventDefault();
    if (running || starting) return;
    const file = e.dataTransfer.files?.[0];
    const path = file ? ((file as File & { path?: string }).path ?? "") : "";
    if (path) {
      acceptVideoPath(path);
    } else {
      toast("没有读取到拖入文件路径，请用“选择录屏”按钮选择", "error");
    }
  }

  async function handleStart() {
    if (!canStart) return;
    setStarting(true);
    try {
      const next = await startWechatEvidenceJob({
        videoPath,
        caseId: targetMode === "case" ? caseId : null,
        targetFolder: targetMode === "folder" ? targetFolder : null,
        strideSeconds: strideMode === "auto" ? "auto" : strideSeconds,
        preserveHeadSec: 8,
        runOcr,
        ocrScope,
        cloudTextSummary,
      });
      setJob(next);
      toast("已开始微信录屏取证", "info");
    } catch (e) {
      toast(`启动失败:${formatError(e)}`, "error", 7000);
    } finally {
      setStarting(false);
    }
  }

  async function handleStop() {
    if (!job || !running) return;
    setStopping(true);
    try {
      await stopWechatEvidenceJob(job.id);
      toast("已发送停止指令,当前输出会保留", "info");
    } catch (e) {
      toast(`停止失败:${formatError(e)}`, "error");
    } finally {
      setStopping(false);
    }
  }

  const statusClass = job ? statusTone[job.status] : statusTone.queued;

  return (
    <div className="space-y-5">
      <div className="rounded-lg border border-border bg-card/50 p-4">
        <p className="text-sm leading-relaxed text-foreground">
          处理微信聊天录屏，导出可复核的截图证据 PDF，并可生成 OCR 索引和聊天记录分析报告。
          原视频、截图和 PDF 是证据本体；OCR、身份、日期、金额和法律判断都需要人工核实。
        </p>
        <p className="mt-2 text-xs text-muted-foreground">
          默认保留开头 8 秒详情页，可归档到案件材料，也可保存到本机任意文件夹:
          <span className="ml-1 font-mono">证据/微信录屏取证/</span>
        </p>
      </div>

      <section className="space-y-3 rounded-lg border border-border bg-background p-4">
        <div className="flex items-center gap-2">
          <Video className="size-4 text-foreground/70" />
          <h3 className="text-sm font-medium text-foreground">取证来源</h3>
        </div>

        <div className="grid gap-3 md:grid-cols-[1fr_auto]">
          <div
            onDragOver={(e) => e.preventDefault()}
            onDrop={handleDrop}
            className="min-w-0 rounded-md border border-dashed border-border bg-muted/20 px-3 py-3 transition-colors hover:border-foreground/30"
          >
            <div className="text-xs text-muted-foreground">录屏文件</div>
            <div className="mt-0.5 truncate text-sm text-foreground">
              {videoPath ? fileName(videoPath) : "拖入录屏文件，或点击右侧按钮选择"}
            </div>
          </div>
          <Button variant="outline" onClick={pickVideo} disabled={running || starting}>
            <Upload className="size-4" />
            选择录屏
          </Button>
        </div>

        <div className="space-y-3">
          <div className="inline-flex rounded-md border border-border bg-muted/30 p-1">
            <button
              type="button"
              onClick={() => setTargetMode("case")}
              disabled={running || starting}
              className={`rounded px-3 py-1.5 text-xs transition-colors ${
                targetMode === "case" ? "bg-background text-foreground shadow-sm" : "text-muted-foreground"
              }`}
            >
              归档到案件
            </button>
            <button
              type="button"
              onClick={() => setTargetMode("folder")}
              disabled={running || starting}
              className={`rounded px-3 py-1.5 text-xs transition-colors ${
                targetMode === "folder" ? "bg-background text-foreground shadow-sm" : "text-muted-foreground"
              }`}
            >
              保存到本地文件夹
            </button>
          </div>

          {targetMode === "case" ? (
            <div className="space-y-1.5">
              <label className="text-xs text-muted-foreground">归档到案件</label>
              <select
                value={caseId}
                onChange={(e) => setCaseId(e.target.value)}
                disabled={running || starting}
                className="w-full rounded-md border border-border bg-background px-3 py-2 text-sm outline-none focus:border-sky-400"
              >
                {cases.length === 0 && <option value="">暂无案件</option>}
                {cases.map((c) => (
                  <option key={c.id} value={c.id}>
                    {caseLabel(c)}
                  </option>
                ))}
              </select>
              {selectedCase && (
                <p className="truncate text-xs text-muted-foreground">
                  输出位置: {selectedCase.source_folder}/证据/微信录屏取证/
                </p>
              )}
            </div>
          ) : (
            <div className="grid gap-3 md:grid-cols-[1fr_auto]">
              <div className="min-w-0 rounded-md border border-border bg-muted/20 px-3 py-2">
                <div className="text-xs text-muted-foreground">本地输出文件夹</div>
                <div className="mt-0.5 truncate text-sm text-foreground">
                  {targetFolder || "未选择"}
                </div>
              </div>
              <Button variant="outline" onClick={pickTargetFolder} disabled={running || starting}>
                <Download className="size-4" />
                选择文件夹
              </Button>
            </div>
          )}
        </div>
      </section>

      <section className="space-y-3 rounded-lg border border-border bg-background p-4">
        <h3 className="text-sm font-medium text-foreground">处理选项</h3>
        <div className="grid gap-3 md:grid-cols-2">
          <div className="space-y-1.5">
            <label className="text-xs text-muted-foreground">抽帧方式</label>
            <select
              value={strideMode}
              onChange={(e) => setStrideMode(e.target.value as "auto" | "seconds")}
              disabled={running || starting}
              className="w-full rounded-md border border-border bg-background px-3 py-2 text-sm outline-none focus:border-sky-400"
            >
              <option value="auto">自动判断聊天滚动速度</option>
              <option value="seconds">每 N 秒留 1 张</option>
            </select>
          </div>
          <div className="space-y-1.5">
            <label className="text-xs text-muted-foreground">间隔秒数</label>
            <input
              type="number"
              min="0.5"
              step="0.5"
              value={strideSeconds}
              onChange={(e) => setStrideSeconds(e.target.value)}
              disabled={running || starting || strideMode === "auto"}
              className="w-full rounded-md border border-border bg-background px-3 py-2 text-sm outline-none focus:border-sky-400 disabled:opacity-50"
            />
          </div>
        </div>

        <div className="grid gap-3 md:grid-cols-3">
          <label className="flex items-center gap-2 rounded-md border border-border bg-card/40 px-3 py-2 text-sm">
            <input
              type="checkbox"
              checked={runOcr}
              onChange={(e) => setRunOcr(e.target.checked)}
              disabled={running || starting}
            />
            生成 OCR 索引
          </label>
          <select
            value={ocrScope}
            onChange={(e) => setOcrScope(e.target.value as "selected" | "raw")}
            disabled={running || starting || !runOcr}
            className="rounded-md border border-border bg-background px-3 py-2 text-sm outline-none focus:border-sky-400 disabled:opacity-50"
          >
            <option value="selected">只识别入选截图</option>
            <option value="raw">识别原始抽帧</option>
          </select>
          <label className="flex items-center gap-2 rounded-md border border-border bg-card/40 px-3 py-2 text-sm">
            <input
              type="checkbox"
              checked={cloudTextSummary}
              onChange={(e) => setCloudTextSummary(e.target.checked)}
              disabled={running || starting || !runOcr}
            />
            云端文字增强
          </label>
        </div>

        {cloudTextSummary && (
          <div className="flex items-start gap-2 rounded-md border border-amber-200 bg-amber-50 px-3 py-2 text-xs text-amber-800 dark:border-amber-900/60 dark:bg-amber-950/30 dark:text-amber-200">
            <AlertTriangle className="mt-0.5 size-3.5 shrink-0" />
            只发送 OCR 文字、录屏时间、聊天时间、截图编号和 sha256，不发送视频或截图图片。
          </div>
        )}
      </section>

      <section className="space-y-3 rounded-lg border border-border bg-background p-4">
        <div className="flex flex-wrap items-center gap-2">
          <Button onClick={handleStart} disabled={!canStart}>
            {starting ? <Loader2 className="size-4 animate-spin" /> : <Play className="size-4" />}
            开始取证
          </Button>
          <Button variant="outline" onClick={handleStop} disabled={!running || stopping}>
            {stopping ? <Loader2 className="size-4 animate-spin" /> : <Square className="size-4" />}
            停止
          </Button>
        </div>

        {job && (
          <div className={`rounded-lg border px-4 py-3 ${statusClass}`}>
            <div className="flex items-center justify-between gap-3">
              <div className="min-w-0">
                <div className="flex items-center gap-2 text-sm font-medium">
                  {job.status === "completed" ? (
                    <CheckCircle2 className="size-4" />
                  ) : job.status === "running" || job.status === "queued" ? (
                    <Loader2 className="size-4 animate-spin" />
                  ) : (
                    <AlertTriangle className="size-4" />
                  )}
                  {stageText[job.stage] ?? job.stage}
                </div>
                <p className="mt-1 text-xs">{job.message}</p>
              </div>
              <div className="text-sm tabular-nums">{job.pct}%</div>
            </div>
            <div className="mt-3 h-2 overflow-hidden rounded-full bg-background/70">
              <div
                className="h-full rounded-full bg-current transition-all"
                style={{ width: `${Math.max(0, Math.min(100, job.pct))}%` }}
              />
            </div>
            {job.error && (
              <pre className="mt-3 max-h-32 overflow-auto whitespace-pre-wrap rounded-md bg-background/70 p-2 text-xs text-destructive">
                {job.error}
              </pre>
            )}
          </div>
        )}

        {job?.outputDir && (
          <div className="flex flex-wrap gap-2">
            <Button variant="outline" size="sm" onClick={() => revealInFinder(job.outputDir!)}>
              <FolderOpen className="size-4" />
              打开输出目录
            </Button>
            {job.pdfPath && (
              <Button variant="outline" size="sm" onClick={() => openInDefaultApp(job.pdfPath!)}>
                <FileText className="size-4" />
                打开 PDF
              </Button>
            )}
            {job.reportPath && (
              <Button variant="outline" size="sm" onClick={() => openInDefaultApp(job.reportPath!)}>
                <FileText className="size-4" />
                查看报告
              </Button>
            )}
          </div>
        )}
      </section>
    </div>
  );
}
