/**
 * 合同审查工具(非诉 tab · 2026-06-17)。
 *
 * 上传合同 .docx → 选我方立场 + 审查口径 → LLM 三层审查(交易结构/文本形式/条款语言)→
 * 风险清单(P0/P1/P2)+ 审查结论 → 导出审查意见书 Word。
 *
 * 后端命令 review_contract_docx / export_contract_opinion_docx(contract_review 模块)。
 * P1:审查闭环 + 意见书(干净稿)。P2/P3 再加批注版 / 修订痕迹版 docx。
 * 错误真错透传(坑 #8)。
 */

import { useEffect, useState } from "react";
import { open as dialogOpen, save as dialogSave } from "@tauri-apps/plugin-dialog";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import type { UnlistenFn } from "@tauri-apps/api/event";
import {
  AlertTriangle,
  CheckCircle2,
  FileText,
  Gavel,
  Loader2,
  ScrollText,
  ShieldAlert,
  Upload,
} from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  convertDocToDocx,
  exportContractOpinionDocx,
  exportContractRedlineDocx,
  reviewContractDocx,
  type ContractReviewResponse,
  type RedlineSummary,
  type ReviewRisk,
} from "@/lib/api";

/** 合同审查支持的文件类型:.docx 直接用;.doc/.rtf/.odt 先自动转 .docx 再处理。 */
const DIRECT_EXT = "docx";
const CONVERT_EXTS = ["doc", "rtf", "odt"];
const SUPPORTED_EXTS = [DIRECT_EXT, ...CONVERT_EXTS];
function extOf(p: string): string {
  return (p.split(".").pop() || "").toLowerCase();
}

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

type Stance = "party_a" | "party_b" | "neutral";
type Strictness = "lenient" | "normal" | "aggressive";

const STANCE_OPTIONS: { id: Stance; label: string; hint: string }[] = [
  { id: "party_a", label: "我方代表甲方", hint: "优先护甲方" },
  { id: "party_b", label: "我方代表乙方", hint: "优先护乙方" },
  { id: "neutral", label: "中立审查", hint: "不偏向任一方" },
];

const STRICTNESS_OPTIONS: { id: Strictness; label: string; hint: string }[] = [
  { id: "lenient", label: "克制", hint: "只挑硬伤与高风险" },
  { id: "normal", label: "常规", hint: "标准力度" },
  { id: "aggressive", label: "强势", hint: "逐条挑剔、尽量争取" },
];

/** 风险等级 → 配色(P0 红 / P1 橙 / P2 灰)。 */
function levelStyle(level: string): { badge: string; card: string; label: string } {
  const lv = (level || "").toUpperCase();
  if (lv === "P0")
    return {
      badge: "bg-red-100 text-red-700 dark:bg-red-950/40 dark:text-red-300",
      card: "border-red-200/70 dark:border-red-900/40",
      label: "P0 优先处理",
    };
  if (lv === "P1")
    return {
      badge: "bg-amber-100 text-amber-700 dark:bg-amber-950/40 dark:text-amber-300",
      card: "border-amber-200/70 dark:border-amber-900/40",
      label: "P1 建议修改",
    };
  return {
    badge: "bg-slate-100 text-slate-600 dark:bg-slate-800/60 dark:text-slate-300",
    card: "border-border",
    label: "P2 优化项",
  };
}

/** 审查结论 verdict → 配色。 */
function verdictStyle(verdict: string): string {
  const v = verdict || "";
  if (v.includes("不建议"))
    return "bg-red-50 border-red-200 text-red-700 dark:bg-red-950/20 dark:border-red-900/40 dark:text-red-300";
  if (v.includes("有条件"))
    return "bg-amber-50 border-amber-200 text-amber-800 dark:bg-amber-950/20 dark:border-amber-900/40 dark:text-amber-300";
  if (v.includes("可签"))
    return "bg-emerald-50 border-emerald-200 text-emerald-700 dark:bg-emerald-950/20 dark:border-emerald-900/40 dark:text-emerald-300";
  return "bg-sky-50 border-sky-200 text-sky-800 dark:bg-sky-950/20 dark:border-sky-900/40 dark:text-sky-300";
}

function RiskCard({ risk, index }: { risk: ReviewRisk; index: number }) {
  const s = levelStyle(risk.level);
  return (
    <div className={`rounded-lg border bg-card p-4 ${s.card}`}>
      <div className="flex items-start gap-2">
        <span
          className={`mt-0.5 shrink-0 rounded px-1.5 py-0.5 text-caption font-semibold ${s.badge}`}
        >
          {s.label}
        </span>
        <div className="min-w-0 flex-1">
          <h4 className="text-sm font-medium text-foreground">
            {index}. {risk.title}
            {risk.clause_ref && (
              <span className="ml-2 text-xs font-normal text-muted-foreground">
                {risk.clause_ref}
              </span>
            )}
          </h4>
        </div>
        {risk.action === "revise" && (
          <span className="shrink-0 rounded bg-sky-50 px-1.5 py-0.5 text-caption text-sky-700 dark:bg-sky-950/30 dark:text-sky-300">
            建议改正文
          </span>
        )}
      </div>

      <div className="mt-2 space-y-1.5 text-xs leading-relaxed">
        {risk.consequence && (
          <p className="text-foreground/80">
            <span className="text-muted-foreground">风险后果:</span>
            {risk.consequence}
          </p>
        )}
        {risk.anchor_text && (
          <p className="rounded bg-muted/50 px-2 py-1 font-mono text-foreground/70">
            <span className="text-muted-foreground">原文:</span>
            {risk.anchor_text}
          </p>
        )}
        {risk.suggestion && (
          <p className="text-foreground/80">
            <span className="text-muted-foreground">整改建议:</span>
            {risk.suggestion}
          </p>
        )}
        {risk.recommended_text && (
          <p className="rounded border border-sky-200/60 bg-sky-50/50 px-2 py-1 text-sky-900 dark:border-sky-900/40 dark:bg-sky-950/20 dark:text-sky-200">
            <span className="text-sky-600 dark:text-sky-400">推荐措辞:</span>
            {risk.recommended_text}
          </p>
        )}
        {risk.basis && (
          <p className="text-muted-foreground">
            <span>法律依据:</span>
            {risk.basis}
          </p>
        )}
      </div>
    </div>
  );
}

export function ContractReviewTool() {
  const [docxPath, setDocxPath] = useState<string | null>(null);
  const [stance, setStance] = useState<Stance>("neutral");
  const [strictness, setStrictness] = useState<Strictness>("normal");
  const [hint, setHint] = useState("");

  const [reviewing, setReviewing] = useState(false);
  const [exporting, setExporting] = useState(false);
  const [converting, setConverting] = useState(false);
  const [dragging, setDragging] = useState(false);
  const [busyMsg, setBusyMsg] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [resp, setResp] = useState<ContractReviewResponse | null>(null);
  const [redlineInfo, setRedlineInfo] = useState<RedlineSummary | null>(null);

  const fileName = docxPath ? docxPath.split(/[\\/]/).pop() : null;

  // 选中 / 拖入一个文件:.docx 直接用;旧版 .doc/.rtf/.odt 先调后端转 .docx 再用。
  async function handleFileSelected(path: string) {
    setError(null);
    setResp(null);
    setRedlineInfo(null);
    const ext = extOf(path);
    if (ext === DIRECT_EXT) {
      setDocxPath(path);
      return;
    }
    if (CONVERT_EXTS.includes(ext)) {
      setConverting(true);
      setBusyMsg(`正在把 .${ext} 转换成 .docx…`);
      try {
        const converted = await convertDocToDocx(path);
        setDocxPath(converted);
        setBusyMsg("✓ 已转换为 .docx,可开始审查");
        window.setTimeout(() => setBusyMsg(""), 3000);
      } catch (e) {
        setError(formatError(e));
        setBusyMsg("");
      } finally {
        setConverting(false);
      }
      return;
    }
    setError(
      `不支持的文件类型「.${ext}」—— 请选 Word 合同(.docx,旧版 .doc/.rtf/.odt 会自动转换)`,
    );
  }

  async function handlePickFile() {
    setError(null);
    try {
      const picked = await dialogOpen({
        directory: false,
        multiple: false,
        filters: [{ name: "Word 合同", extensions: SUPPORTED_EXTS }],
      });
      if (typeof picked !== "string" || !picked.trim()) return;
      await handleFileSelected(picked);
    } catch (e) {
      setError(formatError(e));
    }
  }

  // F1(2026-06-18):把合同文件直接拖进来即可识别。onDragDropEvent 是窗口级事件,
  // 靠「本工具只在非诉 tab 挂载」把作用域收在这里(卸载即 unlisten)。
  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    getCurrentWebview()
      .onDragDropEvent((event) => {
        const p = event.payload;
        if (p.type === "enter" || p.type === "over") {
          setDragging(true);
        } else if (p.type === "drop") {
          setDragging(false);
          const path = p.paths[0];
          if (path) void handleFileSelected(path);
        } else {
          setDragging(false);
        }
      })
      .then((fn) => {
        unlisten = fn;
      })
      .catch((e) => console.warn("listen drag-drop failed", e));
    return () => {
      if (unlisten) unlisten();
    };
    // 只需挂一次;handleFileSelected 用的都是稳定 setter
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  async function handleReview() {
    if (!docxPath) return;
    setError(null);
    setResp(null);
    setRedlineInfo(null);
    setReviewing(true);
    setBusyMsg("正在通读合同、三层审查中…(约 30–90 秒)");
    try {
      const r = await reviewContractDocx(docxPath, stance, strictness, hint.trim());
      setResp(r);
    } catch (e) {
      setError(formatError(e));
    } finally {
      setReviewing(false);
      setBusyMsg("");
    }
  }

  async function handleExportOpinion() {
    if (!resp) return;
    setError(null);
    try {
      const safeName = (resp.contract_name || "合同").replace(/[\\/:*?"<>|]/g, "_").slice(0, 40);
      const picked = await dialogSave({
        defaultPath: `${safeName}_审查意见书.docx`,
        filters: [{ name: "Word 文档", extensions: ["docx"] }],
      });
      if (typeof picked !== "string" || !picked.trim()) return;
      setExporting(true);
      setBusyMsg("导出审查意见书…");
      await exportContractOpinionDocx(
        resp.result,
        resp.contract_name,
        stance,
        strictness,
        picked,
      );
      setBusyMsg("✓ 审查意见书已导出");
    } catch (e) {
      setError(formatError(e));
    } finally {
      setExporting(false);
      window.setTimeout(() => setBusyMsg(""), 6000);
    }
  }

  async function handleExportRedline() {
    if (!resp || !docxPath) return;
    setError(null);
    try {
      const safeName = (resp.contract_name || "合同").replace(/[\\/:*?"<>|]/g, "_").slice(0, 40);
      const picked = await dialogSave({
        defaultPath: `${safeName}_修订批注版.docx`,
        filters: [{ name: "Word 文档", extensions: ["docx"] }],
      });
      if (typeof picked !== "string" || !picked.trim()) return;
      setExporting(true);
      setBusyMsg("在原合同上落批注 / 修订痕迹…");
      const summary = await exportContractRedlineDocx(docxPath, resp.result, "", picked);
      setRedlineInfo(summary);
      setBusyMsg(
        `✓ 修订批注版已导出:修订 ${summary.applied_inline} 处 · 批注 ${summary.applied_comment} 处` +
          (summary.skipped.length ? ` · ${summary.skipped.length} 项仅见意见书` : ""),
      );
    } catch (e) {
      setError(formatError(e));
    } finally {
      setExporting(false);
      window.setTimeout(() => setBusyMsg(""), 8000);
    }
  }

  const busy = reviewing || exporting || converting;
  const risks = resp?.result.risks ?? [];
  const sorted = [...risks].sort((a, b) => {
    const rank = (l: string) => (l.toUpperCase() === "P0" ? 0 : l.toUpperCase() === "P1" ? 1 : 2);
    return rank(a.level) - rank(b.level);
  });
  const counts = sorted.reduce(
    (acc, r) => {
      const lv = r.level.toUpperCase();
      if (lv === "P0") acc.p0++;
      else if (lv === "P1") acc.p1++;
      else acc.p2++;
      return acc;
    },
    { p0: 0, p1: 0, p2: 0 },
  );

  const segCls = (active: boolean) =>
    `flex-1 rounded-md px-3 py-1.5 text-xs transition-colors ${
      active
        ? "bg-sky-100 font-medium text-sky-800 dark:bg-sky-950/40 dark:text-sky-200"
        : "text-muted-foreground hover:bg-accent"
    }`;

  return (
    <div className="relative space-y-5">
      {/* F1:拖入合同文件的全区遮罩提示 */}
      {dragging && (
        <div className="pointer-events-none absolute inset-0 z-50 flex flex-col items-center justify-center gap-2 rounded-lg border-2 border-dashed border-sky-400 bg-sky-50/90 backdrop-blur-sm animate-in fade-in-0 duration-150 dark:bg-sky-950/40">
          <Upload className="size-10 text-sky-500" />
          <p className="text-base font-semibold text-sky-700 dark:text-sky-300">
            松开即可载入这份合同
          </p>
          <p className="text-xs text-sky-600 dark:text-sky-400">
            支持 .docx(旧版 .doc / .rtf / .odt 会自动转换)
          </p>
        </div>
      )}

      {/* 介绍 */}
      <div className="rounded-lg border border-sky-200/70 bg-sky-50/50 p-4 dark:border-sky-900/40 dark:bg-sky-950/15">
        <div className="flex items-start gap-2.5">
          <ShieldAlert className="mt-0.5 size-4 shrink-0 text-sky-600 dark:text-sky-400" />
          <div className="text-sm leading-relaxed text-foreground">
            拖入或上传一份合同(.docx;旧版 .doc/.rtf/.odt 自动转换),AI 按「交易结构 → 文本形式 → 条款语言」三层扫描,输出分级风险清单
            (P0 优先处理 / P1 建议修改 / P2 优化项)、审查结论和推荐措辞;可导出审查意见书 Word,
            或在原合同上直接生成带**修订痕迹 + 批注**的修订版(Word「审阅」可见)。
            <span className="mt-1 block text-xs text-muted-foreground">
              结果仅供执业参考,需自行复核;不动你的源文件。
            </span>
          </div>
        </div>
      </div>

      {/* Step 1 上传 */}
      <section className="space-y-2 rounded-lg border border-border bg-background p-4">
        <div className="flex items-center gap-2">
          <FileText className="size-4 text-foreground/70" />
          <h3 className="text-sm font-medium text-foreground">① 选择合同文件</h3>
        </div>
        <div className="flex items-center gap-3">
          <Button size="sm" variant="outline" onClick={handlePickFile} disabled={busy}>
            {converting ? (
              <Loader2 className="size-3.5 animate-spin" />
            ) : (
              <Upload className="size-3.5" />
            )}
            选择合同…
          </Button>
          {fileName ? (
            <span className="truncate text-xs text-foreground/80" title={fileName}>
              {fileName}
            </span>
          ) : (
            <span className="text-xs text-muted-foreground">
              拖入或选择 Word 合同(.docx;旧版 .doc/.rtf/.odt 自动转换;扫描件请先 OCR)
            </span>
          )}
        </div>
      </section>

      {/* Step 2 立场 + 口径 */}
      <section className="space-y-3 rounded-lg border border-border bg-background p-4">
        <div className="flex items-center gap-2">
          <Gavel className="size-4 text-foreground/70" />
          <h3 className="text-sm font-medium text-foreground">② 审查立场与口径</h3>
        </div>

        <div className="space-y-1.5">
          <label className="text-xs text-muted-foreground">我方立场</label>
          <div className="flex gap-1.5 rounded-lg border border-border bg-muted/30 p-1">
            {STANCE_OPTIONS.map((o) => (
              <button
                key={o.id}
                type="button"
                onClick={() => setStance(o.id)}
                disabled={busy}
                className={segCls(stance === o.id)}
                title={o.hint}
              >
                {o.label}
              </button>
            ))}
          </div>
        </div>

        <div className="space-y-1.5">
          <label className="text-xs text-muted-foreground">审查口径</label>
          <div className="flex gap-1.5 rounded-lg border border-border bg-muted/30 p-1">
            {STRICTNESS_OPTIONS.map((o) => (
              <button
                key={o.id}
                type="button"
                onClick={() => setStrictness(o.id)}
                disabled={busy}
                className={segCls(strictness === o.id)}
                title={o.hint}
              >
                {o.label}
              </button>
            ))}
          </div>
        </div>

        <div className="space-y-1.5">
          <label className="text-xs text-muted-foreground">合同类型(可选,留空让 AI 自动判断)</label>
          <input
            value={hint}
            onChange={(e) => setHint(e.target.value)}
            disabled={busy}
            placeholder="如:房屋租赁合同 / 股权转让协议"
            className="w-full rounded-md border border-border bg-background px-3 py-1.5 text-sm outline-none focus:border-sky-400"
          />
        </div>
      </section>

      {/* 开始审查 */}
      <div className="flex items-center gap-3">
        <Button onClick={handleReview} disabled={!docxPath || busy}>
          {reviewing ? <Loader2 className="size-4 animate-spin" /> : <ShieldAlert className="size-4" />}
          开始审查
        </Button>
        {busyMsg && <span className="text-xs text-muted-foreground">{busyMsg}</span>}
      </div>

      {error && (
        <div className="rounded-md border border-destructive/30 bg-destructive/5 px-3 py-2 text-xs text-destructive">
          <p className="font-medium">出错了</p>
          <p className="mt-0.5 break-all font-mono">{error}</p>
        </div>
      )}

      {/* 审查结果 */}
      {resp && (
        <div className="space-y-4">
          {/* 结论 */}
          <section
            className={`space-y-2 rounded-lg border p-4 ${verdictStyle(resp.result.conclusion.verdict)}`}
          >
            <div className="flex flex-wrap items-center gap-2">
              <CheckCircle2 className="size-4 shrink-0" />
              <h3 className="text-sm font-semibold">
                审查结论:{resp.result.conclusion.verdict || "—"}
              </h3>
              {resp.result.contract_type && (
                <span className="rounded bg-background/60 px-1.5 py-0.5 text-caption">
                  {resp.result.contract_type}
                </span>
              )}
            </div>
            {resp.result.conclusion.summary && (
              <p className="text-xs leading-relaxed">{resp.result.conclusion.summary}</p>
            )}
            {resp.result.conclusion.preconditions.length > 0 && (
              <div className="text-xs leading-relaxed">
                <p className="font-medium">签署前先决事项:</p>
                <ol className="ml-4 list-decimal space-y-0.5">
                  {resp.result.conclusion.preconditions.map((p, i) => (
                    <li key={i}>{p}</li>
                  ))}
                </ol>
              </div>
            )}
          </section>

          {/* 统计 + 导出 */}
          <div className="flex flex-wrap items-center justify-between gap-3">
            <div className="flex items-center gap-2 text-xs">
              <AlertTriangle className="size-3.5 text-muted-foreground" />
              <span className="text-muted-foreground">
                共 {sorted.length} 项风险 · 段落 {resp.paragraph_count}
              </span>
              {counts.p0 > 0 && (
                <span className="rounded bg-red-100 px-1.5 py-0.5 font-medium text-red-700 dark:bg-red-950/40 dark:text-red-300">
                  P0 × {counts.p0}
                </span>
              )}
              {counts.p1 > 0 && (
                <span className="rounded bg-amber-100 px-1.5 py-0.5 font-medium text-amber-700 dark:bg-amber-950/40 dark:text-amber-300">
                  P1 × {counts.p1}
                </span>
              )}
              {counts.p2 > 0 && (
                <span className="rounded bg-slate-100 px-1.5 py-0.5 font-medium text-slate-600 dark:bg-slate-800/60 dark:text-slate-300">
                  P2 × {counts.p2}
                </span>
              )}
            </div>
            <div className="flex items-center gap-2">
              <Button size="sm" variant="outline" onClick={handleExportOpinion} disabled={busy}>
                {exporting ? (
                  <Loader2 className="size-3.5 animate-spin" />
                ) : (
                  <ScrollText className="size-3.5" />
                )}
                审查意见书
              </Button>
              <Button size="sm" onClick={handleExportRedline} disabled={busy}>
                {exporting ? (
                  <Loader2 className="size-3.5 animate-spin" />
                ) : (
                  <FileText className="size-3.5" />
                )}
                修订批注版
              </Button>
            </div>
          </div>

          {/* 修订版落痕摘要 */}
          {redlineInfo && (
            <div className="rounded-md border border-emerald-200 bg-emerald-50/60 px-3 py-2 text-xs text-emerald-800 dark:border-emerald-900/40 dark:bg-emerald-950/20 dark:text-emerald-300">
              已生成修订批注版:行内修订 <strong>{redlineInfo.applied_inline}</strong> 处 · 整段批注{" "}
              <strong>{redlineInfo.applied_comment}</strong> 处
              {redlineInfo.skipped.length > 0 && (
                <span>
                  {" "}
                  · 另有 {redlineInfo.skipped.length} 项无法在正文定位,仅见审查意见书
                </span>
              )}
              。用 Word 打开,在「审阅」里可见修订痕迹与批注。
            </div>
          )}

          {/* 风险清单 */}
          <div className="space-y-3">
            {sorted.length === 0 ? (
              <p className="rounded-lg border border-border bg-card p-4 text-sm text-muted-foreground">
                未识别到明显风险点。
              </p>
            ) : (
              sorted.map((r, i) => <RiskCard key={i} risk={r} index={i + 1} />)
            )}
          </div>
        </div>
      )}
    </div>
  );
}
