import { useEffect, useRef, useState } from "react";
import {
  BookOpen,
  FolderSearch,
  Loader2,
  Microscope,
  NotebookPen,
  Pencil,
  RefreshCw,
  Search,
  SendToBack,
  ShieldAlert,
  Trash2,
} from "lucide-react";

import { Button } from "@/components/ui/button";
import { MarkdownModal } from "@/components/MarkdownModal";
import { toast } from "@/components/ui/toast";
import {
  addCaseLog,
  deleteCaseLog,
  deleteDocument,
  exportCaseOsInput,
  getCaseWithDocs,
  getSettings,
  listCaseLogs,
  reextractDocument,
  revealInFinder,
  syncCaseToFeishu,
  yuandianBasicQuery,
  yuandianDeepDive,
  yuandianFullReport,
  type YuandianP1Response,
} from "@/lib/api";
import { confirmDialog } from "@/lib/dialog";
import { type Case, type CaseLog, type Document } from "@/lib/types";
import { formatRelativeTime, shortenPath } from "@/lib/format";
import { cn } from "@/lib/utils";

import { groupByStage } from "../lib/groupByStage";
import { CaseChatPanel } from "./chat/CaseChatPanel";
import { CaseSnapshotView } from "./snapshot/CaseSnapshotView";
import { CaseSwitcher } from "./CaseSwitcher";
import {
  type DocumentWritingPaneHandle,
  DocumentWritingPane,
} from "./editor/DocumentWritingPane";
import { ErrorState, LoadingState, NoDocsHint } from "./StatusViews";
import { SourceFilesSection } from "./SourceFilesSection";

/* ------------------------------------------------------------------ */
/* 案件视图                                                            */
/* ------------------------------------------------------------------ */

export function CaseView({
  cases,
  selectedCase,
  documents,
  loading,
  error,
  onSwitchCase,
  onGoHome,
  onOpenDoc,
  onRevealDoc,
  onRevealCase,
  isEditMode,
  onToggleEditMode,
  onDeleteCase,
  onRefreshFiles,
  refreshingFiles,
  onOpenReport,
  reportLoading,
  onReloadCase,
  editingDoc,
  onCloseEditor,
  onArtifactCreated,
}: {
  cases: Case[];
  selectedCase: Case | null;
  documents: Document[];
  loading: boolean;
  error: string | null;
  onSwitchCase: (id: string) => void;
  onGoHome: () => void;
  onOpenDoc: (doc: Document) => void;
  onRevealDoc: (doc: Document) => void;
  onRevealCase: () => void;
  /** 编辑模式开关 — 右上角铅笔按钮控制,P3 接 inline 编辑 / 拖卡片 / 删行 */
  isEditMode: boolean;
  onToggleEditMode: () => void;
  onDeleteCase: () => void;
  onRefreshFiles: () => void;
  refreshingFiles: boolean;
  onOpenReport: () => void;
  reportLoading: boolean;
  /** 2026-05-27 V0.1.13+ chat artifact 完成后的轻量 reload(只重读 DB,不 sync 源文件夹) */
  onReloadCase: () => void;
  /** V0.3 D1+D2 · 写作模式:当前在编辑器里打开的文书(null = 看板模式) */
  editingDoc: Document | null;
  /** V0.3 D1+D2 · 关闭编辑器,回看板模式 */
  onCloseEditor: () => void;
  /** V0.3 D2 · chat 落了 save_artifact 文书后的回调:reload + 自动进编辑器打开(docId 空=仅 reload) */
  onArtifactCreated: (docId: string) => void;
}) {
  const groups = groupByStage(documents);
  const aiArtifacts = documents.filter((d) => d.is_ai_artifact);

  // V0.3 ADR-0003 Phase 1B+2 · chat 改文书的 flush/审阅握手(编辑器磁盘冲突防护)。
  const editorRef = useRef<DocumentWritingPaneHandle>(null);
  // 发送前:编辑器有未保存改动先 flush 到磁盘,让 AI 的 edit_artifact 在最新内容上操作
  //(同时让审阅的「改前基线」= flush 后的内容)。
  const flushEditorBeforeSend = async () => {
    await editorRef.current?.flushIfDirty();
  };
  // AI 这轮调了 edit_artifact 改磁盘后:编辑器若打开 → 进 diff 审阅(接受/拒绝);
  // 编辑器有未保存改动则不进审阅(警告,避免基线错乱);没开编辑器则只刷新列表。
  const handleArtifactEdited = () => {
    if (!editingDoc) {
      onReloadCase();
      return;
    }
    if (editorRef.current?.isDirty()) {
      toast(
        "AI 改了这份文书,但你编辑器里有未保存改动,未进入审阅。先保存或退出,再让 AI 改。",
        "info",
      );
      return;
    }
    void editorRef.current?.enterReview();
  };

  // V0.2.2 · 删除一条 AI 摘要 artifact(软删 + 重读案件)。关键决策点用 confirm 拦一下。
  const handleDeleteDoc = async (doc: Document) => {
    if (
      !(await confirmDialog(
        `删除「${doc.filename}」?会从材料列表移除(软删,不影响磁盘原文件)。`,
        { danger: true, okLabel: "删除" },
      ))
    )
      return;
    try {
      await deleteDocument(doc.id);
      onReloadCase();
    } catch (e) {
      toast(`删除失败:${e}`, "error");
    }
  };

  // V0.3 · 强制重抽单个源文档(抽取失败/想重抽)。立刻 reload 看到「抽取中」,
  // 完成后 App 订阅的 extraction_progress 会再自动刷新。
  const handleReextract = async (doc: Document) => {
    try {
      await reextractDocument(doc.id);
      toast(`已开始重新抽取「${doc.filename}」`, "success");
      onReloadCase();
    } catch (e) {
      toast(`重新抽取失败:${e}`, "error");
    }
  };

  return (
    <main className="flex h-full w-full flex-col bg-background">
      {/* Header */}
      <header className="border-b border-border bg-card/50 px-8 py-5">
        <div className="mx-auto flex max-w-6xl items-start justify-between gap-4">
          <div className="min-w-0 flex-1">
            <button
              type="button"
              onClick={onGoHome}
              className="mb-2 inline-flex items-center gap-1 text-xs text-muted-foreground transition-colors hover:text-foreground"
            >
              ← 返回看板
            </button>
            <div className="flex items-baseline gap-2">
              {cases.length > 1 ? (
                <CaseSwitcher
                  cases={cases}
                  selectedId={selectedCase?.id ?? null}
                  onSwitch={onSwitchCase}
                />
              ) : (
                <h1 className="text-xl font-semibold tracking-tight text-foreground">
                  {selectedCase?.name ?? "—"}
                </h1>
              )}
              <span className="text-xs text-muted-foreground">
                {selectedCase?.case_type}
              </span>
            </div>
            {selectedCase && (
              <button
                type="button"
                onClick={onRevealCase}
                className="mt-1 inline-flex items-center gap-1.5 truncate font-mono text-xs text-muted-foreground transition-colors hover:text-foreground"
                title="在 Finder 中打开案件文件夹"
              >
                <FolderSearch className="size-3 shrink-0" />
                <span className="truncate">
                  {shortenPath(selectedCase.source_folder, 3)}
                </span>
              </button>
            )}
            {!loading && !error && documents.length > 0 && (
              <p className="mt-2 text-xs text-muted-foreground">
                共{" "}
                <span className="font-medium text-foreground">
                  {documents.length}
                </span>{" "}
                份文档
                {aiArtifacts.length > 0 && (
                  <>
                    {" · "}
                    <span className="text-foreground">{aiArtifacts.length}</span>{" "}
                    份 AI 产物
                  </>
                )}
                {selectedCase?.last_scanned_at && (
                  <>
                    {" · 上次扫描 "}
                    <span
                      className="text-foreground"
                      title={selectedCase.last_scanned_at}
                    >
                      {formatRelativeTime(selectedCase.last_scanned_at)}
                    </span>
                  </>
                )}
              </p>
            )}
          </div>
          <div className="flex shrink-0 items-center gap-1.5">
            {/* 「📖 案件分析报告」醒目主按钮 — 没报告也能点(点击触发抽取 + 完成后自动弹) */}
            <Button
              size="sm"
              onClick={onOpenReport}
              disabled={!selectedCase || reportLoading}
              className="bg-foreground text-background hover:bg-foreground/90"
              title={
                selectedCase?.case_report_path
                  ? "查看 LLM 案件分析报告"
                  : "立刻生成案件分析报告(~ 10-30 秒)"
              }
            >
              {reportLoading ? (
                <Loader2 className="size-3.5 animate-spin" />
              ) : (
                <BookOpen className="size-3.5" />
              )}
              {reportLoading ? "生成中…" : "案件报告"}
            </Button>
            <button
              type="button"
              onClick={onRefreshFiles}
              disabled={
                !selectedCase ||
                refreshingFiles ||
                selectedCase?.source_folder === "__DEMO__"
              }
              className="rounded p-1.5 text-muted-foreground transition-colors hover:bg-accent hover:text-foreground disabled:cursor-not-allowed disabled:opacity-30"
              title={
                selectedCase?.source_folder === "__DEMO__"
                  ? "示例案件没有源文件夹,无法更新"
                  : "检测源文件夹有没有新增 / 修改 / 删除的文件,有变动会自动抽取"
              }
              aria-label="更新源文件"
            >
              <RefreshCw
                className={cn("size-4", refreshingFiles && "animate-spin")}
              />
            </button>
            <button
              type="button"
              onClick={onDeleteCase}
              disabled={!selectedCase}
              className="rounded p-1.5 text-muted-foreground transition-colors hover:bg-destructive/10 hover:text-destructive disabled:cursor-not-allowed disabled:opacity-30"
              title="从看板删除当前案件(不动原始文件夹)"
              aria-label="删除当前案件"
            >
              <Trash2 className="size-4" />
            </button>
            <button
              type="button"
              onClick={onToggleEditMode}
              disabled={!selectedCase}
              className={cn(
                "rounded p-1.5 transition-colors disabled:cursor-not-allowed disabled:opacity-30",
                isEditMode
                  ? "bg-foreground text-background hover:bg-foreground/90"
                  : "text-muted-foreground hover:bg-accent hover:text-foreground",
              )}
              title={
                isEditMode
                  ? "退出编辑模式(改动已自动保存)"
                  : "编辑模式 — 改字段 / 删词条 / 拖卡片"
              }
              aria-label={isEditMode ? "退出编辑" : "进入编辑模式"}
              aria-pressed={isEditMode}
            >
              <Pencil className="size-4" />
            </button>
          </div>
        </div>
      </header>

      {/*
        Body + 右侧 AI 助手(2026-05-27 V0.1.13+)。
        V0.3 D1+D2:左侧主区按 mode 二选一(看板 / 写作模式编辑器);
        **CaseChatPanel 永远是本 flex row 的稳定末位子节点**(固定 key,只换它前面的兄弟),
        切换模式不卸载它 —— 否则会丢正在输入的内容/引用 + history 闪烁重拉
        (chatRunRegistry 只保 streaming,不保面板本地状态)。见 docs/V0.3-Milkdown编辑器-实施落地.md §1.3
      */}
      <div className="flex min-h-0 flex-1">
        {editingDoc ? (
          <DocumentWritingPane
            ref={editorRef}
            doc={editingDoc}
            onClose={onCloseEditor}
            onSaved={onReloadCase}
          />
        ) : (
          <div className="flex-1 overflow-auto animate-in fade-in-0 duration-200 ease-out">
            <div className="mx-auto max-w-6xl px-8 py-6">
              {loading && <LoadingState />}
              {error && !loading && <ErrorState message={error} />}
              {!loading && !error && documents.length === 0 && <NoDocsHint />}
              {!loading && !error && documents.length > 0 && selectedCase && (
                <div className="space-y-5">
                  {/* 整套案件信息(框架永远显示,字段空就 "—",作者 2026-05-23 晚十四) */}
                  <CaseSnapshotView
                    caseData={selectedCase}
                    documents={documents}
                    isEditMode={isEditMode}
                  />

                  {/* 原文件(默认折叠) */}
                  <SourceFilesSection
                    total={documents.length}
                    aiArtifacts={aiArtifacts}
                    groups={groups}
                    onOpenDoc={onOpenDoc}
                    onRevealDoc={onRevealDoc}
                    onDeleteDoc={handleDeleteDoc}
                    onReextract={handleReextract}
                    onRefresh={onRefreshFiles}
                    refreshing={refreshingFiles}
                  />

                  <CounterpartyRiskCard caseData={selectedCase} />

                  <CaseWorkLog caseData={selectedCase} />
                </div>
              )}
            </div>
          </div>
        )}

        {/* 案件 AI 助手 — 默认展开 420px,可折叠到 32px sliver。稳定末位,勿被 mode 分支包裹。 */}
        <CaseChatPanel
          key="case-chat"
          caseId={selectedCase?.id ?? null}
          caseName={selectedCase?.name ?? null}
          onArtifactCreated={onArtifactCreated}
          editingDocId={editingDoc?.id ?? null}
          onBeforeSend={flushEditorBeforeSend}
          onArtifactEdited={handleArtifactEdited}
        />
      </div>
    </main>
  );
}

function CaseWorkLog({ caseData }: { caseData: Case }) {
  const [logs, setLogs] = useState<CaseLog[]>([]);
  const [content, setContent] = useState("");
  const [loading, setLoading] = useState(false);
  const [saving, setSaving] = useState(false);
  const [exporting, setExporting] = useState(false);
  const [syncingFeishu, setSyncingFeishu] = useState(false);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    listCaseLogs(caseData.id)
      .then((items) => {
        if (!cancelled) setLogs(items);
      })
      .catch((e) => toast(`读取工作日志失败:${e}`, "error"))
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [caseData.id]);

  const handleAdd = async () => {
    const text = content.trim();
    if (!text || saving) return;
    setSaving(true);
    try {
      const saved = await addCaseLog({
        case_id: caseData.id,
        content: text,
        source: "manual",
      });
      setLogs((prev) => [saved, ...prev]);
      setContent("");
      toast("工作日志已保存", "success");
    } catch (e) {
      toast(`保存工作日志失败:${e}`, "error");
    } finally {
      setSaving(false);
    }
  };

  const handleDelete = async (log: CaseLog) => {
    const ok = await confirmDialog("删除这条工作日志?", {
      danger: true,
      okLabel: "删除",
    });
    if (!ok) return;
    try {
      await deleteCaseLog(log.id);
      setLogs((prev) => prev.filter((x) => x.id !== log.id));
      toast("工作日志已删除", "success");
    } catch (e) {
      toast(`删除工作日志失败:${e}`, "error");
    }
  };

  const handleExportCaseOs = async () => {
    if (exporting) return;
    setExporting(true);
    try {
      const result = await exportCaseOsInput(caseData.id);
      toast("案件 OS 输入源已生成", "success");
      await revealInFinder(result.memo_path);
    } catch (e) {
      toast(`导出案件 OS 输入源失败:${e}`, "error", 7000);
    } finally {
      setExporting(false);
    }
  };

  const handleSyncFeishu = async () => {
    if (syncingFeishu) return;
    setSyncingFeishu(true);
    try {
      const result = await syncCaseToFeishu(caseData.id);
      if (result.synced) {
        toast(result.message, "success");
      } else {
        toast(result.message, "info", 7000);
      }
    } catch (e) {
      toast(`飞书同步失败:${e}`, "error", 7000);
    } finally {
      setSyncingFeishu(false);
    }
  };

  return (
    <section className="rounded-xl border border-border bg-card p-5 shadow-sm">
      <div className="mb-4 flex items-start justify-between gap-3">
        <div>
          <h2 className="flex items-center gap-2 text-sm font-semibold text-foreground">
            <NotebookPen className="size-4" />
            工作日志
          </h2>
          <p className="mt-1 text-caption text-muted-foreground">
            {logs.length} 条记录
          </p>
        </div>
        <div className="flex shrink-0 flex-wrap justify-end gap-2">
          <Button
            variant="outline"
            size="sm"
            onClick={handleSyncFeishu}
            disabled={syncingFeishu || caseData.source_folder === "__DEMO__"}
            title={
              caseData.source_folder === "__DEMO__"
                ? "示例案件不写入飞书"
                : "把当前案件状态和基础信息同步到飞书案件池"
            }
          >
            {syncingFeishu ? (
              <Loader2 className="size-3.5 animate-spin" />
            ) : (
              <RefreshCw className="size-3.5" />
            )}
            同步飞书
          </Button>
          <Button
            variant="outline"
            size="sm"
            onClick={handleExportCaseOs}
            disabled={exporting || caseData.source_folder === "__DEMO__"}
            title={
              caseData.source_folder === "__DEMO__"
                ? "示例案件没有真实案件目录"
                : "在案件目录生成 _caseboard/case_os_input.json"
            }
          >
            {exporting ? (
              <Loader2 className="size-3.5 animate-spin" />
            ) : (
              <SendToBack className="size-3.5" />
            )}
            案件 OS 输入源
          </Button>
        </div>
      </div>

      <div className="flex gap-2">
        <textarea
          value={content}
          onChange={(e) => setContent(e.target.value)}
          rows={3}
          className="min-h-20 flex-1 resize-y rounded-lg border border-input bg-background px-3 py-2 text-sm leading-6 outline-none transition focus:border-ring focus:ring-2 focus:ring-ring/20"
          placeholder="记录沟通、庭期准备、待核实事项..."
        />
        <Button
          onClick={handleAdd}
          disabled={!content.trim() || saving}
          className="self-start bg-foreground text-background hover:bg-foreground/90"
        >
          {saving && <Loader2 className="size-3.5 animate-spin" />}
          保存
        </Button>
      </div>

      <div className="mt-4 divide-y divide-border">
        {loading && (
          <div className="py-4 text-sm text-muted-foreground">读取中...</div>
        )}
        {!loading && logs.length === 0 && (
          <div className="py-4 text-sm text-muted-foreground">暂无工作日志</div>
        )}
        {!loading &&
          logs.map((log) => (
            <div key={log.id} className="group flex gap-3 py-3">
              <div className="min-w-28 shrink-0 text-caption text-muted-foreground">
                <div title={log.occurred_at}>{formatRelativeTime(log.occurred_at)}</div>
                <div className="mt-0.5 font-mono">{log.source || "manual"}</div>
              </div>
              <div className="min-w-0 flex-1 whitespace-pre-wrap text-sm leading-6 text-foreground">
                {log.content}
              </div>
              <button
                type="button"
                onClick={() => handleDelete(log)}
                className="h-7 rounded p-1.5 text-muted-foreground opacity-0 transition hover:bg-destructive/10 hover:text-destructive group-hover:opacity-100"
                title="删除日志"
                aria-label="删除日志"
              >
                <Trash2 className="size-3.5" />
              </button>
            </div>
          ))}
      </div>
    </section>
  );
}

/* ============ 相对方风险画像卡 ============ */

/**
 * 诉讼模块的相对方风险画像(2026-06-10 V0.3.7)。
 *
 * 复用执行模块的元典 P1/P2/Full Report 管线,但面向所有非己方当事人
 * (原告/被告/第三人等),不仅限于被执行人。
 *
 * 数据:agg_party_contacts 中 is_our_side != true 的当事人 → 元典企业查询 → DeepSeek 风险评估。
 */
function CounterpartyRiskCard({ caseData }: { caseData: Case }) {
  const [yuandianResult, setYuandianResult] = useState<YuandianP1Response | null>(null);
  const [riskOpen, setRiskOpen] = useState(false);
  const [deepDiveOpen, setDeepDiveOpen] = useState(false);
  const [fullReportOpen, setFullReportOpen] = useState(false);
  const [loading, setLoading] = useState(false);
  const [loadingDeepDive, setLoadingDeepDive] = useState(false);
  const [loadingFull, setLoadingFull] = useState(false);
  const [current, setCurrent] = useState(caseData);

  // 同步外部 caseData 更新
  useEffect(() => setCurrent(caseData), [caseData]);

  // 解析非己方当事人
  const partyContacts = (() => {
    try {
      const arr = JSON.parse(current.agg_party_contacts ?? "[]");
      return Array.isArray(arr) ? arr : [];
    } catch {
      return [];
    }
  })();
  const counterparties = partyContacts.filter(
    (p: { is_our_side?: boolean; role?: string }) =>
      p.is_our_side === false || (p.is_our_side !== true && !/原告|申请人|上诉人/.test(p.role ?? "")),
  );

  // 防呆:已查过 30 天内再查需确认
  async function confirmRerunIfRecent(lastAt: string | null, label: string): Promise<boolean> {
    if (!lastAt) return true;
    const daysAgo = Math.floor((Date.now() - new Date(lastAt).getTime()) / 86400000);
    if (daysAgo >= 30) return true;
    return confirmDialog(
      `${label}已在 ${daysAgo} 天前查询过。元典数据每月更新一次,建议间隔 30 天再查以节省 API 调用次数。仍要重新查询?`,
      { okLabel: "仍要查询" },
    );
  }

  // 防呆:没填元典 API key
  async function ensureYuandianKey(): Promise<boolean> {
    try {
      const settings = await getSettings();
      if (settings.yuandian_api_key?.trim()) return true;
    } catch (e) {
      alert(`读取设置失败:${e}`);
      return false;
    }
    await confirmDialog(
      "⚠ 未配置元典 API key。「查相对方风险」需要元典法律开放平台的 API key,请在「设置 → 元典法律开放平台」填入并验证。",
      { okLabel: "去设置" },
    );
    return false;
  }

  // P1:查相对方风险
  const handleQuery = async () => {
    if (!(await ensureYuandianKey())) return;
    if (!(await confirmRerunIfRecent(current.risk_assessment_at, "风险报告"))) return;
    setLoading(true);
    try {
      const r = await yuandianBasicQuery(current.id);
      setYuandianResult(r);
      const fresh = await getCaseWithDocs(current.id);
      setCurrent(fresh.case);
      if (fresh.case.risk_assessment_path) {
        setRiskOpen(true);
      } else if (r.assessment.error) {
        alert(`报告生成失败:${r.assessment.error}`);
      }
    } catch (e) {
      alert(`查询失败:${e}`);
    } finally {
      setLoading(false);
    }
  };

  // P2:深挖
  const handleDeepDive = async () => {
    if (!(await ensureYuandianKey())) return;
    if (!(await confirmRerunIfRecent(current.deep_dive_at, "深挖报告"))) return;
    setLoadingDeepDive(true);
    try {
      const r = await yuandianDeepDive(current.id);
      if (r.error) {
        alert(`深挖失败:${r.error}`);
        return;
      }
      const fresh = await getCaseWithDocs(current.id);
      setCurrent(fresh.case);
      if (fresh.case.deep_dive_report_path) {
        setDeepDiveOpen(true);
      }
    } catch (e) {
      alert(`深挖失败:${e}`);
    } finally {
      setLoadingDeepDive(false);
    }
  };

  // 完整报告
  const handleFullReport = async () => {
    if (current.full_report_path) {
      setFullReportOpen(true);
      return;
    }
    if (!(await ensureYuandianKey())) return;
    if (!current.risk_assessment_path) {
      alert("请先点「查相对方风险」生成风险报告");
      return;
    }
    if (!current.deep_dive_report_path) {
      alert("请先点「🔬 深挖」生成深查报告");
      return;
    }
    setLoadingFull(true);
    try {
      const r = await yuandianFullReport(current.id);
      if (r.error) {
        alert(`完整报告生成失败:${r.error}`);
        return;
      }
      const fresh = await getCaseWithDocs(current.id);
      setCurrent(fresh.case);
      if (fresh.case.full_report_path) {
        setFullReportOpen(true);
      }
    } catch (e) {
      alert(`完整报告失败:${e}`);
    } finally {
      setLoadingFull(false);
    }
  };

  const digHints = yuandianResult?.assessment.dig_hints ?? [];
  const canDeepDive = digHints.length > 0 || current.deep_dive_report_path;
  const canFullReport = current.risk_assessment_path && current.deep_dive_report_path;

  return (
    <section className="rounded-xl border border-border bg-card p-5 shadow-sm">
      <div className="mb-4 flex items-start justify-between gap-3">
        <div>
          <h2 className="flex items-center gap-2 text-sm font-semibold text-foreground">
            <ShieldAlert className="size-4" />
            相对方风险画像
          </h2>
          <p className="mt-1 text-caption text-muted-foreground">
            {counterparties.length} 位非己方当事人
          </p>
        </div>
        <div className="flex shrink-0 flex-wrap justify-end gap-2">
          {current.risk_assessment_path && (
            <Button size="sm" variant="outline" onClick={() => setRiskOpen(true)}>
              <BookOpen className="size-3.5" />
              查看风险报告
            </Button>
          )}
          {canDeepDive && current.deep_dive_report_path && (
            <Button size="sm" variant="outline" onClick={() => setDeepDiveOpen(true)}>
              <BookOpen className="size-3.5" />
              查看深挖报告
            </Button>
          )}
          {canFullReport && current.full_report_path && (
            <Button size="sm" variant="outline" onClick={() => setFullReportOpen(true)}>
              <BookOpen className="size-3.5" />
              完整报告
            </Button>
          )}
          <Button
            size="sm"
            onClick={handleQuery}
            disabled={loading || current.source_folder === "__DEMO__"}
            className="bg-foreground text-background hover:bg-foreground/90"
            title="查询非己方当事人的企业风险信息(元典聚合查询 + LLM 风险报告,预计 30-90 秒)"
          >
            {loading ? (
              <Loader2 className="size-3.5 animate-spin" />
            ) : (
              <Search className="size-3.5" />
            )}
            {loading ? "查询中…" : "查相对方风险"}
          </Button>
        </div>
      </div>

      {/* 当事人列表 */}
      {counterparties.length > 0 && (
        <div className="mb-3 flex flex-wrap gap-1.5">
          {counterparties.map((p: { name: string; role?: string }, i: number) => (
            <span
              key={i}
              className="inline-flex items-center gap-1 rounded-full border border-border bg-muted/50 px-2.5 py-0.5 text-caption"
            >
              {p.name}
              {p.role && (
                <span className="text-muted-foreground">· {p.role}</span>
              )}
            </span>
          ))}
        </div>
      )}

      {/* 查询结果摘要 */}
      {yuandianResult && (
        <div className="mt-3 rounded-lg border border-border bg-muted/30 p-3 text-caption text-muted-foreground">
          <div className="flex flex-wrap gap-3">
            <span>查询 {yuandianResult.orchestrator.subjects.length} 个主体</span>
            <span>拉取 {yuandianResult.orchestrator.raw_files.length} 份数据</span>
            <span>耗时 {(yuandianResult.orchestrator.elapsed_ms / 1000).toFixed(1)}s</span>
            {yuandianResult.orchestrator.failures.length > 0 && (
              <span className="text-destructive">
                {yuandianResult.orchestrator.failures.length} 个失败
              </span>
            )}
          </div>
          {canDeepDive && !current.deep_dive_report_path && (
            <Button
              size="sm"
              variant="outline"
              onClick={handleDeepDive}
              disabled={loadingDeepDive}
              className="mt-2"
              title="按深挖建议拉关联公司/案号/第三方主体 → 出深查报告(60-180 秒)"
            >
              {loadingDeepDive ? (
                <Loader2 className="size-3.5 animate-spin" />
              ) : (
                <Microscope className="size-3.5" />
              )}
              {loadingDeepDive ? "深挖中…" : "🔬 深挖"}
            </Button>
          )}
          {canFullReport && !current.full_report_path && (
            <Button
              size="sm"
              variant="outline"
              onClick={handleFullReport}
              disabled={loadingFull}
              className="mt-2 ml-2"
            >
              {loadingFull ? (
                <Loader2 className="size-3.5 animate-spin" />
              ) : (
                <BookOpen className="size-3.5" />
              )}
              {loadingFull ? "生成中…" : "📊 完整报告"}
            </Button>
          )}
        </div>
      )}

      {/* 报告弹窗 */}
      {riskOpen && current.risk_assessment_path && (
        <MarkdownModal
          path={current.risk_assessment_path}
          filename={`${current.name} · 相对方风险画像.md`}
          badge="元典 + LLM"
          onClose={() => setRiskOpen(false)}
          exportMd={{
            mdPath: current.risk_assessment_path,
            title: `${current.name}_风险报告`,
          }}
        />
      )}
      {deepDiveOpen && current.deep_dive_report_path && (
        <MarkdownModal
          path={current.deep_dive_report_path}
          filename={`${current.name} · 深查报告.md`}
          badge="P2 深挖"
          onClose={() => setDeepDiveOpen(false)}
          exportMd={{
            mdPath: current.deep_dive_report_path,
            title: `${current.name}_深挖报告`,
          }}
        />
      )}
      {fullReportOpen && current.full_report_path && (
        <MarkdownModal
          path={current.full_report_path}
          filename={`${current.name} · 完整风险报告.md`}
          badge="风险 + 深挖 合并"
          onClose={() => setFullReportOpen(false)}
          exportMd={{
            mdPath: current.full_report_path,
            title: `${current.name}_完整报告`,
          }}
        />
      )}
    </section>
  );
}

/* 2026-05-23 晚十:删 ExtractingHint — 抽取中详情页主区空白,进度看顶部 banner */
