/**
 * V0.2 D6 · 工具调用 trace 时间线。
 *
 * 用户视角:看到 AI 在背后调了哪几个工具、命中本地缓存还是去查元典、耗时多久、有没有出错。
 * 系统视角:渲染 `ToolCallRecord[]`(后端 agent_loop 流式发上来的或 chat_tasks.tool_calls_json
 *           落库后回放的),按时间正序展示,折叠态紧凑一行,展开看 args/error 全文。
 *
 * 设计要点(详 § 20 D6 acceptance):
 *   - 四种状态图标:
 *     - 🟢 本地 KB 命中(kb_hit=true, success=true)— 零积分,瞬秒
 *     - 🌐 在线查元典 (kb_hit=false, success=true) — 积分 > 0
 *     - 🟡 LLM 直答 (本组件不渲染,Markdown 正文里;留个 case 防 future)
 *     - ⚠️ 失败 (success=false)
 *   - 单行显示:icon + tool_name + 参数摘要 + (积分 + 耗时) + ✓
 *   - 点击行展开 args(JSON pretty) + error 全文(若有)
 *   - `live` 模式(流式中):最后一行显示加载指示
 *   - 空数组 → 返回 null,**不占位**(没工具调就别留个空 trace 区)
 */

import { useState } from "react";
import { ChevronDown, ChevronRight, Loader2 } from "lucide-react";

import type { ToolCallRecord } from "@/lib/types";
import { cn } from "@/lib/utils";

interface Props {
  records: ToolCallRecord[];
  /** true = 流式还在跑,显示最后一行加载指示 */
  live?: boolean;
  /** thinking 模型本段推理已累计字数:>0 时 live 行显示「深度推理中…(N 字)」(字数在涨=没卡死)。 */
  reasoningChars?: number;
}

export function ToolCallTrace({
  records,
  live = false,
  reasoningChars = 0,
}: Props) {
  if (records.length === 0 && !live) return null;

  return (
    <div className="my-1.5 rounded-md border border-border/60 bg-muted/30 px-2 py-1.5 text-xs">
      <ul className="space-y-0.5">
        {records.map((r, i) => (
          <TraceRow key={`${r.tool}-${r.started_at_ms}-${i}`} record={r} />
        ))}
        {live && (
          <li className="flex items-center gap-1.5 px-1 py-0.5 text-muted-foreground">
            <Loader2 className="size-3 animate-spin" />
            <span>
              {reasoningChars > 0
                ? `🧠 深度推理中…(已 ${reasoningChars.toLocaleString()} 字)`
                : "正在思考下一步…"}
            </span>
          </li>
        )}
      </ul>
    </div>
  );
}

interface RowProps {
  record: ToolCallRecord;
}

function TraceRow({ record }: RowProps) {
  const [open, setOpen] = useState(false);

  const icon = statusIcon(record);
  const colorClass = statusColor(record);
  const elapsedMs = Math.max(0, record.finished_at_ms - record.started_at_ms);

  return (
    <li>
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        className="flex w-full items-center gap-1.5 rounded px-1 py-0.5 text-left hover:bg-accent/50"
      >
        {open ? (
          <ChevronDown className="size-3 shrink-0 text-muted-foreground" />
        ) : (
          <ChevronRight className="size-3 shrink-0 text-muted-foreground" />
        )}
        <span aria-hidden className="shrink-0 text-sm leading-none">
          {icon}
        </span>
        <span className={cn("font-mono text-label font-medium", colorClass)}>
          {record.tool}
        </span>
        <span className="min-w-0 flex-1 truncate text-label text-muted-foreground">
          {argsSummary(record.args)}
        </span>
        <span className="shrink-0 text-caption tabular-nums text-muted-foreground">
          {formatMeta(record, elapsedMs)}
        </span>
      </button>
      {open && (
        <div className="ml-5 my-1 space-y-1.5 rounded border border-border/40 bg-background/60 p-2 text-label">
          <DetailBlock label="args">
            <pre className="overflow-x-auto whitespace-pre-wrap break-all font-mono text-caption leading-relaxed text-muted-foreground">
              {jsonPretty(record.args)}
            </pre>
          </DetailBlock>
          {record.error_short && (
            <DetailBlock label="error">
              <pre className="overflow-x-auto whitespace-pre-wrap break-all font-mono text-caption leading-relaxed text-destructive">
                {record.error_short}
              </pre>
            </DetailBlock>
          )}
          <DetailBlock label="meta">
            <span className="font-mono text-caption text-muted-foreground">
              {elapsedMs.toLocaleString()} ms
              {record.credits_used > 0 && ` · ${record.credits_used} 积分`}
              {record.kb_hit && " · 本地 KB 命中"}
              {!record.success && " · 失败"}
            </span>
          </DetailBlock>
        </div>
      )}
    </li>
  );
}

function DetailBlock({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="space-y-0.5">
      <div className="text-caption font-medium uppercase tracking-wider text-muted-foreground/70">
        {label}
      </div>
      {children}
    </div>
  );
}

/** 选 emoji。规则见组件 doc。 */
function statusIcon(r: ToolCallRecord): string {
  if (!r.success) return "⚠️";
  if (r.kb_hit) return "🟢";
  return "🌐";
}

/** tool 名颜色:🟢 本地绿,🌐 在线蓝,⚠️ 红。 */
function statusColor(r: ToolCallRecord): string {
  if (!r.success) return "text-destructive";
  if (r.kb_hit) return "text-emerald-700 dark:text-emerald-400";
  return "text-sky-700 dark:text-sky-400";
}

/** 末尾元信息:✓ + 耗时 + 积分(如果有)。失败时不带 ✓。 */
function formatMeta(r: ToolCallRecord, elapsedMs: number): string {
  const parts: string[] = [];
  parts.push(formatElapsed(elapsedMs));
  if (r.credits_used > 0) parts.push(`-${r.credits_used}`);
  parts.push(r.success ? "✓" : "✗");
  return parts.join(" ");
}

function formatElapsed(ms: number): string {
  if (ms < 1000) return `${ms}ms`;
  return `${(ms / 1000).toFixed(1)}s`;
}

/** args 单行摘要:把 args object 压成 `k=v, k=v` 短格式,超 60 字截断。 */
function argsSummary(args: unknown): string {
  if (args == null) return "";
  if (typeof args !== "object") return String(args);
  const obj = args as Record<string, unknown>;
  const pairs: string[] = [];
  for (const [k, v] of Object.entries(obj)) {
    const vs = inlineValue(v);
    if (vs == null) continue;
    pairs.push(`${k}=${vs}`);
  }
  const joined = pairs.join(", ");
  return joined.length > 60 ? `${joined.slice(0, 57)}…` : joined;
}

function inlineValue(v: unknown): string | null {
  if (v == null) return null;
  if (typeof v === "string") {
    const s = v.length > 24 ? `"${v.slice(0, 22)}…"` : `"${v}"`;
    return s;
  }
  if (typeof v === "number" || typeof v === "boolean") return String(v);
  if (Array.isArray(v)) return `[${v.length}]`;
  return "{…}";
}

function jsonPretty(v: unknown): string {
  try {
    return JSON.stringify(v, null, 2);
  } catch {
    return String(v);
  }
}
