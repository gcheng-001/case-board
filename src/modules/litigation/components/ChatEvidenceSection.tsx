/**
 * 2026-06-14 · 聊天录屏取证区块(挂在案件详情页)。
 * 选录屏视频 → 选档位 → spawn 本机 wechat_evidence.py → 进度事件 → PDF 列表。
 */
import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";

import { Button } from "@/components/ui/button";
import { listChatEvidenceJobs, openInDefaultApp, startChatEvidenceJob } from "@/lib/api";
import type { ChatEvidenceJob, ChatEvidenceProgress } from "@/lib/types";

type Preset = "少漏内容" | "平衡" | "更少页";
const PRESETS: Preset[] = ["少漏内容", "平衡", "更少页"];

export function ChatEvidenceSection({ caseId }: { caseId: string }) {
  const [videoPath, setVideoPath] = useState("");
  const [preset, setPreset] = useState<Preset>("少漏内容");
  const [running, setRunning] = useState(false);
  const [jobs, setJobs] = useState<ChatEvidenceJob[]>([]);
  const [progressMsg, setProgressMsg] = useState("");

  async function refresh() {
    try {
      setJobs(await listChatEvidenceJobs(caseId));
    } catch {
      /* 案件可能还没建,忽略 */
    }
  }

  useEffect(() => {
    refresh();
    const unlisten = listen<ChatEvidenceProgress>("chat-evidence-progress", (e) => {
      const p = e.payload;
      if (p.case_id !== caseId) return;
      if (p.stage === "started") {
        setProgressMsg(`正在处理:${p.video_name}(${p.preset})…视频较长可能需要几分钟`);
      } else if (p.stage === "completed") {
        setRunning(false);
        setProgressMsg(`✅ 完成,用时 ${(p.elapsed_ms / 1000).toFixed(0)} 秒`);
        refresh();
      } else if (p.stage === "error") {
        setRunning(false);
        setProgressMsg("❌ 失败:" + p.error.slice(0, 150));
        refresh();
      }
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [caseId]);

  async function pickVideo() {
    const selected = await open({
      multiple: false,
      filters: [{ name: "视频", extensions: ["mp4", "mov", "m4v", "mkv"] }],
    });
    if (typeof selected === "string") setVideoPath(selected);
  }

  async function start() {
    if (!videoPath.trim()) {
      setProgressMsg("请先选择录屏视频");
      return;
    }
    setRunning(true);
    setProgressMsg("已提交,等待处理…");
    try {
      await startChatEvidenceJob(caseId, videoPath.trim(), preset);
    } catch (e) {
      setRunning(false);
      setProgressMsg("启动失败:" + String(e));
    }
  }

  return (
    <div className="space-y-2">
      <div className="flex items-center gap-2">
        <input
          className="flex-1 rounded border border-input bg-background px-2 py-1 text-sm"
          placeholder="录屏视频路径"
          value={videoPath}
          onChange={(e) => setVideoPath(e.target.value)}
        />
        <Button type="button" size="sm" variant="outline" onClick={pickVideo}>
          选择…
        </Button>
      </div>
      <div className="flex items-center gap-3">
        <span className="text-xs text-muted-foreground">档位</span>
        {PRESETS.map((p) => (
          <label key={p} className="flex items-center gap-1 text-xs">
            <input type="radio" checked={preset === p} onChange={() => setPreset(p)} />
            {p}
            {p === "少漏内容" && <span className="text-muted-foreground">(推荐)</span>}
          </label>
        ))}
      </div>
      <Button type="button" size="sm" onClick={start} disabled={running}>
        {running ? "处理中…" : "开始取证"}
      </Button>
      {progressMsg && <p className="text-xs text-blue-600">{progressMsg}</p>}

      {jobs.length > 0 && (
        <div className="space-y-1">
          <p className="text-xs font-medium text-muted-foreground">该案件的取证记录</p>
          {jobs.map((j) => (
            <div
              key={j.id}
              className="flex items-center justify-between rounded border border-border px-2 py-1 text-xs"
            >
              <span>
                {j.preset} · {j.status === "completed" ? "✅ 完成" : j.status === "failed" ? "❌ 失败" : j.status === "running" ? "⏳ 处理中" : "排队"}
                {j.elapsed_ms != null ? ` · ${(j.elapsed_ms / 1000).toFixed(0)}s` : ""}
                {j.status === "failed" && j.error ? <span className="text-red-600" title={j.error}>（查看错误）</span> : null}
              </span>
              {j.status === "completed" && j.pdf_path ? (
                <Button
                  type="button"
                  size="sm"
                  variant="outline"
                  onClick={() => openInDefaultApp(j.pdf_path!)}
                >
                  打开 PDF
                </Button>
              ) : null}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
