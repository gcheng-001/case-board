import { useEffect, useMemo, useState } from "react";
import { open, save } from "@tauri-apps/plugin-dialog";
import { writeFile } from "@tauri-apps/plugin-fs";
import {
  AlertTriangle,
  ArrowLeft,
  Check,
  Download,
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
  exportElementDocument,
  externalElementConvert,
  generateElementDocument,
  listElementDocumentTypes,
  saveElementDocument,
  saveExternalElementDocument,
} from "@/lib/api";
import type {
  Document,
  ElementDocumentType,
  ElementDraft,
  ElementFieldValue,
  ExternalElementResult,
} from "@/lib/types";
import { cn } from "@/lib/utils";

type Mode = "owned" | "external";

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

export function buildElementBody(fields: ElementFieldValue[]): string {
  return fields
    .map((field) => `## ${field.label}\n\n${field.value.trim() || "[待补充]"}`)
    .join("\n\n");
}

function decodeBase64(value: string): Uint8Array {
  const binary = atob(value);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i += 1) bytes[i] = binary.charCodeAt(i);
  return bytes;
}

export function ElementConvertWorkbench({ caseId, documents = [], onClose, onSaved }: Props) {
  const [types, setTypes] = useState<ElementDocumentType[]>([]);
  const [loadingTypes, setLoadingTypes] = useState(true);
  const [sourcePath, setSourcePath] = useState("");
  const [sourceDocId, setSourceDocId] = useState("");
  const [templateId, setTemplateId] = useState("");
  const [suggestedId, setSuggestedId] = useState("");
  const [mode, setMode] = useState<Mode>("owned");
  const [processing, setProcessing] = useState(false);
  const [draft, setDraft] = useState<ElementDraft | null>(null);
  const [externalResult, setExternalResult] = useState<ExternalElementResult | null>(null);
  const [error, setError] = useState<string | null>(null);

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
    setExternalResult(null);
  }, [sourceDocId, sourcePath, sourceDoc?.filename, types]);

  const missing = useMemo(
    () => draft?.fields.filter((field) => field.required && !field.value.trim()).map((f) => f.label) ?? [],
    [draft],
  );
  const bodyMd = useMemo(() => (draft ? buildElementBody(draft.fields) : ""), [draft]);

  async function chooseFile() {
    const picked = await open({
      multiple: false,
      filters: [{ name: "传统文书", extensions: ["docx", "doc", "pdf"] }],
    });
    if (typeof picked === "string") {
      setSourcePath(picked);
      setSourceDocId("");
    }
  }

  async function runOwned() {
    if (!sourcePath || !templateId || processing) return;
    setProcessing(true);
    setError(null);
    setExternalResult(null);
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
      setProcessing(false);
    }
  }

  async function runExternal() {
    if (!sourcePath || !templateId || processing) return;
    const ok = await confirmDialog(
      `将把“${basename(sourcePath)}”发送到外部要素式转换服务。\n\n接收方可能包括 gdzqfy.gov.cn 与 susong51.com。文件可能含当事人身份、案情和证据。当前确认仅对本次上传有效。`,
      { title: "确认外传案件材料", okLabel: "确认本次上传", danger: true },
    );
    if (!ok) return;
    setProcessing(true);
    setError(null);
    setDraft(null);
    try {
      setExternalResult(await externalElementConvert(sourcePath, templateId, true));
    } catch (e) {
      setError(String(e));
    } finally {
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
        const docId = await saveElementDocument(caseId, draft.document_type, draft.title, bodyMd);
        toast("已作为新文书保存，原文书未改动", "success");
        onSaved?.(docId);
      } else {
        const path = await save({
          defaultPath: `${draft.title}.docx`,
          filters: [{ name: "Word", extensions: ["docx"] }],
        });
        if (!path) return;
        await exportElementDocument(draft.title, bodyMd, path);
        toast("要素式 Word 已保存", "success");
      }
    } catch (e) {
      setError(`保存失败: ${e}`);
    }
  }

  async function saveExternalResult() {
    if (!externalResult) return;
    try {
      if (caseId) {
        await saveExternalElementDocument(caseId, externalResult.filename, externalResult.data_base64);
        toast("外部转换 Word 已作为新案件文书保存", "success");
        onSaved?.("");
      } else {
        const path = await save({
          defaultPath: externalResult.filename,
          filters: [{ name: "Word", extensions: ["docx"] }],
        });
        if (!path) return;
        await writeFile(path, decodeBase64(externalResult.data_base64));
        toast("外部转换 Word 已按原始字节保存", "success");
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
              {caseId ? (
                <select
                  value={sourceDocId}
                  onChange={(e) => {
                    const doc = sourceDocuments.find((item) => item.id === e.target.value);
                    setSourceDocId(e.target.value);
                    setSourcePath(doc?.source_path ?? "");
                  }}
                  className="h-10 w-full rounded-md border border-border bg-background px-3 text-sm"
                >
                  <option value="">请选择本案文书</option>
                  {sourceDocuments.map((doc) => <option key={doc.id} value={doc.id}>{doc.filename}</option>)}
                </select>
              ) : (
                <button
                  type="button"
                  onClick={() => void chooseFile()}
                  className="flex h-20 w-full items-center gap-3 rounded-lg border border-dashed border-border px-4 text-left hover:bg-muted/30"
                >
                  {sourcePath ? <FileText className="size-5" /> : <Upload className="size-5 text-muted-foreground" />}
                  <span className="min-w-0">
                    <span className="block truncate text-sm font-medium">{sourcePath ? basename(sourcePath) : "选择或拖入传统文书"}</span>
                    <span className="block text-xs text-muted-foreground">.docx / .doc / .pdf，最大 20MB</span>
                  </span>
                </button>
              )}
            </div>

            <div>
              <div className="mb-2 text-xs font-medium text-muted-foreground">2. 确认目标类型</div>
              <select
                value={templateId}
                onChange={(e) => {
                  setTemplateId(e.target.value);
                  setDraft(null);
                  setExternalResult(null);
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
            <div className="mb-3 text-xs font-medium text-muted-foreground">3. 选择处理方式</div>
            <div className="grid gap-3 md:grid-cols-2">
              <button
                type="button"
                onClick={() => setMode("owned")}
                className={cn("rounded-lg border p-4 text-left", mode === "owned" ? "border-foreground bg-muted/40" : "border-border")}
              >
                <div className="flex items-center gap-2 font-medium"><Sparkles className="size-4" />案件看板自有生成</div>
                <p className="mt-1 text-xs text-muted-foreground">文本发送给当前配置的大模型；Word 在本机确定性生成。</p>
              </button>
              <button
                type="button"
                onClick={() => setMode("external")}
                className={cn("rounded-lg border p-4 text-left", mode === "external" ? "border-amber-500 bg-amber-50/50 dark:bg-amber-950/10" : "border-border")}
              >
                <div className="flex items-center gap-2 font-medium"><ShieldAlert className="size-4" />外部服务转换</div>
                <p className="mt-1 text-xs text-muted-foreground">每次上传前确认；公开版未安装授权插件时会安全降级。</p>
              </button>
            </div>
            {mode === "external" && (
              <div className="mt-3 flex gap-2 rounded-lg border border-amber-300 bg-amber-50 p-3 text-xs text-amber-900 dark:bg-amber-950/20 dark:text-amber-200">
                <AlertTriangle className="mt-0.5 size-4 shrink-0" />
                案件材料可能发送至 gdzqfy.gov.cn、susong51.com；确认仅对本次操作有效，系统不会记住授权。
              </div>
            )}
            <Button
              className="mt-4"
              disabled={!sourceReady || !typeReady || processing}
              onClick={() => void (mode === "owned" ? runOwned() : runExternal())}
            >
              {processing ? <Loader2 className="size-4 animate-spin" /> : <Check className="size-4" />}
              {processing ? "正在处理…" : mode === "owned" ? "提取要素" : "确认风险并转换"}
            </Button>
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
                <summary className="cursor-pointer text-sm font-medium">预览生成正文</summary>
                <pre className="mt-3 whitespace-pre-wrap font-sans text-sm leading-7 text-foreground">{bodyMd}</pre>
              </details>
              <div className="flex items-center justify-between gap-3 border-t border-border pt-4">
                <p className="text-xs text-muted-foreground">AI 初稿，金额、日期、当事人和法律依据必须由律师核对。</p>
                <Button onClick={() => void saveOwnedResult()}><Save className="size-4" />{caseId ? "审阅通过并新建文书" : "另存为 Word"}</Button>
              </div>
            </section>
          )}

          {externalResult && (
            <section className="space-y-4 rounded-xl border border-border bg-card p-5">
              <div><h2 className="font-semibold">外部转换结果审阅</h2><p className="text-xs text-muted-foreground">保存时保留服务返回的原始 Word 字节，不重新排版。</p></div>
              <pre className="max-h-96 overflow-auto whitespace-pre-wrap rounded-lg bg-muted p-4 font-sans text-sm leading-7">{externalResult.preview_text || "服务未提供文本预览，请保存后在 Word 中继续核对。"}</pre>
              <div className="flex justify-end"><Button onClick={() => void saveExternalResult()}><Download className="size-4" />{caseId ? "审阅通过并保存新文书" : "另存原始 Word"}</Button></div>
            </section>
          )}
        </div>
      </div>
    </main>
  );
}
