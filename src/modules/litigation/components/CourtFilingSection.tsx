// @ts-nocheck
/**
 * 2026-06-15 · 法院一张网在线立案区块(挂在案件详情页)。
 *
 * 案件信息预填 → 立案类型 → 律师选择 → spawn CLI → 进度事件 → 验证码弹窗。
 * 复用 ChatEvidenceSection 的 listen/emit 模式。
 */
import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";

import { Button } from "@/components/ui/button";
import {
  startCourtFiling,
  listCourtFilingJobs,
  submitCaptchaAnswer,
  listLawyerProfiles,
} from "@/lib/api";
import type {
  CourtFilingJob,
  CourtFilingProgress,
  CourtFilingCaptcha,
  LawyerProfile,
} from "@/lib/types";

// 阶段名 → 中文映射
const STAGE_LABELS: Record<string, string> = {
  "cli.started": "启动",
  "cli.info": "信息",
  "cli.params": "参数",
  "filing.start": "开始立案",
  "filing.success": "✅ 到达预览页（未提交）",
  "filing.failed": "❌ 立案失败",
  "login.start": "登录一张网…",
  "login.success": "登录成功",
  "login.failed": "❌ 登录失败",
  "playwright.start": "进入浏览器立案流程",
  "playwright.step.open_case_type": "打开案件类型页",
  "playwright.step.select_court": "选择受理法院",
  "playwright.step.read_notice": "阅读立案须知",
  "playwright.step.select_cause": "选择案由",
  "playwright.step.upload_materials": "上传诉讼材料",
  "playwright.step.fill_case_info": "完善当事人信息",
  "playwright.step.next": "进入预览页",
  "playwright.step.select_execution_basis": "填写执行依据",
  "playwright.step.fill_execution_target": "填写执行标的",
  "playwright.success": "✅ 到达预览页（未提交）",
  "playwright.failed": "❌ 浏览器立案失败",
  "captcha.required": "⏳ 等待输入验证码",
  "captcha.answered": "验证码已输入",
  "captcha.timeout": "⏰ 验证码等待超时",
  "captcha.degrading": "自动识别失败，降级人工",
  "http.start": "HTTP主链路开始",
  "http.success": "HTTP主链路成功",
  "http.failed": "HTTP主链路失败",
  "cli.spawn_failed": "❌ CLI 启动失败",
};

function stageLabel(stage: string): string {
  return STAGE_LABELS[stage] || stage;
}

// 验证码弹窗
function CaptchaModal({
  captcha,
  onSubmit,
  onClose,
}: {
  captcha: CourtFilingCaptcha;
  onSubmit: (answer: string) => void;
  onClose: () => void;
}) {
  const [answer, setAnswer] = useState("");
  const [countdown, setCountdown] = useState(captcha.timeout_sec);
  const timerRef = useRef<ReturnType<typeof setInterval>>();

  useEffect(() => {
    setCountdown(captcha.timeout_sec);
    timerRef.current = setInterval(() => {
      setCountdown((c) => {
        if (c <= 1) {
          clearInterval(timerRef.current);
          onClose();
          return 0;
        }
        return c - 1;
      });
    }, 1000);
    return () => clearInterval(timerRef.current);
  }, [captcha]);

  function handleSubmit() {
    if (answer.trim()) {
      onSubmit(answer.trim());
      clearInterval(timerRef.current);
    }
  }

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50">
      <div className="w-80 rounded-lg bg-background p-4 shadow-lg space-y-3">
        <p className="text-sm font-medium">请输入验证码（第 {captcha.round} 轮）</p>
        <img
          src={captcha.image_base64}
          alt="验证码"
          className="mx-auto rounded border"
          style={{ maxHeight: 80 }}
        />
        <input
          className="w-full rounded border border-input bg-background px-2 py-1 text-sm"
          placeholder="输入验证码"
          value={answer}
          onChange={(e) => setAnswer(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && handleSubmit()}
          autoFocus
        />
        <div className="flex items-center justify-between">
          <span className="text-xs text-muted-foreground">
            {countdown > 0 ? `${countdown}s` : "已超时"}
          </span>
          <div className="flex gap-2">
            <Button type="button" size="sm" variant="outline" onClick={onClose}>
              取消
            </Button>
            <Button type="button" size="sm" onClick={handleSubmit} disabled={!answer.trim()}>
              提交
            </Button>
          </div>
        </div>
      </div>
    </div>
  );
}

export function CourtFilingSection({ caseId }: { caseId: string }) {
  const [filingType, setFilingType] = useState<"civil" | "execution">("civil");
  const [originalCaseNo, setOriginalCaseNo] = useState("");
  const [selectedAgents, setSelectedAgents] = useState<string[]>([]);
  const [lawyers, setLawyers] = useState<LawyerProfile[]>([]);
  const [jobs, setJobs] = useState<CourtFilingJob[]>([]);
  const [running, setRunning] = useState(false);
  const [progressMsg, setProgressMsg] = useState("");
  const [progressStage, setProgressStage] = useState("");
  const [captchaModal, setCaptchaModal] = useState<CourtFilingCaptcha | null>(null);

  async function refresh() {
    try {
      setJobs(await listCourtFilingJobs(caseId));
      const profiles = await listLawyerProfiles();
      setLawyers(profiles);
      // 默认选中 is_default 的律师
      if (selectedAgents.length === 0) {
        const defaults = profiles.filter((p) => p.is_default).map((p) => p.id);
        if (defaults.length > 0) setSelectedAgents(defaults);
      }
    } catch {
      /* 首次可能还没建表 */
    }
  }

  useEffect(() => {
    refresh();
    // 监听进度事件
    const unlistenProgress = listen<CourtFilingProgress>("court-filing-progress", (e) => {
      const p = e.payload;
      if (p.case_id !== caseId) return;
      setProgressMsg(p.message);
      setProgressStage(p.stage);
      if (p.stage === "filing.success" || p.stage === "playwright.success") {
        setRunning(false);
        refresh();
      }
      if (p.stage === "filing.failed" || p.stage === "playwright.failed" || p.stage === "login.failed") {
        setRunning(false);
        refresh();
      }
    });
    // 监听验证码事件
    const unlistenCaptcha = listen<CourtFilingCaptcha>("court-filing-captcha", (e) => {
      const c = e.payload;
      if (c.case_id !== caseId) return;
      setCaptchaModal(c);
    });
    return () => {
      unlistenProgress.then((fn) => fn());
      unlistenCaptcha.then((fn) => fn());
    };
  }, [caseId]);

  async function handleStart() {
    if (selectedAgents.length === 0) {
      setProgressMsg("请先选择代理律师（设置→律师档案）");
      return;
    }
    setRunning(true);
    setProgressMsg("已提交，正在启动…");
    setProgressStage("cli.started");
    try {
      await startCourtFiling(
        caseId,
        filingType,
        selectedAgents,
        filingType === "execution" ? originalCaseNo || undefined : undefined,
      );
    } catch (e) {
      setRunning(false);
      setProgressMsg("启动失败: " + String(e));
    }
  }

  async function handleCaptchaSubmit(answer: string) {
    if (!captchaModal) return;
    try {
      await submitCaptchaAnswer(
        captchaModal.job_id,
        captchaModal.task_id,
        captchaModal.round,
        answer,
      );
      setCaptchaModal(null);
      setProgressMsg("验证码已提交，继续立案…");
    } catch (e) {
      setProgressMsg("提交验证码失败: " + String(e));
    }
  }

  function toggleAgent(id: string) {
    setSelectedAgents((prev) =>
      prev.includes(id) ? prev.filter((a) => a !== id) : [...prev, id],
    );
  }

  return (
    <div className="space-y-2">
      {/* 立案类型选择 */}
      <div className="flex items-center gap-4">
        <span className="text-xs text-muted-foreground">立案类型</span>
        {(["civil", "execution"] as const).map((t) => (
          <label key={t} className="flex items-center gap-1 text-xs">
            <input type="radio" checked={filingType === t} onChange={() => setFilingType(t)} />
            {t === "civil" ? "民事一审" : "申请执行"}
          </label>
        ))}
      </div>
      {filingType === "execution" && (
        <input
          className="w-full rounded border border-input bg-background px-2 py-1 text-sm"
          placeholder="执行依据案号（必填，如 (2024)粤01民初123号）"
          value={originalCaseNo}
          onChange={(e) => setOriginalCaseNo(e.target.value)}
        />
      )}

      {/* 律师选择 */}
      {lawyers.length > 0 && (
        <div className="space-y-1">
          <span className="text-xs text-muted-foreground">代理律师</span>
          <div className="flex flex-wrap gap-2">
            {lawyers.map((l) => (
              <label
                key={l.id}
                className={`flex items-center gap-1 rounded border px-2 py-0.5 text-xs cursor-pointer ${
                  selectedAgents.includes(l.id) ? "border-primary bg-primary/5" : "border-border"
                }`}
              >
                <input
                  type="checkbox"
                  checked={selectedAgents.includes(l.id)}
                  onChange={() => toggleAgent(l.id)}
                />
                {l.name}
                {l.law_firm ? ` · ${l.law_firm}` : ""}
                {l.is_default ? " ⭐" : ""}
              </label>
            ))}
          </div>
        </div>
      )}
      {lawyers.length === 0 && (
        <p className="text-xs text-amber-600">
          未配置律师档案，请先在「设置→律师档案」中添加。
        </p>
      )}

      {/* 开始按钮 */}
      <Button type="button" size="sm" onClick={handleStart} disabled={running || selectedAgents.length === 0}>
        {running ? "立案中…" : "开始立案"}
      </Button>

      {/* 进度展示 */}
      {progressMsg && (
        <p
          className={`text-xs ${
            progressStage.includes("failed") || progressStage.includes("error")
              ? "text-red-600"
              : progressStage.includes("success")
              ? "text-green-600"
              : progressStage.includes("captcha")
              ? "text-amber-600"
              : "text-blue-600"
          }`}
        >
          {progressMsg}
        </p>
      )}

      {/* 历史任务列表 */}
      {jobs.length > 0 && (
        <div className="space-y-1">
          <p className="text-xs font-medium text-muted-foreground">立案记录</p>
          {jobs.map((j) => (
            <div
              key={j.id}
              className="flex items-center justify-between rounded border border-border px-2 py-1 text-xs"
            >
              <span>
                {j.filing_type === "civil" ? "民事" : "执行"} · {j.court_name || "—"} ·{" "}
                {j.status === "completed"
                  ? "✅ 已到预览页"
                  : j.status === "failed"
                  ? "❌ 失败"
                  : j.status === "running"
                  ? "⏳ 进行中"
                  : j.status === "waiting_captcha"
                  ? "🔑 等验证码"
                  : j.status}
                {j.timing_json ? (() => {
                  try {
                    const t = JSON.parse(j.timing_json);
                    const secs = t.overall_ms ? Math.round(t.overall_ms / 1000) : 0;
                    return secs > 0 ? ` · ${secs}秒` : null;
                  } catch { return null; }
                })() : null}
                {j.status === "failed" && j.error ? (
                  <span className="text-red-600 ml-1" title={j.error}>
                    （{j.error.slice(0, 60)}）
                  </span>
                ) : null}
              </span>
            </div>
          ))}
        </div>
      )}

      {/* 验证码弹窗 */}
      {captchaModal && (
        <CaptchaModal
          captcha={captchaModal}
          onSubmit={handleCaptchaSubmit}
          onClose={() => setCaptchaModal(null)}
        />
      )}
    </div>
  );
}
