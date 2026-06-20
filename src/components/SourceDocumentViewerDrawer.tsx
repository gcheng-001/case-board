/**
 * 源文件查看器抽屉(源文件看板重构 · Phase 1 · 2026-06-19)。
 *
 * 点案件源文件 → 右侧抽屉,顶部「处理后 MD / 原件」双 tab:
 * - **处理后 MD**:react-markdown 渲染抽取产物(AI 实际读到的内容,方便核对抽取质量)。
 * - **原件**:PDF/图片**板内**原生渲染(走流式 `asset://` 协议,大扫描件不占内存、自带 Range);
 *   `.docx/.xls/.ppt` 等渲不了的 → 「用系统默认程序打开」兜底。
 *
 * 铁律:只读渲染,绝不改原文件。打开前必须 `await allowCaseAssets(caseFolder)` 把案件目录
 * 加进 asset scope,否则 iframe 首次请求 403(详见 docs/源文件看板重构-执行清单.md Phase 0 结论③)。
 */
import { useEffect, useRef, useState } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { convertFileSrc } from "@tauri-apps/api/core";
import { readFile } from "@tauri-apps/plugin-fs";
import { X, Loader2, ExternalLink, FolderOpen, FileText } from "lucide-react";

import {
  allowCaseAssets,
  readTextFile,
  openInDefaultApp,
  revealInFinder,
  convertDocToDocx,
} from "@/lib/api";
import type { Document } from "@/lib/types";
import { cn } from "@/lib/utils";

type Tab = "md" | "original";

const isPdf = (name: string) => /\.pdf$/i.test(name);
const isImage = (name: string) =>
  /\.(png|jpe?g|webp|tiff?|bmp|gif|jp2)$/i.test(name);
const isNativeText = (name: string) => /\.(md|markdown|txt)$/i.test(name);
// 路 B(2026-06-19):.docx / Excel 用 JS 库板内渲染(零外部依赖、跨平台);
// 老 .doc / .rtf / .odt 先 convert_doc_to_docx(mac textutil / Win soffice)转 .docx 再渲;
// .ppt / .pptx 仍渲不了 → 退回「处理后 MD + 系统打开」。
const isDocx = (name: string) => /\.docx$/i.test(name);
const isSpreadsheet = (name: string) => /\.(xlsx?|csv)$/i.test(name);
// 非 .docx、但能转成 .docx 再渲的老格式(textutil/soffice 支持)
const isConvertibleDoc = (name: string) => /\.(doc|rtf|odt)$/i.test(name);
const isInBoardOffice = (name: string) =>
  isDocx(name) || isSpreadsheet(name) || isConvertibleDoc(name);

export function SourceDocumentViewerDrawer({
  doc,
  caseFolder,
  onClose,
}: {
  doc: Document;
  /** 案件源文件夹(用于 asset scope 授权) */
  caseFolder: string;
  onClose: () => void;
}) {
  const hasMd = doc.extraction_status === "done" && !!doc.extracted_text_path;
  const nativeText = isNativeText(doc.filename);
  // 处理后 MD 的来源:本身是文本→读原文件;否则读抽取产物
  const mdPath = nativeText ? doc.source_path : (doc.extracted_text_path ?? null);
  const canShowMd = nativeText || hasMd;

  // 默认 tab:能看处理后文本→md;否则直接原件
  const [tab, setTab] = useState<Tab>(canShowMd ? "md" : "original");

  // Esc 关闭
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  // ── 处理后 MD ──
  const [mdText, setMdText] = useState<string | null>(null);
  const [mdLoading, setMdLoading] = useState(false);
  const [mdError, setMdError] = useState<string | null>(null);
  useEffect(() => {
    if (tab !== "md" || !mdPath || mdText !== null) return;
    let cancelled = false;
    setMdLoading(true);
    setMdError(null);
    readTextFile(mdPath)
      .then((t) => !cancelled && setMdText(t))
      .catch((e) => !cancelled && setMdError(String(e)))
      .finally(() => !cancelled && setMdLoading(false));
    return () => {
      cancelled = true;
    };
  }, [tab, mdPath, mdText]);

  // ── 原件:先把案件目录加进 asset scope,再让 iframe/img 请求(避免首次 403) ──
  const [assetReady, setAssetReady] = useState(false);
  const [assetError, setAssetError] = useState<string | null>(null);
  useEffect(() => {
    if (tab !== "original" || assetReady) return;
    let cancelled = false;
    allowCaseAssets(caseFolder)
      .then(() => !cancelled && setAssetReady(true))
      .catch((e) => !cancelled && setAssetError(String(e)));
    return () => {
      cancelled = true;
    };
  }, [tab, caseFolder, assetReady]);

  const assetUrl = assetReady ? convertFileSrc(doc.source_path) : null;

  return (
    <div className="fixed inset-0 z-[110] flex justify-end bg-black/40 backdrop-blur-sm">
      {/* 点遮罩关闭 */}
      <div className="flex-1" onClick={onClose} />
      <div className="flex h-full w-[min(1080px,94vw)] flex-col bg-white shadow-2xl">
        {/* 头:文件名 + tab + 外部操作 */}
        <div className="flex items-center gap-3 border-b border-stone-200 px-5 py-3">
          <FileText className="size-5 shrink-0 text-stone-400" />
          <div className="min-w-0 flex-1">
            <p className="truncate text-sm font-medium text-stone-800">
              {doc.filename}
            </p>
          </div>
          <div className="flex shrink-0 items-center gap-1">
            <button
              onClick={() => openInDefaultApp(doc.source_path)}
              className="flex items-center gap-1 rounded px-2 py-1 text-xs text-stone-500 hover:bg-stone-100"
              title="用系统默认程序打开原件"
            >
              <ExternalLink className="size-3.5" />
              打开
            </button>
            <button
              onClick={() => revealInFinder(doc.source_path)}
              className="flex items-center gap-1 rounded px-2 py-1 text-xs text-stone-500 hover:bg-stone-100"
              title="在文件管理器中显示"
            >
              <FolderOpen className="size-3.5" />
              定位
            </button>
            <button
              onClick={onClose}
              className="ml-1 rounded p-1 text-stone-400 hover:bg-stone-100 hover:text-stone-600"
              title="关闭 (Esc)"
            >
              <X className="size-4" />
            </button>
          </div>
        </div>

        {/* tab 切换 */}
        <div className="flex shrink-0 items-center gap-1 border-b border-stone-200 px-5">
          <TabButton active={tab === "md"} onClick={() => setTab("md")} disabled={!canShowMd}>
            处理后 MD
          </TabButton>
          <TabButton active={tab === "original"} onClick={() => setTab("original")}>
            原件
          </TabButton>
          {tab === "md" && canShowMd && (
            <span className="ml-auto py-1.5 text-[11px] text-stone-400">
              图片 / 表格 / 盖章页请切「原件」核对
            </span>
          )}
        </div>

        {/* 内容 */}
        <div className="min-h-0 flex-1 overflow-auto bg-stone-50">
          {tab === "md" ? (
            !canShowMd ? (
              <Empty text="这份材料没有处理后文本,请切「原件」查看。" />
            ) : mdLoading ? (
              <Loading />
            ) : mdError ? (
              <Empty text={`读取失败:${mdError}`} />
            ) : (
              <div
                className={cn(
                  "px-6 py-5 text-sm leading-relaxed text-foreground",
                  // 简易 prose 样式(对齐 MarkdownModal,避免引入 @tailwindcss/typography)
                  "[&_h1]:mb-3 [&_h1]:mt-5 [&_h1]:text-xl [&_h1]:font-semibold",
                  "[&_h2]:mb-2 [&_h2]:mt-4 [&_h2]:text-base [&_h2]:font-semibold",
                  "[&_h3]:mb-1.5 [&_h3]:mt-3 [&_h3]:text-sm [&_h3]:font-semibold",
                  "[&_p]:my-2",
                  "[&_ul]:my-2 [&_ul]:list-disc [&_ul]:pl-6",
                  "[&_ol]:my-2 [&_ol]:list-decimal [&_ol]:pl-6",
                  "[&_li]:my-1",
                  "[&_code]:rounded [&_code]:bg-muted [&_code]:px-1 [&_code]:py-0.5 [&_code]:font-mono [&_code]:text-[12px]",
                  "[&_pre]:my-3 [&_pre]:overflow-auto [&_pre]:rounded-md [&_pre]:bg-muted [&_pre]:p-3",
                  "[&_a]:text-foreground [&_a]:underline [&_a]:underline-offset-2",
                  "[&_strong]:font-semibold",
                  "[&_blockquote]:my-3 [&_blockquote]:border-l-2 [&_blockquote]:border-border [&_blockquote]:pl-3 [&_blockquote]:text-muted-foreground",
                  "[&_table]:my-3 [&_table]:w-full [&_table]:border-collapse [&_table]:text-xs",
                  "[&_th]:border [&_th]:border-border [&_th]:bg-muted/50 [&_th]:px-2 [&_th]:py-1.5 [&_th]:text-left [&_th]:font-medium",
                  "[&_td]:border [&_td]:border-border [&_td]:px-2 [&_td]:py-1.5",
                  "[&_hr]:my-4 [&_hr]:border-border",
                )}
              >
                <ReactMarkdown remarkPlugins={[remarkGfm]}>
                  {mdText ?? ""}
                </ReactMarkdown>
              </div>
            )
          ) : (
            <OriginalView
              filename={doc.filename}
              sourcePath={doc.source_path}
              assetUrl={assetUrl}
              assetError={assetError}
              onOpenExternal={() => openInDefaultApp(doc.source_path)}
            />
          )}
        </div>
      </div>
    </div>
  );
}

function OriginalView({
  filename,
  sourcePath,
  assetUrl,
  assetError,
  onOpenExternal,
}: {
  filename: string;
  sourcePath: string;
  assetUrl: string | null;
  assetError: string | null;
  onOpenExternal: () => void;
}) {
  // PDF / 图片:走 asset 流式协议
  if (isPdf(filename) || isImage(filename)) {
    if (assetError) return <Empty text={`无法加载原件:${assetError}`} />;
    if (!assetUrl) return <Loading />;
    if (isImage(filename)) {
      return (
        <div className="flex min-h-full items-start justify-center p-4">
          <img src={assetUrl} alt={filename} className="max-w-full" />
        </div>
      );
    }
    return <iframe src={assetUrl} title={filename} className="h-full w-full border-0" />;
  }
  // .docx / Excel:JS 库板内渲染(读字节)
  if (isInBoardOffice(filename)) {
    return (
      <OfficeView
        path={sourcePath}
        filename={filename}
        onOpenExternal={onOpenExternal}
      />
    );
  }
  // 老 .doc / .ppt / .rtf / .odt 等渲不了 → 系统打开兜底(内容仍可看「处理后 MD」)
  return (
    <div className="flex h-full flex-col items-center justify-center gap-3 text-stone-500">
      <FileText className="size-10 text-stone-300" />
      <p className="px-6 text-center text-sm">
        这种格式无法板内渲染原件。
        <br />
        文字内容可看上方「处理后 MD」,或用系统程序打开原件。
      </p>
      <button
        onClick={onOpenExternal}
        className="flex items-center gap-1.5 rounded-lg bg-sky-500 px-4 py-2 text-sm font-medium text-white hover:bg-sky-600"
      >
        <ExternalLink className="size-4" />
        用系统默认程序打开
      </button>
    </div>
  );
}

/** .docx(docx-preview)/ Excel(SheetJS)板内渲染:读字节 → 渲进容器。库都是动态 import(代码分包)。 */
function OfficeView({
  path,
  filename,
  onOpenExternal,
}: {
  path: string;
  filename: string;
  onOpenExternal: () => void;
}) {
  const containerRef = useRef<HTMLDivElement>(null);
  const [status, setStatus] = useState<"loading" | "ok" | "error">("loading");
  const [err, setErr] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setStatus("loading");
    setErr(null);
    (async () => {
      try {
        if (isSpreadsheet(filename)) {
          // .xlsx / .xls / .csv → 每个 sheet 渲成 HTML 表格
          const bytes = await readFile(path); // office 文件小,IPC 字节可接受
          const XLSX = await import("xlsx");
          const wb = XLSX.read(bytes, { type: "array" });
          if (cancelled || !containerRef.current) return;
          containerRef.current.innerHTML = wb.SheetNames.map((name) => {
            const html = XLSX.utils.sheet_to_html(wb.Sheets[name]);
            return `<div class="mb-1 mt-3 text-xs font-semibold text-stone-500">${name}</div>${html}`;
          }).join("");
        } else {
          // .docx 直接渲;老 .doc/.rtf/.odt 先转成 .docx(mac textutil / Win soffice)再渲
          const docxPath = isConvertibleDoc(filename)
            ? await convertDocToDocx(path)
            : path;
          if (cancelled) return;
          const bytes = await readFile(docxPath);
          if (cancelled || !containerRef.current) return;
          containerRef.current.innerHTML = "";
          const { renderAsync } = await import("docx-preview");
          if (cancelled || !containerRef.current) return;
          await renderAsync(bytes, containerRef.current, undefined, {
            inWrapper: true,
            ignoreWidth: false,
            ignoreHeight: false,
          });
        }
        if (!cancelled) setStatus("ok");
      } catch (e) {
        if (!cancelled) {
          setErr(String(e));
          setStatus("error");
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [path, filename]);

  return (
    <div className="relative min-h-full">
      {status === "loading" && (
        <div className="absolute inset-0 flex items-center justify-center">
          <Loader2 className="size-5 animate-spin text-stone-400" />
        </div>
      )}
      {status === "error" && (
        <div className="flex h-full flex-col items-center justify-center gap-3 px-6 text-center text-sm text-stone-500">
          <p>板内渲染失败:{err}</p>
          <button
            onClick={onOpenExternal}
            className="flex items-center gap-1.5 rounded-lg bg-sky-500 px-4 py-2 text-sm font-medium text-white hover:bg-sky-600"
          >
            <ExternalLink className="size-4" />
            用系统默认程序打开
          </button>
        </div>
      )}
      {/* 容器常驻挂载(renderAsync 需要已挂载的 DOM 节点) */}
      <div
        ref={containerRef}
        className={cn(
          "bg-white px-4 py-4",
          status !== "ok" && "hidden",
          // Excel 表格基础样式
          "[&_table]:my-2 [&_table]:border-collapse [&_table]:text-xs",
          "[&_td]:border [&_td]:border-stone-200 [&_td]:px-2 [&_td]:py-1",
          "[&_th]:border [&_th]:border-stone-200 [&_th]:bg-stone-50 [&_th]:px-2 [&_th]:py-1",
        )}
      />
    </div>
  );
}

function TabButton({
  active,
  disabled,
  onClick,
  children,
}: {
  active: boolean;
  disabled?: boolean;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      onClick={onClick}
      disabled={disabled}
      className={cn(
        "border-b-2 px-3 py-2 text-sm font-medium transition",
        active
          ? "border-sky-500 text-sky-600"
          : "border-transparent text-stone-500 hover:text-stone-700",
        disabled && "cursor-not-allowed opacity-40 hover:text-stone-500",
      )}
    >
      {children}
    </button>
  );
}

function Loading() {
  return (
    <div className="flex h-full items-center justify-center text-stone-400">
      <Loader2 className="size-5 animate-spin" />
    </div>
  );
}

function Empty({ text }: { text: string }) {
  return (
    <div className="flex h-full items-center justify-center px-6 text-center text-sm text-stone-400">
      {text}
    </div>
  );
}
