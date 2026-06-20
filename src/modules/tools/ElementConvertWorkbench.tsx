import { useEffect, useMemo, useState } from "react";
import { open, save } from "@tauri-apps/plugin-dialog";
import { writeFile } from "@tauri-apps/plugin-fs";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import type { UnlistenFn } from "@tauri-apps/api/event";
import {
  AlertTriangle,
  ArrowLeft,
  Check,
  FileText,
  Loader2,
  Save,
  ShieldAlert,
  Sparkles,
  Upload,
} from "lucide-react";

import { Button } from "@/components/ui/button";
import { toast } from "@/components/ui/toast";
import { confirmDialog } from "@/lib/dialog";
import {
  courtElementConvert,
  externalElementConvert,
  exportElementDocument,
  generateElementDocument,
  listElementDocumentTypes,
  openInDefaultApp,
  revealInFinder,
  saveElementDocument,
  saveExternalElementDocument,
} from "@/lib/api";
import type {
  Document,
  ElementDocumentType,
  ElementDraft,
  ElementFieldValue,
} from "@/lib/types";
import { cn } from "@/lib/utils";

interface Props {
  caseId?: string;
  documents?: Document[];
  onClose: () => void;
  onSaved?: (docId: string) => void;
}

const ACCEPTED = /\.(docx?|pdf)$/i;

function basename(path: string) {
  return path.split(/[\\/]/).pop() ?? path;
}

function markdownCell(value: string): string {
  const normalized = value
    .trim()
    .replace(/\r\n/g, "\n")
    .replace(/\r/g, "\n")
    .replace(/\|/g, "\\|")
    .replace(/\n{2,}/g, "<br><br>")
    .replace(/\n/g, "<br>");
  return normalized || "[待补充]";
}

export function buildElementTableBody(fields: ElementFieldValue[]): string {
  const rows = fields.map((field, index) => [
    String(index + 1),
    markdownCell(field.label),
    markdownCell(field.value),
    markdownCell(field.evidence),
    `${Math.round(field.confidence * 100)}%`,
    field.required ? "是" : "否",
  ]);
  return [
    "| 序号 | 要素项 | 内容 | 原文依据 | 置信度 | 必填 |",
    "| --- | --- | --- | --- | --- | --- |",
    ...rows.map((row) => `| ${row.join(" | ")} |`),
  ].join("\n");
}

export function ElementConvertWorkbench({ caseId, documents = [], onClose, onSaved }: Props) {
  const [types, setTypes] = useState<ElementDocumentType[]>([]);
  const [loadingTypes, setLoadingTypes] = useState(true);
  const [sourcePath, setSourcePath] = useState("");
  const [sourceDocId, setSourceDocId] = useState("");
  const [templateId, setTemplateId] = useState("");
  const [suggestedId, setSuggestedId] = useState("");
  const [processing, setProcessing] = useState(false);
  const [dragging, setDragging] = useState(false);
  const [draft, setDraft] = useState<ElementDraft | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [statusText, setStatusText] = useState("");
  const [elapsed, setElapsed] = useState(0);

  const sourceDocuments = useMemo(
    () => documents.filter((doc) => !doc.is_ai_artifact && ACCEPTED.test(doc.filename)),
    [documents],
  );
  const sourceDoc = sourceDocuments.find((doc) => doc.id === sourceDocId) ?? null;
  const selectedType = types.find((item) => item.id === templateId) ?? null;
  const groupedTypes = useMemo(() => {
    const groups = new Map<string, ElementDocumentType[]>();
    for (const item of types) {
      groups.set(item.category, [...(groups.get(item.category) ?? []), item]);
    }
    return [...groups.entries()];
  }, [types]);

  useEffect(() => {
    listElementDocumentTypes()
      .then(setTypes)
      .catch((e) => setError(`加载文书目录失败: ${e}`))
      .finally(() => setLoadingTypes(false));
  }, []);

  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    getCurrentWebview()
      .onDragDropEvent((event) => {
        const payload = event.payload;
        if (payload.type === "enter" || payload.type === "over") {
          setDragging(true);
          return;
        }
        if (payload.type === "drop") {
          setDragging(false);
          const path = payload.paths[0];
          if (path) selectSourcePath(path);
          return;
        }
        setDragging(false);
      })
      .then((fn) => {
        unlisten = fn;
      })
      .catch((e) => console.warn("listen element document drag-drop failed", e));
    return () => {
      if (unlisten) unlisten();
    };
    // 只需挂载一次；selectSourcePath 只依赖稳定 setter。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    const filename = sourceDoc?.filename ?? basename(sourcePath);
    if (!filename || types.length === 0) {
      setSuggestedId("");
      return;
    }
    const stem = filename.replace(/\.[^.]+$/, "").replace(/^\d+[、._ -]*/, "");
    const suggestion = types.find(
      (item) => stem.includes(item.name) || item.name.includes(stem.replace(/^传统/, "")),
    );
    setSuggestedId(suggestion?.id ?? "");
    setTemplateId("");
    setDraft(null);
  }, [sourceDocId, sourcePath, sourceDoc?.filename, types]);

  const missing = useMemo(
    () => draft?.fields.filter((field) => field.required && !field.value.trim()).map((f) => f.label) ?? [],
    [draft],
  );
  const bodyMd = useMemo(() => (draft ? buildElementTableBody(draft.fields) : ""), [draft]);

  useEffect(() => {
    if (!processing) {
      setElapsed(0);
      return;
    }
    const startedAt = Date.now();
    const timer = window.setInterval(() => {
      setElapsed(Math.floor((Date.now() - startedAt) / 1000));
    }, 1000);
    return () => window.clearInterval(timer);
  }, [processing]);

  function selectSourcePath(path: string) {
    if (!ACCEPTED.test(path)) {
      setError("仅支持 .docx、.doc 和 .pdf 格式");
      return;
    }
    setSourcePath(path);
    setSourceDocId("");
    setError(null);
  }

  async function chooseFile() {
    const picked = await open({
      multiple: false,
      filters: [{ name: "传统文书", extensions: ["docx", "doc", "pdf"] }],
    });
    if (typeof picked === "string") {
      selectSourcePath(picked);
    }
  }

  async function runOwned() {
    if (!sourcePath || !templateId || processing) return;
    setProcessing(true);
    setStatusText("正在生成本机备用草稿...");
    setError(null);
    try {
      const result = await generateElementDocument(
        sourcePath,
        sourceDoc?.extracted_text_path ?? null,
        templateId,
      );
      setDraft(result);
    } catch (e) {
      setError(String(e));
    } finally {
      setStatusText("");
      setProcessing(false);
    }
  }

  async function runCourt() {
    if (!sourcePath || !templateId || processing) return;
    if (!caseId) {
      setError("法院一张网官方转换需要案件上下文和法院名称。请从案件文书页进入。");
      return;
    }
    const ok = await confirmDialog(
      `将登录人民法院在线服务网并上传“${basename(sourcePath)}”。\n\n接收方：zxfw.court.gov.cn\n文书类型：${selectedType?.name ?? templateId}\n系统只下载法院端生成结果，不提交立案。当前确认仅对本次上传有效。`,
      { title: "确认上传到人民法院在线服务", okLabel: "确认本次上传", danger: true },
    );
    if (!ok) return;
    setProcessing(true);
    setStatusText("正在通过法院网页登录流程转换...");
    setError(null);
    setDraft(null);
    try {
      const result = await courtElementConvert(caseId, sourcePath, templateId);
      const saved = await saveExternalElementDocument(caseId, result.filename, result.data_base64);
      toast(`法院一张网生成结果已保存：${saved.path}`, "success", 8000);
      await openInDefaultApp(saved.path).catch(() => revealInFinder(saved.path).catch(() => {}));
      onSaved?.(saved.doc_id);
    } catch (e) {
      setError(String(e));
    } finally {
      setStatusText("");
      setProcessing(false);
    }
  }

  async function runDirect() {
    if (!sourcePath || !templateId || processing) return;
    const ok = await confirmDialog(
      `将把“${basename(sourcePath)}”上传到智能转写服务并生成要素式 Word。\n\n接收方：gdzqfy.gov.cn、susong51.com\n文书类型：${selectedType?.name ?? templateId}\n不进入立案流程，也不会提交案件。当前确认仅对本次上传有效。`,
      { title: "确认本次智能转写", okLabel: "确认转换", danger: true },
    );
    if (!ok) return;
    setProcessing(true);
    setStatusText("正在调用智能转写服务，最长等待 60 秒...");
    setError(null);
    setDraft(null);
    try {
      const result = await externalElementConvert(sourcePath, templateId, true);
      if (caseId) {
        const saved = await saveExternalElementDocument(caseId, result.filename, result.data_base64);
        toast(`要素式 Word 已保存：${saved.path}`, "success", 8000);
        await openInDefaultApp(saved.path).catch(() => revealInFinder(saved.path).catch(() => {}));
        onSaved?.(saved.doc_id);
      } else {
        const path = await save({
          defaultPath: result.filename,
          filters: [{ name: "Word", extensions: ["docx"] }],
        });
        if (path) {
          const binary = atob(result.data_base64);
          const bytes = Uint8Array.from(binary, (char) => char.charCodeAt(0));
          await writeFile(path, bytes);
          toast(`要素式 Word 已保存：${path}`, "success", 8000);
          await openInDefaultApp(path).catch(() => revealInFinder(path).catch(() => {}));
        }
      }
    } catch (e) {
      setError(`要素式转换失败: ${e}\n\n本机 AI 备用草稿不是法院标准要素式格式，系统不会自动把它当作正式结果保存。`);
    } finally {
      setStatusText("");
      setProcessing(false);
    }
  }

  function updateField(key: string, value: string) {
    setDraft((current) =>
      current
        ? { ...current, fields: current.fields.map((field) => (field.key === key ? { ...field, value } : field)) }
        : current,
    );
  }

  async function saveOwnedResult() {
    if (!draft) return;
    try {
      if (caseId) {
        const saved = await saveElementDocument(caseId, draft.template_id, draft.title, draft.fields);
        toast(`本机备用草稿已保存：${saved.path}`, "success", 8000);
        await openInDefaultApp(saved.path).catch(() => revealInFinder(saved.path).catch(() => {}));
        onSaved?.(saved.doc_id);
      } else {
        const path = await save({
          defaultPath: `${draft.title}.docx`,
          filters: [{ name: "Word", extensions: ["docx"] }],
        });
        if (!path) return;
        await exportElementDocument(draft.template_id, draft.title, draft.fields, path);
        toast(`本机备用草稿已保存：${path}`, "success", 8000);
        await openInDefaultApp(path).catch(() => revealInFinder(path).catch(() => {}));
      }
    } catch (e) {
      setError(`保存失败: ${e}`);
    }
  }

  const sourceReady = Boolean(sourcePath);
  const typeReady = Boolean(templateId);

  return (
    <main className="flex h-full min-h-0 flex-1 flex-col bg-background">
      <header className="flex shrink-0 items-center gap-3 border-b border-border bg-card/50 px-6 py-2.5">
        <button
          type="button"
          onClick={onClose}
          className="inline-flex items-center gap-1 rounded px-2 py-1 text-xs text-muted-foreground hover:bg-accent hover:text-foreground"
        >
          <ArrowLeft className="size-3.5" /> 返回
        </button>
        <span className="text-muted-foreground/40">·</span>
        <h2 className="text-sm font-medium">要素式文书</h2>
      </header>

      <div className="min-h-0 flex-1 overflow-auto">
        <div className="mx-auto max-w-4xl space-y-5 px-6 py-6">
          <section className="text-center">
            <div className="mb-2 inline-flex items-center gap-1.5 rounded-full bg-muted px-3 py-1 text-xs text-muted-foreground">
              <Sparkles className="size-3" /> 62 种文书 · 原件永不覆盖
            </div>
            <h1 className="text-2xl font-semibold">要素式转换</h1>
            <p className="mt-1 text-sm text-muted-foreground">提取要素、人工复核，再生成或保存 Word</p>
          </section>

          <section className="grid gap-4 rounded-xl border border-border bg-card p-5 md:grid-cols-2">
            <div>
              <div className="mb-2 text-xs font-medium text-muted-foreground">1. 选择原文书</div>
              {caseId && (
                <select
                  value={sourceDocId}
                  onChange={(e) => {
                    const doc = sourceDocuments.find((item) => item.id === e.target.value);
                    setSourceDocId(e.target.value);
                    setSourcePath(doc?.source_path ?? "");
                    setError(null);
                  }}
                  className="mb-2 h-10 w-full rounded-md border border-border bg-background px-3 text-sm"
                >
                  <option value="">请选择本案文书，或拖入/点选外部文件</option>
                  {sourceDocuments.map((doc) => <option key={doc.id} value={doc.id}>{doc.filename}</option>)}
                </select>
              )}
              <button
                type="button"
                onClick={() => void chooseFile()}
                className={cn(
                  "flex h-20 w-full items-center gap-3 rounded-lg border border-dashed px-4 text-left transition-colors hover:bg-muted/30",
                  dragging ? "border-foreground bg-muted/60" : "border-border",
                )}
              >
                {sourcePath ? <FileText className="size-5" /> : <Upload className="size-5 text-muted-foreground" />}
                <span className="min-w-0">
                  <span className="block truncate text-sm font-medium">{sourcePath ? basename(sourcePath) : "选择或拖入传统文书"}</span>
                  <span className="block text-xs text-muted-foreground">.docx / .doc / .pdf，最大 20MB</span>
                </span>
              </button>
            </div>

            <div>
              <div className="mb-2 text-xs font-medium text-muted-foreground">2. 确认目标类型</div>
              <select
                value={templateId}
                onChange={(e) => {
                  setTemplateId(e.target.value);
                  setDraft(null);
                }}
                disabled={!sourceReady || loadingTypes}
                className="h-10 w-full rounded-md border border-border bg-background px-3 text-sm disabled:opacity-50"
              >
                <option value="">{loadingTypes ? "正在加载 62 种类型…" : "请选择并确认文书类型"}</option>
                {groupedTypes.map(([category, items]) => (
                  <optgroup key={category} label={category}>
                    {items.map((item) => <option key={item.id} value={item.id}>{item.name}</option>)}
                  </optgroup>
                ))}
              </select>
              {suggestedId && !templateId && (
                <button
                  type="button"
                  onClick={() => setTemplateId(suggestedId)}
                  className="mt-2 text-xs font-medium text-foreground underline underline-offset-2"
                >
                  系统建议：{types.find((item) => item.id === suggestedId)?.name}，点击确认
                </button>
              )}
              {selectedType && (
                <p className="mt-2 text-xs text-muted-foreground">
                  {selectedType.quality_level === "refined" ? "高频精校模板" : "已覆盖，生成后需重点人工复核"}
                  {" · "}版本 {selectedType.template_version}
                </p>
              )}
            </div>
          </section>

          <section className="rounded-xl border border-border bg-card p-5">
            <div className="mb-3 text-xs font-medium text-muted-foreground">3. 一键转换</div>
            <div className="flex gap-2 rounded-lg border border-blue-300 bg-blue-50 p-3 text-xs text-blue-900 dark:bg-blue-950/20 dark:text-blue-200">
              <ShieldAlert className="mt-0.5 size-4 shrink-0" />
              <span>调用智能转写服务生成法院格式要素式 Word；最长等待 60 秒，失败会明确提示，不再自动改用本机 AI 草稿。</span>
            </div>
            <Button
              className="mt-4"
              disabled={!sourceReady || !typeReady || processing}
              onClick={() => void runDirect()}
            >
              {processing ? <Loader2 className="size-4 animate-spin" /> : <Check className="size-4" />}
              {processing ? "正在转换并生成 Word…" : caseId ? "一键转换并自动回库" : "一键转换并保存 Word"}
            </Button>
            {processing && (
              <p className="mt-2 text-xs text-muted-foreground">
                {statusText || "正在处理..."} 已等待 {elapsed} 秒
              </p>
            )}
            <details className="mt-4 rounded-lg border border-border p-3">
              <summary className="cursor-pointer text-xs text-muted-foreground">备用方式</summary>
              <p className="mt-2 text-xs text-muted-foreground">本机 AI 只能生成备用草稿，不是法院标准要素式格式；案件内也可尝试原法院网页登录流程。</p>
              <Button
                className="mt-3"
                variant="outline"
                disabled={!sourceReady || !typeReady || processing}
                onClick={() => void runOwned()}
              >
                <Sparkles className="size-4" />生成本机备用草稿
              </Button>
              {caseId && (
                <Button
                  className="ml-2 mt-3"
                  variant="outline"
                  disabled={!sourceReady || !typeReady || processing}
                  onClick={() => void runCourt()}
                >
                  <AlertTriangle className="size-4" />尝试法院网页登录流程
                </Button>
              )}
            </details>
          </section>

          {error && <div className="rounded-lg border border-destructive/30 bg-destructive/5 p-3 text-sm text-destructive">{error}</div>}

          {draft && (
            <section className="space-y-4 rounded-xl border border-border bg-card p-5">
              <div className="flex flex-wrap items-center justify-between gap-2">
                <div>
                  <h2 className="font-semibold">要素审阅</h2>
                  <p className="text-xs text-muted-foreground">证据摘录只用于核对，不写入最终文书。</p>
                </div>
                {missing.length > 0 && <span className="rounded-full bg-amber-100 px-2.5 py-1 text-xs text-amber-800">缺少 {missing.length} 个必填要素</span>}
              </div>
              {draft.input_truncated && <p className="text-xs text-amber-700">原文较长，本次只分析前 80,000 字，请重点复核后半部分信息。</p>}
              <label className="block text-xs font-medium text-muted-foreground">
                文书标题
                <input
                  value={draft.title}
                  onChange={(e) => setDraft({ ...draft, title: e.target.value })}
                  className="mt-1 h-10 w-full rounded-md border border-border bg-background px-3 text-sm text-foreground"
                />
              </label>
              <div className="space-y-3">
                {draft.fields.map((field) => (
                  <div key={field.key} className="rounded-lg border border-border p-3">
                    <div className="mb-1.5 flex items-center justify-between gap-2">
                      <label className="text-sm font-medium">{field.label}{field.required && <span className="text-destructive"> *</span>}</label>
                      <span className="text-[11px] text-muted-foreground">置信度 {Math.round(field.confidence * 100)}%</span>
                    </div>
                    <textarea
                      value={field.value}
                      onChange={(e) => updateField(field.key, e.target.value)}
                      rows={3}
                      className={cn("w-full rounded-md border bg-background px-3 py-2 text-sm", field.required && !field.value.trim() ? "border-amber-400" : "border-border")}
                      placeholder="待补充"
                    />
                    {field.evidence && <p className="mt-1.5 text-xs text-muted-foreground">原文依据：{field.evidence}</p>}
                  </div>
                ))}
              </div>
              <details className="rounded-lg border border-border p-3">
                <summary className="cursor-pointer text-sm font-medium">预览要素表格</summary>
                <pre className="mt-3 whitespace-pre-wrap font-sans text-sm leading-7 text-foreground">{bodyMd}</pre>
              </details>
              <div className="flex items-center justify-between gap-3 border-t border-border pt-4">
                <p className="text-xs text-muted-foreground">本机 AI 备用草稿，不是法院标准要素式格式；金额、日期、当事人和法律依据必须由律师核对。</p>
                <Button onClick={() => void saveOwnedResult()}><Save className="size-4" />{caseId ? "保存备用草稿" : "另存为备用草稿"}</Button>
              </div>
            </section>
          )}

        </div>
      </div>
    </main>
  );
}
