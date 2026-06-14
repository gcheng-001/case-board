import { useState } from "react";
import {
  CheckCircle2,
  ChevronDown,
  CircleAlert,
  Droplets,
  FileText,
  FolderSearch,
  Loader2,
  RefreshCw,
  Sparkles,
  Trash2,
} from "lucide-react";

import { type Document, STAGE_ORDER } from "@/lib/types";
import { formatBytes } from "@/lib/format";
import { cn } from "@/lib/utils";

import { type GroupKey } from "../lib/groupByStage";

/* ------------------------------------------------------------------ */
/* 原文件区(默认折叠,展开看分组文件列表 + 统计)                       */
/* ------------------------------------------------------------------ */

export function SourceFilesSection({
  total,
  aiArtifacts,
  groups,
  onOpenDoc,
  onRevealDoc,
  onDeleteDoc,
  onReextract,
  onReextractDewatermark,
  onRefresh,
  refreshing,
  onReanalyze,
  reanalyzing,
}: {
  total: number;
  aiArtifacts: Document[];
  groups: Record<GroupKey, Document[]>;
  onOpenDoc: (doc: Document) => void;
  onRevealDoc: (doc: Document) => void;
  onDeleteDoc: (doc: Document) => void;
  /** V0.3 · 强制重抽单个源文档(抽取失败/想重抽时) */
  onReextract: (doc: Document) => void;
  /** 2026-06-13 · 去水印重识别(带水印工商调档件,强制 PP-OCRv6+去水印) */
  onReextractDewatermark: (doc: Document) => void;
  onRefresh: () => void;
  refreshing: boolean;
  /** 2026-06-11 · 重新分析:只重跑全案 LLM 分析(不重跑 OCR),分析失败后的重试入口 */
  onReanalyze: () => void;
  reanalyzing: boolean;
}) {
  const [expanded, setExpanded] = useState(false);
  const toggle = () => setExpanded((v) => !v);

  return (
    <section className="rounded-lg border border-border bg-card shadow-sm">
      {/* 2026-05-25 V0.1.5:外层从 button 改成 div + role=button,允许内部嵌套真正的「刷新」button */}
      <div
        role="button"
        tabIndex={0}
        onClick={toggle}
        onKeyDown={(e) => {
          if (e.key === "Enter" || e.key === " ") {
            e.preventDefault();
            toggle();
          }
        }}
        className="flex w-full cursor-pointer items-center justify-between px-5 py-3 text-left transition-colors hover:bg-muted/30"
      >
        <div className="flex items-center gap-2">
          <ChevronDown
            className={cn(
              "size-4 text-muted-foreground transition-transform",
              expanded ? "rotate-0" : "-rotate-90",
            )}
          />
          <h2 className="text-sm font-semibold text-foreground">
            原文件
          </h2>
          <span className="text-xs text-muted-foreground">
            {total} 份{aiArtifacts.length > 0 && ` · ${aiArtifacts.length} 份 AI 摘要`}
          </span>
        </div>
        <div className="flex items-center gap-3">
          <button
            type="button"
            onClick={(e) => {
              e.stopPropagation();
              onReanalyze();
            }}
            disabled={reanalyzing}
            title="重新跑全案 AI 分析(更新画像/报告;不重跑 OCR、不烧积分)— 分析失败或没更新时用这个"
            className={cn(
              "inline-flex items-center gap-1 rounded-md border border-border bg-background px-2.5 py-1 text-xs text-foreground transition-colors hover:bg-muted",
              reanalyzing && "cursor-wait opacity-60",
            )}
          >
            <Sparkles className={cn("size-3", reanalyzing && "animate-pulse")} />
            {reanalyzing ? "分析中…" : "重新分析"}
          </button>
          <button
            type="button"
            onClick={(e) => {
              e.stopPropagation();
              onRefresh();
            }}
            disabled={refreshing}
            title="重扫源文件夹,增量抽取新增/修改的文件"
            className={cn(
              "inline-flex items-center gap-1 rounded-md border border-border bg-background px-2.5 py-1 text-xs text-foreground transition-colors hover:bg-muted",
              refreshing && "cursor-wait opacity-60",
            )}
          >
            <RefreshCw
              className={cn("size-3", refreshing && "animate-spin")}
            />
            {refreshing ? "同步中…" : "刷新源文件"}
          </button>
          <span className="text-xs text-muted-foreground">
            {expanded ? "点击折叠" : "点击展开"}
          </span>
        </div>
      </div>

      {expanded && (
        <div className="space-y-6 border-t border-border px-5 py-5">
          <OverviewCard
            total={total}
            aiArtifacts={aiArtifacts.length}
            groups={groups}
          />
          {aiArtifacts.length > 0 && (
            <StageSection
              title="AI 摘要"
              count={aiArtifacts.length}
              docs={aiArtifacts}
              highlight
              onOpenDoc={onOpenDoc}
              onRevealDoc={onRevealDoc}
              onDeleteDoc={onDeleteDoc}
            />
          )}
          {STAGE_ORDER.map((stage) =>
            groups[stage].length > 0 ? (
              <StageSection
                key={stage}
                title={stage}
                count={groups[stage].length}
                docs={groups[stage]}
                onOpenDoc={onOpenDoc}
                onRevealDoc={onRevealDoc}
                onReextract={onReextract}
                onReextractDewatermark={onReextractDewatermark}
              />
            ) : null,
          )}
          {groups.其他.length > 0 && (
            <StageSection
              title="其他"
              count={groups.其他.length}
              docs={groups.其他}
              dim
              onOpenDoc={onOpenDoc}
              onRevealDoc={onRevealDoc}
              onReextract={onReextract}
              onReextractDewatermark={onReextractDewatermark}
            />
          )}
        </div>
      )}
    </section>
  );
}

function OverviewCard({
  total,
  aiArtifacts,
  groups,
}: {
  total: number;
  aiArtifacts: number;
  groups: Record<GroupKey, Document[]>;
}) {
  // 显式选 4 个关键诉讼阶段(立案/一审/二审/执行),其他阶段不展示在 overview
  const stats: { label: string; count: number; dim?: boolean }[] = [
    { label: "立案", count: groups.立案.length },
    { label: "一审", count: groups.一审.length },
    { label: "二审", count: groups.二审.length, dim: groups.二审.length === 0 },
    { label: "执行", count: groups.执行.length },
  ];

  return (
    <section className="rounded-lg border border-border bg-card px-5 py-4">
      <div className="grid grid-cols-2 gap-4 sm:grid-cols-3 md:grid-cols-6">
        <Stat label="总文档" count={total} primary />
        {stats.map((s) => (
          <Stat key={s.label} label={s.label} count={s.count} dim={s.dim} />
        ))}
        {aiArtifacts > 0 && (
          <Stat label="AI 产物" count={aiArtifacts} accent />
        )}
      </div>
    </section>
  );
}

function Stat({
  label,
  count,
  primary = false,
  accent = false,
  dim = false,
}: {
  label: string;
  count: number;
  primary?: boolean;
  accent?: boolean;
  dim?: boolean;
}) {
  return (
    <div>
      <div
        className={cn(
          "font-mono text-2xl font-semibold tracking-tight",
          primary
            ? "text-foreground"
            : accent
              ? "text-foreground"
              : dim
                ? "text-muted-foreground/40"
                : "text-foreground",
        )}
      >
        {count}
      </div>
      <div
        className={cn(
          "mt-0.5 text-xs",
          accent
            ? "font-medium text-foreground/80"
            : "text-muted-foreground",
        )}
      >
        {label}
      </div>
    </div>
  );
}

function StageSection({
  title,
  count,
  docs,
  highlight = false,
  dim = false,
  onOpenDoc,
  onRevealDoc,
  onDeleteDoc,
  onReextract,
  onReextractDewatermark,
}: {
  title: string;
  count: number;
  docs: Document[];
  highlight?: boolean;
  dim?: boolean;
  onOpenDoc: (doc: Document) => void;
  onRevealDoc: (doc: Document) => void;
  onDeleteDoc?: (doc: Document) => void;
  onReextract?: (doc: Document) => void;
  onReextractDewatermark?: (doc: Document) => void;
}) {
  return (
    <section>
      <div className="mb-3 flex items-baseline gap-2">
        <h2
          className={cn(
            "text-sm font-semibold",
            dim ? "text-muted-foreground" : "text-foreground",
          )}
        >
          {title}
        </h2>
        <span className="text-xs text-muted-foreground">{count}</span>
      </div>
      <ul
        className={cn(
          "divide-y divide-border rounded-lg border",
          highlight
            ? "border-foreground/15 bg-muted/30"
            : "border-border bg-card",
        )}
      >
        {docs.map((doc) => (
          <DocRow
            key={doc.id}
            doc={doc}
            highlight={highlight}
            onOpen={() => onOpenDoc(doc)}
            onReveal={() => onRevealDoc(doc)}
            onDelete={onDeleteDoc ? () => onDeleteDoc(doc) : undefined}
            onReextract={onReextract ? () => onReextract(doc) : undefined}
            onReextractDewatermark={
              onReextractDewatermark
                ? () => onReextractDewatermark(doc)
                : undefined
            }
          />
        ))}
      </ul>
    </section>
  );
}

function DocRow({
  doc,
  highlight = false,
  onOpen,
  onReveal,
  onDelete,
  onReextract,
  onReextractDewatermark,
}: {
  doc: Document;
  highlight?: boolean;
  onOpen: () => void;
  onReveal: () => void;
  onDelete?: () => void;
  onReextract?: () => void;
  onReextractDewatermark?: () => void;
}) {
  const Icon = doc.is_ai_artifact ? Sparkles : FileText;

  return (
    <li className="group flex items-center gap-3 px-4 py-2.5 text-sm transition-colors hover:bg-accent/50">
      <button
        type="button"
        onClick={onOpen}
        className="flex min-w-0 flex-1 items-center gap-3 text-left"
        title="文本类 → 在 App 内渲染;其他类型 → 用系统默认应用打开"
      >
        <Icon
          className={cn(
            "size-4 shrink-0",
            highlight ? "text-foreground" : "text-muted-foreground",
          )}
        />
        <div className="min-w-0 flex-1">
          <p className="truncate font-medium text-foreground">{doc.filename}</p>
          {doc.category && (
            <p className="mt-0.5 text-xs text-muted-foreground">{doc.category}</p>
          )}
        </div>
      </button>

      {/* V0.3 · 抽取状态(只对源文件,AI 摘要是产物不显) */}
      {!doc.is_ai_artifact && (
        <ExtractStatus
          doc={doc}
          onReextract={onReextract}
          onReextractDewatermark={onReextractDewatermark}
        />
      )}

      <span className="shrink-0 font-mono text-xs text-muted-foreground/70">
        {formatBytes(doc.size_bytes)}
      </span>

      <button
        type="button"
        onClick={(e) => {
          e.stopPropagation();
          onReveal();
        }}
        className="shrink-0 rounded p-1 text-muted-foreground/60 opacity-0 transition-all hover:bg-accent hover:text-foreground group-hover:opacity-100"
        title="在 Finder 中显示"
        aria-label="在 Finder 中显示"
      >
        <FolderSearch className="size-3.5" />
      </button>

      {onDelete && (
        <button
          type="button"
          onClick={(e) => {
            e.stopPropagation();
            onDelete();
          }}
          className="shrink-0 rounded p-1 text-muted-foreground/60 opacity-0 transition-all hover:bg-destructive/10 hover:text-destructive group-hover:opacity-100"
          title="删除这条 AI 摘要(从材料列表移除)"
          aria-label="删除这条 AI 摘要"
        >
          <Trash2 className="size-3.5" />
        </button>
      )}
    </li>
  );
}

/**
 * V0.3 · 源文件抽取状态指示 + 重抽按钮。
 *   done → 绿勾;failed → 红「抽取失败」+ 重抽;pending/processing → 抽取中;skipped → 跳过。
 * 重抽按钮:失败时常显,其余 hover 显示(允许手动强制重抽)。
 */
function ExtractStatus({
  doc,
  onReextract,
  onReextractDewatermark,
}: {
  doc: Document;
  onReextract?: () => void;
  onReextractDewatermark?: () => void;
}) {
  const status = doc.extraction_status;
  const isPdf = /\.pdf$/i.test(doc.filename);
  const onDw = doc.is_ai_artifact ? undefined : onReextractDewatermark;
  const reBtn = onReextract ? (
    <button
      type="button"
      onClick={(e) => {
        e.stopPropagation();
        onReextract();
      }}
      title="重新抽取这份文档(重跑 OCR/识别;PDF 会再用云端 OCR 积分)"
      aria-label="重新抽取"
      className={cn(
        "shrink-0 rounded p-1 transition-all hover:bg-accent hover:text-foreground",
        status === "failed"
          ? "text-destructive/80 opacity-100"
          : "text-muted-foreground/60 opacity-0 group-hover:opacity-100",
      )}
    >
      <RefreshCw className="size-3.5" />
    </button>
  ) : null;
  // 去水印重识别:仅 PDF 显示(带水印的工商调档件常见;图片用普通重识别即可)
  const dwBtn =
    onDw && isPdf ? (
      <button
        type="button"
        onClick={(e) => {
          e.stopPropagation();
          onDw();
        }}
        title="去水印重新识别:带大幅水印的工商调档件/章程改用 PP-OCRv6(纯文字)+ 去水印,关键字段更准。已被 ocr_backend_override 标记的会一直走去水印,普通「重新识别」可恢复"
        aria-label="去水印重新识别"
        className={cn(
          "shrink-0 rounded p-1 transition-all hover:bg-accent hover:text-foreground",
          doc.ocr_backend_override
            ? "text-sky-600 opacity-100"
            : "text-muted-foreground/60 opacity-0 group-hover:opacity-100",
        )}
      >
        <Droplets className="size-3.5" />
      </button>
    ) : null;

  if (status === "done") {
    return (
      <span className="flex shrink-0 items-center gap-0.5">
        <CheckCircle2 className="size-4 text-emerald-600" aria-label="已抽取" />
        {dwBtn}
        {reBtn}
      </span>
    );
  }
  if (status === "failed") {
    return (
      <span className="flex shrink-0 items-center gap-1">
        <span
          className="inline-flex items-center gap-1 rounded bg-destructive/10 px-1.5 py-0.5 text-label font-medium text-destructive"
          title="这份文档抽取失败(OCR 或字段识别出错),点右边按钮重抽"
        >
          <CircleAlert className="size-3" />
          抽取失败
        </span>
        {dwBtn}
        {reBtn}
      </span>
    );
  }
  if (status === "pending" || status === "processing") {
    return (
      <span className="flex shrink-0 items-center gap-1 text-label text-muted-foreground">
        <Loader2 className="size-3.5 animate-spin" />
        抽取中
      </span>
    );
  }
  // skipped 及其他
  return (
    <span className="flex shrink-0 items-center gap-0.5">
      <span
        className="text-label text-muted-foreground/60"
        title="律所规范/程序材料,按设计不抽取正文(仍可在 chat 里读全文);如需可手动重抽"
      >
        跳过
      </span>
      {dwBtn}
      {reBtn}
    </span>
  );
}
