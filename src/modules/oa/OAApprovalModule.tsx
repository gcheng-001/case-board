import { useCallback, useEffect, useRef, useState } from "react";
import {
  AlertTriangle,
  Bell,
  CheckCircle2,
  Clock,
  FileCheck2,
  Loader2,
  RefreshCw,
  Settings2,
  ShieldAlert,
  XCircle,
} from "lucide-react";

import { Button } from "@/components/ui/button";
import { toast } from "@/components/ui/toast";
import { cn } from "@/lib/utils";
import type {
  OAApprovalListRow,
  OAApprovalOptions,
  OAApprovalPendingResult,
  OAApprovalReview,
  OAConfig,
  OACredential,
  OASession,
} from "./api";
import {
  oaApprovalCheck,
  oaApprovalMonitorSnapshot,
  oaApproveCase,
  oaListConfigs,
  oaListCredentials,
  oaPendingApprovals,
  oaRejectCase,
} from "./api";

type ReminderSettings = {
  enabled: boolean;
  workStart: string;
  workEnd: string;
  eveningEnd: string;
  workIntervalMin: number;
  eveningIntervalMin: number;
  minFee: number;
  lowRatio: number;
  highRatio: number;
  riskBaseFeeMin: number;
};

const DEFAULT_SETTINGS: ReminderSettings = {
  enabled: true,
  workStart: "08:20",
  workEnd: "18:30",
  eveningEnd: "23:00",
  workIntervalMin: 10,
  eveningIntervalMin: 120,
  minFee: 5000,
  lowRatio: 0.005,
  highRatio: 0.3,
  riskBaseFeeMin: 0,
};

const SETTINGS_KEY = "caseboard.oa_approval.settings";
const PENDING_COUNT_KEY = "caseboard.oa_approval.pending_count";

const REJECT_TEMPLATES = [
  "资料不完整",
  "利冲待复核",
  "风险代理材料不足",
  "收费过低需调整",
  "收费过高需说明",
  "收费方式不匹配",
  "其他",
];

export function OAApprovalModule() {
  const [configs, setConfigs] = useState<OAConfig[]>([]);
  const [credentials, setCredentials] = useState<OACredential[]>([]);
  const [configId, setConfigId] = useState("");
  const [credentialId, setCredentialId] = useState("");
  const [pending, setPending] = useState<OAApprovalPendingResult>({ filing: [], closing: [] });
  const [newIds, setNewIds] = useState<Set<string>>(new Set());
  const [selected, setSelected] = useState<OAApprovalListRow | null>(null);
  const [review, setReview] = useState<OAApprovalReview | null>(null);
  const [loading, setLoading] = useState(true);
  const [refreshing, setRefreshing] = useState(false);
  const [checking, setChecking] = useState(false);
  const [actionSession, setActionSession] = useState<OASession | null>(null);
  const [memo, setMemo] = useState("同意");
  const [rejectMemo, setRejectMemo] = useState("");
  const [conflictMemo, setConflictMemo] = useState("");
  const [feeMemo, setFeeMemo] = useState("");
  const [riskConfirmed, setRiskConfirmed] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [settings, setSettings] = useState<ReminderSettings>(() => loadSettings());
  const [lastRefreshAt, setLastRefreshAt] = useState<string | null>(null);
  const [nextRefreshAt, setNextRefreshAt] = useState<Date | null>(null);
  const timerRef = useRef<number | null>(null);

  const selectedId = selected ? rowId(selected) : "";
  const selectedLawcaseId = selected ? Number(rowId(selected)) : 0;

  const approvalOptions = useCallback(
    (extra?: Partial<OAApprovalOptions>): OAApprovalOptions => ({
      min_fee: settings.minFee,
      low_ratio: settings.lowRatio,
      high_ratio: settings.highRatio,
      risk_base_fee_min: settings.riskBaseFeeMin,
      conflict_reviewed: Boolean(conflictMemo.trim()),
      conflict_memo: conflictMemo.trim() || null,
      fee_reviewed: Boolean(feeMemo.trim()),
      fee_memo: feeMemo.trim() || null,
      risk_contract_confirmed: riskConfirmed,
      risk_notice_confirmed: riskConfirmed,
      ...extra,
    }),
    [conflictMemo, feeMemo, riskConfirmed, settings],
  );

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    oaListConfigs()
      .then(async (list) => {
        if (cancelled) return;
        setConfigs(list);
        const preferred = list.find((c) => c.oa_type === "nedev") ?? list[0];
        if (!preferred) return;
        setConfigId(preferred.id);
        const creds = await oaListCredentials(preferred.id);
        if (cancelled) return;
        setCredentials(creds);
        if (creds[0]) setCredentialId(creds[0].id);
      })
      .catch((e) => toast(`加载 OA 配置失败:${e}`, "error"))
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    if (!configId) return;
    oaListCredentials(configId)
      .then((list) => {
        setCredentials(list);
        setCredentialId((prev) => (list.some((c) => c.id === prev) ? prev : list[0]?.id ?? ""));
      })
      .catch((e) => toast(`加载 OA 凭证失败:${e}`, "error"));
  }, [configId]);

  const refresh = useCallback(
    async (monitor = false) => {
      if (!configId || !credentialId || refreshing) return;
      setRefreshing(true);
      try {
        const data = monitor
          ? await oaApprovalMonitorSnapshot(configId, credentialId, approvalOptions())
          : await oaPendingApprovals(configId, credentialId);
        const normalized = normalizePending(data);
        setPending(normalized);
        setLastRefreshAt(new Date().toLocaleString());
        // 广播待审总数给顶栏角标
        const count = (normalized.filing?.length ?? 0) + (normalized.closing?.length ?? 0);
        try { localStorage.setItem(PENDING_COUNT_KEY, String(count)); } catch { /* */ }
        window.dispatchEvent(new CustomEvent("caseboard:oa-pending-count", { detail: { count } }));
        const newRows = data.new_filing ?? [];
        const ids = new Set(newRows.map(rowId).filter(Boolean));
        setNewIds(ids);
        if (ids.size > 0) {
          toast(`摩尚 OA 新增 ${ids.size} 件立案待审批`, "info", 8000);
        }
        if (selectedId && ![...(data.filing ?? []), ...(data.closing ?? [])].some((r) => rowId(r) === selectedId)) {
          setSelected(null);
          setReview(null);
        }
      } catch (e) {
        toast(`刷新 OA 审批失败:${e}`, "error", 8000);
      } finally {
        setRefreshing(false);
      }
    },
    [approvalOptions, configId, credentialId, refreshing, selectedId],
  );

  useEffect(() => {
    if (configId && credentialId) void refresh(false);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [configId, credentialId]);

  useEffect(() => {
    if (timerRef.current) window.clearTimeout(timerRef.current);
    if (!settings.enabled || !configId || !credentialId) {
      setNextRefreshAt(null);
      return;
    }
    const schedule = () => {
      const next = nextRefreshDate(new Date(), settings);
      setNextRefreshAt(next);
      timerRef.current = window.setTimeout(async () => {
        await refresh(true);
        schedule();
      }, Math.max(1000, next.getTime() - Date.now()));
    };
    schedule();
    return () => {
      if (timerRef.current) window.clearTimeout(timerRef.current);
    };
  }, [configId, credentialId, refresh, settings]);

  useEffect(() => {
    localStorage.setItem(SETTINGS_KEY, JSON.stringify(settings));
  }, [settings]);

  // 恢复上次缓存的待审数 → 立刻广播给顶栏角标(不等第一次网络刷新)
  useEffect(() => {
    try {
      const cached = Number(localStorage.getItem(PENDING_COUNT_KEY)) || 0;
      if (cached > 0) {
        window.dispatchEvent(new CustomEvent("caseboard:oa-pending-count", { detail: { count: cached } }));
      }
    } catch { /* */ }
  }, []);

  const runCheck = useCallback(
    async (row: OAApprovalListRow) => {
      if (!configId || !credentialId) return;
      const id = Number(rowId(row));
      if (!id) return;
      setSelected(row);
      setChecking(true);
      setReview(null);
      setConflictMemo("");
      setFeeMemo("");
      setRiskConfirmed(false);
      try {
        const data = await oaApprovalCheck(configId, credentialId, id, approvalOptions());
        setReview(data);
      } catch (e) {
        toast(`审批复核失败:${e}`, "error", 8000);
      } finally {
        setChecking(false);
      }
    },
    [approvalOptions, configId, credentialId],
  );

  const submitApprove = async () => {
    if (!selectedLawcaseId || !configId || !credentialId) return;
    const warnings = review ? approvalWarningLines(review) : [];
    const warningText = warnings.length ? `\n\n系统提示：\n${warnings.map((item) => `- ${item}`).join("\n")}` : "";
    if (!confirm(`确认通过 ${caseNo(selected)}？${warningText}`)) return;
    try {
      const s = await oaApproveCase(
        configId,
        credentialId,
        selectedLawcaseId,
        memo.trim() || "同意",
        approvalOptions({ force_approve: true }),
      );
      setActionSession(s);
      toast("审批通过已提交，正在回读验证", "info");
      window.setTimeout(() => refresh(false), 2000);
    } catch (e) {
      toast(`审批通过失败:${e}`, "error", 8000);
    }
  };

  const submitReject = async () => {
    if (!selectedLawcaseId || !configId || !credentialId) return;
    const text = rejectMemo.trim();
    if (!text) {
      toast("驳回必须填写原因", "error");
      return;
    }
    if (!confirm(`确认驳回 ${caseNo(selected)}？`)) return;
    try {
      const s = await oaRejectCase(configId, credentialId, selectedLawcaseId, text, approvalOptions());
      setActionSession(s);
      toast("驳回已提交，正在回读验证", "info");
      window.setTimeout(() => refresh(false), 2000);
    } catch (e) {
      toast(`驳回失败:${e}`, "error", 8000);
    }
  };

  const recommendation = review?.recommendation;
  const approvalBlockers = review ? hardApprovalBlockers(review) : ["请先完成审批复核"];
  const needsConflictMemo = Boolean((review?.conflict_review as any)?.findings?.length);
  const needsFeeMemo = ["manual_review_required", "correction_required"].includes(
    String((review?.fee_reasonableness_review as any)?.result ?? ""),
  );
  const needsRiskConfirm = (review?.risk_charge_review as any)?.result === "documents_confirmation_required";
  const missingManualInput =
    (needsConflictMemo && !conflictMemo.trim()) ||
    (needsFeeMemo && !feeMemo.trim()) ||
    (needsRiskConfirm && !riskConfirmed);

  const filing = pending.filing ?? [];
  const closing = pending.closing ?? [];

  if (loading) {
    return (
      <div className="flex h-full items-center justify-center bg-background">
        <Loader2 className="size-5 animate-spin text-muted-foreground" />
      </div>
    );
  }

  if (!configs.length) {
    return (
      <div className="flex h-full items-center justify-center bg-background px-6">
        <div className="max-w-md rounded border border-border bg-card p-5 text-sm">
          <div className="font-semibold text-foreground">还没有 OA 配置</div>
          <p className="mt-2 text-muted-foreground">请先到「工具 - OA 系统对接」添加摩尚 OA 和 API Key。</p>
        </div>
      </div>
    );
  }

  return (
    <main className="flex h-full min-h-0 flex-col bg-background">
      <header className="shrink-0 border-b border-border bg-card/50 px-6 py-3">
        <div className="mx-auto flex max-w-7xl items-center gap-3">
          <FileCheck2 className="size-5 text-foreground" />
          <div>
            <h1 className="text-sm font-semibold text-foreground">摩尚 OA 合伙人审批</h1>
            <p className="text-xs text-muted-foreground">
              立案待审 {filing.length} 件 · 结案待审 {closing.length} 件 · {lastRefreshAt ? `上次刷新 ${lastRefreshAt}` : "尚未刷新"}
            </p>
          </div>
          {newIds.size > 0 && (
            <span className="rounded-full bg-red-600 px-2 py-0.5 text-xs font-medium text-white">新增 {newIds.size}</span>
          )}
          <div className="ml-auto flex items-center gap-2">
            <select value={configId} onChange={(e) => setConfigId(e.target.value)}
              className="h-8 rounded border border-border bg-background px-2 text-xs">
              {configs.map((c) => <option key={c.id} value={c.id}>{c.site_name}</option>)}
            </select>
            <select value={credentialId} onChange={(e) => setCredentialId(e.target.value)}
              className="h-8 rounded border border-border bg-background px-2 text-xs">
              {credentials.map((c) => <option key={c.id} value={c.id}>{c.display_name || c.account}</option>)}
            </select>
            <Button variant="outline" size="sm" onClick={() => refresh(false)} disabled={refreshing || !credentialId}>
              <RefreshCw className={cn("size-3.5", refreshing && "animate-spin")} /> 刷新
            </Button>
            <Button variant="ghost" size="sm" onClick={() => setSettingsOpen((v) => !v)}>
              <Settings2 className="size-3.5" /> 提醒设置
            </Button>
          </div>
        </div>
        <div className="mx-auto mt-2 flex max-w-7xl items-center gap-2 text-xs text-muted-foreground">
          <Bell className="size-3.5" />
          <span>{settings.enabled ? "提醒已开启" : "提醒已暂停"}</span>
          <span>·</span>
          <span>下次刷新 {nextRefreshAt ? nextRefreshAt.toLocaleString() : "未安排"}</span>
        </div>
        {settingsOpen && (
          <ReminderSettingsPanel settings={settings} onChange={setSettings} />
        )}
      </header>

      <div className="mx-auto grid min-h-0 w-full max-w-7xl flex-1 grid-cols-[420px_minmax(0,1fr)] gap-4 px-6 py-4">
        <section className="min-h-0 overflow-auto rounded border border-border bg-card">
          <div className="sticky top-0 z-10 border-b border-border bg-card px-3 py-2 text-xs font-semibold text-foreground">
            立案待审
          </div>
          {filing.length === 0 ? (
            <EmptyList label="暂无立案待审" />
          ) : (
            filing.map((row) => (
              <ApprovalRow
                key={rowId(row)}
                row={row}
                active={rowId(row) === selectedId}
                isNew={newIds.has(rowId(row))}
                onClick={() => runCheck(row)}
              />
            ))
          )}
          <div className="border-y border-border bg-muted/30 px-3 py-2 text-xs font-semibold text-muted-foreground">
            结案待审 · 第一版只读
          </div>
          {closing.length === 0 ? (
            <EmptyList label="暂无结案待审" />
          ) : (
            closing.map((row) => (
              <ApprovalRow
                key={rowId(row)}
                row={row}
                active={rowId(row) === selectedId}
                isNew={false}
                readonly
                onClick={() => setSelected(row)}
              />
            ))
          )}
        </section>

        <section className="min-h-0 overflow-auto rounded border border-border bg-card">
          {!selected ? (
            <div className="flex h-full items-center justify-center text-sm text-muted-foreground">
              选择左侧案件查看复核建议
            </div>
          ) : (
            <div className="space-y-4 p-4">
              <div className="flex items-start gap-3 border-b border-border pb-3">
                <div>
                  <h2 className="text-base font-semibold text-foreground">{caseNo(selected)}</h2>
                  <p className="mt-1 text-sm text-muted-foreground">
                    {selected.wtrNames || selected.dsrNames || "委托人未知"} / {selected.tosNames || "对方未知"}
                  </p>
                </div>
                <div className="ml-auto">
                  <RecommendationBadge recommendation={recommendation} loading={checking} readonly={Number(selected.status) === 4} />
                </div>
              </div>

              <CaseFacts row={selected} review={review} />
              <ApprovalKeyDetails review={review} />

              {checking ? (
                <div className="flex items-center gap-2 rounded border border-border bg-background p-4 text-sm text-muted-foreground">
                  <Loader2 className="size-4 animate-spin" /> 正在复核资料、利冲和收费...
                </div>
              ) : review ? (
                <>
                  <ReviewBlock title="资料完整性" icon={CheckCircle2} data={review.completeness_review} />
                  <ReviewBlock title="利益冲突检索" icon={ShieldAlert} data={review.conflict_review} />
                  <ReviewBlock title="OA 重复立案" icon={AlertTriangle} data={review.duplicate_filing_review} />
                  <ReviewBlock title="本地立重检查" icon={ShieldAlert} data={review.local_case_check} />
                  <ReviewBlock title="风险代理合规" icon={AlertTriangle} data={review.risk_charge_review} />
                  <ReviewBlock title="收费合理性" icon={Clock} data={review.fee_reasonableness_review} />

                  {Number(selected.status) !== 4 && (
                    <div className="space-y-3 rounded border border-border bg-background p-4">
                      <h3 className="text-sm font-semibold text-foreground">审批操作</h3>
                      {needsConflictMemo && (
                        <textarea value={conflictMemo} onChange={(e) => setConflictMemo(e.target.value)}
                          placeholder="填写利冲复核结论" className="min-h-16 w-full rounded border border-border bg-card px-3 py-2 text-sm" />
                      )}
                      {needsFeeMemo && (
                        <textarea value={feeMemo} onChange={(e) => setFeeMemo(e.target.value)}
                          placeholder="填写收费复核意见" className="min-h-16 w-full rounded border border-border bg-card px-3 py-2 text-sm" />
                      )}
                      {needsRiskConfirm && (
                        <label className="flex items-center gap-2 text-sm text-foreground">
                          <input type="checkbox" checked={riskConfirmed} onChange={(e) => setRiskConfirmed(e.target.checked)} />
                          已确认风险代理书面合同、醒目告知和风险提示
                        </label>
                      )}
                      <div>
                        <label className="mb-1 block text-xs text-muted-foreground">通过意见</label>
                        <input value={memo} onChange={(e) => setMemo(e.target.value)}
                          className="h-9 w-full rounded border border-border bg-card px-3 text-sm" />
                      </div>
                      <div className="flex gap-2">
                        <Button onClick={submitApprove} disabled={!review || checking || approvalBlockers.length > 0 || missingManualInput}>
                          <CheckCircle2 className="size-4" /> 通过
                        </Button>
                        <Button variant="destructive" onClick={submitReject}>
                          <XCircle className="size-4" /> 驳回
                        </Button>
                      </div>
                      {(approvalBlockers.length > 0 || missingManualInput) && (
                        <div className="rounded border border-amber-200 bg-amber-50 px-3 py-2 text-xs text-amber-800">
                          <div className="font-medium">系统建议你重点复核：</div>
                          <ul className="mt-1 list-disc space-y-0.5 pl-4">
                            {approvalBlockers.map((item, index) => <li key={`block-${index}`}>{item}</li>)}
                            {needsConflictMemo && !conflictMemo.trim() && <li>填写利冲复核结论</li>}
                            {needsFeeMemo && !feeMemo.trim() && <li>填写收费复核意见</li>}
                            {needsRiskConfirm && !riskConfirmed && <li>确认风险代理合同、醒目告知和风险提示</li>}
                          </ul>
                          <p className="mt-2">以上为系统建议，不会替代合伙人最终审批决定；选择通过时会按合伙人确认通过提交。</p>
                        </div>
                      )}
                      <div className="space-y-2">
                        <div className="flex flex-wrap gap-2">
                          {REJECT_TEMPLATES.map((t) => (
                            <button key={t} type="button" onClick={() => setRejectMemo((prev) => prev ? `${prev}；${t}` : t)}
                              className="rounded border border-border px-2 py-1 text-xs text-muted-foreground hover:bg-accent hover:text-foreground">
                              {t}
                            </button>
                          ))}
                        </div>
                        <textarea value={rejectMemo} onChange={(e) => setRejectMemo(e.target.value)}
                          placeholder="驳回原因，必填" className="min-h-20 w-full rounded border border-border bg-card px-3 py-2 text-sm" />
                      </div>
                      {actionSession && (
                        <p className="text-xs text-muted-foreground">最近提交：{actionSession.session_type} · {actionSession.status}</p>
                      )}
                    </div>
                  )}
                </>
              ) : (
                <Button onClick={() => runCheck(selected)}><FileCheck2 className="size-4" /> 开始复核</Button>
              )}
            </div>
          )}
        </section>
      </div>
    </main>
  );
}

function ReminderSettingsPanel({ settings, onChange }: { settings: ReminderSettings; onChange: (s: ReminderSettings) => void }) {
  const update = <K extends keyof ReminderSettings>(key: K, value: ReminderSettings[K]) => onChange({ ...settings, [key]: value });
  return (
    <div className="mx-auto mt-3 grid max-w-7xl grid-cols-5 gap-3 rounded border border-border bg-background p-3 text-xs">
      <label className="flex items-center gap-2">
        <input type="checkbox" checked={settings.enabled} onChange={(e) => update("enabled", e.target.checked)} /> 启用提醒
      </label>
      <Input label="开始" value={settings.workStart} onChange={(v) => update("workStart", v)} />
      <Input label="工作结束" value={settings.workEnd} onChange={(v) => update("workEnd", v)} />
      <Input label="晚间结束" value={settings.eveningEnd} onChange={(v) => update("eveningEnd", v)} />
      <Input label="最低收费" value={String(settings.minFee)} onChange={(v) => update("minFee", Number(v) || 0)} />
      <Input label="工作间隔(分)" value={String(settings.workIntervalMin)} onChange={(v) => update("workIntervalMin", Number(v) || 10)} />
      <Input label="晚间间隔(分)" value={String(settings.eveningIntervalMin)} onChange={(v) => update("eveningIntervalMin", Number(v) || 120)} />
      <Input label="低比例" value={String(settings.lowRatio)} onChange={(v) => update("lowRatio", Number(v) || 0)} />
      <Input label="高比例" value={String(settings.highRatio)} onChange={(v) => update("highRatio", Number(v) || 0)} />
      <Input label="风险基础费" value={String(settings.riskBaseFeeMin)} onChange={(v) => update("riskBaseFeeMin", Number(v) || 0)} />
    </div>
  );
}

function Input({ label, value, onChange }: { label: string; value: string; onChange: (v: string) => void }) {
  return (
    <label className="flex items-center gap-2">
      <span className="shrink-0 text-muted-foreground">{label}</span>
      <input value={value} onChange={(e) => onChange(e.target.value)}
        className="h-7 min-w-0 flex-1 rounded border border-border bg-card px-2" />
    </label>
  );
}

function ApprovalRow({ row, active, isNew, readonly, onClick }: { row: OAApprovalListRow; active: boolean; isNew: boolean; readonly?: boolean; onClick: () => void }) {
  return (
    <button type="button" onClick={onClick}
      className={cn("block w-full border-b border-border px-3 py-3 text-left transition-colors hover:bg-accent/50", active && "bg-accent", isNew && "bg-red-50 dark:bg-red-950/20")}>
      <div className="flex items-center gap-2">
        <span className="truncate text-sm font-medium text-foreground">{caseNo(row)}</span>
        {isNew && <span className="rounded bg-red-600 px-1.5 py-0.5 text-[10px] text-white">新</span>}
        {readonly && <span className="rounded bg-muted px-1.5 py-0.5 text-[10px] text-muted-foreground">只读</span>}
      </div>
      <div className="mt-1 truncate text-xs text-muted-foreground">{row.wtrNames || row.dsrNames || "委托人未知"} / {row.tosNames || "对方未知"}</div>
      <div className="mt-1 truncate text-xs text-muted-foreground">{row.empNames || "经办律师未知"} · {row.causeAction || "案由未知"}</div>
      <div className="mt-1 text-xs text-muted-foreground">收费 {formatMoney(row.chargeAmount)} · 未收 {formatMoney(row.weishou)}</div>
    </button>
  );
}

function EmptyList({ label }: { label: string }) {
  return <div className="px-3 py-6 text-center text-xs text-muted-foreground">{label}</div>;
}

function RecommendationBadge({ recommendation, loading, readonly }: { recommendation?: OAApprovalReview["recommendation"]; loading: boolean; readonly?: boolean }) {
  if (readonly) return <span className="rounded bg-muted px-2 py-1 text-xs text-muted-foreground">结案待审只读</span>;
  if (loading) return <span className="rounded bg-muted px-2 py-1 text-xs text-muted-foreground">复核中</span>;
  const label = recommendation?.label ?? "未复核";
  const result = recommendation?.result ?? "";
  return (
    <span className={cn("rounded px-2 py-1 text-xs font-medium",
      result === "recommend_approve" && "bg-green-100 text-green-700 dark:bg-green-950/30 dark:text-green-300",
      result === "manual_review_required" && "bg-yellow-100 text-yellow-700 dark:bg-yellow-950/30 dark:text-yellow-300",
      result === "recommend_reject_for_correction" && "bg-orange-100 text-orange-700 dark:bg-orange-950/30 dark:text-orange-300",
      result === "block_approval" && "bg-red-100 text-red-700 dark:bg-red-950/30 dark:text-red-300",
      !result && "bg-muted text-muted-foreground",
    )}>{label}</span>
  );
}

function CaseFacts({ row, review }: { row: OAApprovalListRow; review: OAApprovalReview | null }) {
  const summary = review?.summary ?? {};
  const caseType = firstPresent(
    summary.case_type_display,
    summary.base_type,
    summary.case_category,
    row.baseTypeName,
    row.baseType,
    row.caseCategoryName,
    row.caseCategory,
  );
  return (
    <div className="grid grid-cols-4 gap-2 text-xs">
      <Fact label="经办律师" value={String(row.empNames || summary.emp_names || "未知")} />
      <Fact label="案件类型" value={String(caseType || "未知")} />
      <Fact label="案由" value={String(row.causeAction || summary.cause || "未知")} />
      <Fact label="收费方式" value={String(row.chargeMethodName || summary.charge_method || "未知")} />
      <Fact label="收费类型" value={String(summary.risk_charge_label || "未知")} tone={summary.is_risk_charge ? "warning" : "default"} />
      <Fact label="委托收费" value={formatMoney(row.chargeAmount ?? summary.charge_amount as any)} />
      <Fact label="标的额" value={formatMoney(summary.subject_amount as any)} />
      <Fact label="已收" value={formatMoney(row.yishou ?? summary.received as any)} />
      <Fact label="未收" value={formatMoney(row.weishou ?? summary.unreceived as any)} />
      <Fact label="受理日期" value={String(row.shouliDate || "未知")} />
      <Fact label="状态" value={String(row.statusName || "未知")} />
    </div>
  );
}

function Fact({ label, value, tone = "default" }: { label: string; value: string; tone?: "default" | "warning" }) {
  return (
    <div className={cn(
      "rounded border p-2",
      tone === "warning" ? "border-amber-200 bg-amber-50 text-amber-900" : "border-border bg-background",
    )}>
      <div className="text-muted-foreground">{label}</div>
      <div className="mt-1 truncate text-foreground">{value}</div>
    </div>
  );
}

function ApprovalKeyDetails({ review }: { review: OAApprovalReview | null }) {
  const summary = review?.summary ?? {};
  const caseType = firstPresent(summary.case_type_display, summary.base_type, summary.case_category);
  const rows = [
    ["案件类型", caseType],
    ["风险收费条款", firstPresent(summary.risk_charge_terms, summary.is_risk_charge ? summary.charge_memo : null)],
    ["情况说明", summary.case_memo],
    ["案情摘要", summary.case_summary],
    ["收费说明", summary.charge_memo],
    ["代理事项", summary.proxy_permission],
  ] as const;
  const visible = rows.filter(([, value]) => presentText(value));
  const isRiskCharge = Boolean(summary.is_risk_charge);
  if (!review && !isRiskCharge) return null;
  return (
    <div className="rounded border border-border bg-background p-4">
      <div className="flex items-center gap-2">
        <AlertTriangle className={cn("size-4", isRiskCharge ? "text-amber-600" : "text-muted-foreground")} />
        <h3 className="text-sm font-semibold text-foreground">审批重点内容</h3>
        {isRiskCharge && (
          <span className="rounded bg-amber-100 px-2 py-0.5 text-xs font-medium text-amber-800">
            风险收费
          </span>
        )}
      </div>
      {visible.length === 0 ? (
        <p className="mt-3 text-sm text-muted-foreground">OA 未返回案件类型、风险收费条款、情况说明、案情摘要、收费说明或代理事项。</p>
      ) : (
        <div className="mt-3 space-y-3">
          {visible.map(([label, value]) => (
            <div key={label}>
              <div className="text-xs text-muted-foreground">{label}</div>
              <div className="mt-1 whitespace-pre-wrap rounded border border-border bg-card px-3 py-2 text-sm text-foreground">
                {String(value)}
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

function ReviewBlock({ title, icon: Icon, data }: { title: string; icon: typeof CheckCircle2; data?: Record<string, unknown> }) {
  const result = String(data?.result ?? "未复核");
  const text = flattenReview(data);
  return (
    <div className="rounded border border-border bg-background p-4">
      <div className="flex items-center gap-2">
        <Icon className="size-4 text-muted-foreground" />
        <h3 className="text-sm font-semibold text-foreground">{title}</h3>
        <span className="ml-auto rounded bg-muted px-2 py-0.5 text-xs text-muted-foreground">{result}</span>
      </div>
      <div className="mt-3 space-y-1 text-sm text-muted-foreground">
        {text.length ? text.map((line, i) => <p key={i}>{line}</p>) : <p>暂无异常</p>}
      </div>
    </div>
  );
}

function flattenReview(data?: Record<string, unknown>): string[] {
  if (!data) return [];
  const out: string[] = [];
  for (const key of ["missing", "option_errors", "blockers", "warnings", "issues", "findings", "limitations", "required_documents"]) {
    const value = data[key] as any;
    if (Array.isArray(value)) {
      value.slice(0, 6).forEach((item) => {
        if (typeof item === "string") out.push(item);
        else if (item?.message) out.push(item.message);
        else if (item?.relation) out.push(`${item.relation}：${item.matched_name || ""} ${item.case_no || ""}`);
        else if (item?.matched_principals || item?.matched_opponents) {
          out.push(`命中案件：${item.case_no || item.case_id || "未知案号"} · 委托人 ${item.wtr_names || "未知"} · 对方 ${item.tos_names || "未知"} · 案由 ${item.cause || "未知"}`);
        }
        else if (item?.local_case_id) {
          out.push(`本地案件：${item.case_name || item.case_no || item.local_case_id} · ${item.role || "当事人"} ${item.matched_name || ""} · ${item.cause || "未知案由"}`);
        }
        else if (item?.case_no || item?.case_name) out.push(`${item.case_no || item.case_name} · ${item.cause || ""}`);
      });
    }
  }
  return out;
}

function hardApprovalBlockers(review: OAApprovalReview): string[] {
  const out: string[] = [];
  const completeness = review.completeness_review as any;
  const conflict = review.conflict_review as any;
  const duplicate = review.duplicate_filing_review as any;
  const risk = review.risk_charge_review as any;
  const fee = review.fee_reasonableness_review as any;

  for (const value of completeness?.missing ?? []) {
    out.push(`资料不完整：${value}`);
  }
  for (const value of completeness?.option_errors ?? []) {
    out.push(String(value));
  }
  for (const value of conflict?.blockers ?? []) {
    out.push(String(value));
  }
  for (const value of duplicate?.blockers ?? []) {
    out.push(String(value));
  }
  for (const value of risk?.blockers ?? []) {
    out.push(String(value));
  }
  for (const value of fee?.blockers ?? []) {
    out.push(String(value));
  }
  return out;
}

function approvalWarningLines(review: OAApprovalReview): string[] {
  const out = hardApprovalBlockers(review);
  for (const reason of review.recommendation?.reasons ?? []) {
    if (reason && !out.includes(reason)) out.push(reason);
  }
  return out.slice(0, 10);
}

function rowId(row: OAApprovalListRow | null): string {
  if (!row) return "";
  const id = row.id ?? row.lawcaseId;
  return id == null ? "" : String(id);
}

function caseNo(row: OAApprovalListRow | null): string {
  return row?.no || row?.preNo || (row ? `OA#${rowId(row)}` : "未选择案件");
}

function formatMoney(value: unknown): string {
  const n = Number(value);
  if (!Number.isFinite(n)) return "未知";
  return n.toLocaleString("zh-CN", { maximumFractionDigits: 2 });
}

function presentText(value: unknown): boolean {
  return typeof value === "string" ? value.trim().length > 0 : value != null && value !== "";
}

function firstPresent(...values: unknown[]): unknown {
  return values.find(presentText) ?? "";
}

function normalizePending(data: OAApprovalPendingResult): OAApprovalPendingResult {
  return { filing: data.filing ?? [], closing: data.closing ?? [], new_filing: data.new_filing ?? [], counts: data.counts, fetched_at: data.fetched_at };
}

function loadSettings(): ReminderSettings {
  try {
    const raw = localStorage.getItem(SETTINGS_KEY);
    if (!raw) return DEFAULT_SETTINGS;
    const merged = { ...DEFAULT_SETTINGS, ...JSON.parse(raw) };
    if (!Number.isFinite(merged.minFee) || merged.minFee < DEFAULT_SETTINGS.minFee) {
      merged.minFee = DEFAULT_SETTINGS.minFee;
    }
    return merged;
  } catch {
    return DEFAULT_SETTINGS;
  }
}

function minutesOf(value: string): number {
  const [h, m] = value.split(":").map((x) => Number(x));
  return (Number.isFinite(h) ? h : 0) * 60 + (Number.isFinite(m) ? m : 0);
}

function nextRefreshDate(now: Date, settings: ReminderSettings): Date {
  const current = now.getHours() * 60 + now.getMinutes();
  const workStart = minutesOf(settings.workStart);
  const workEnd = minutesOf(settings.workEnd);
  const eveningEnd = minutesOf(settings.eveningEnd);
  const next = new Date(now);
  if (current < workStart) {
    next.setHours(Math.floor(workStart / 60), workStart % 60, 0, 0);
    return next;
  }
  if (current >= eveningEnd) {
    next.setDate(next.getDate() + 1);
    next.setHours(Math.floor(workStart / 60), workStart % 60, 0, 0);
    return next;
  }
  const interval = current < workEnd ? settings.workIntervalMin : settings.eveningIntervalMin;
  next.setTime(now.getTime() + Math.max(1, interval) * 60_000);
  if (next.getHours() * 60 + next.getMinutes() >= eveningEnd) {
    next.setDate(next.getDate() + 1);
    next.setHours(Math.floor(workStart / 60), workStart % 60, 0, 0);
  }
  return next;
}
