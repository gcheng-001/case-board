/**
 * OA 立案卡片 — 嵌入案件详情页。
 *
 * 显示在案件详情页底部,让用户一键把案件数据推送到 OA 立案表单。
 * 参考法穿的 CourtFilingSection 设计。
 */

import { useEffect, useState, useCallback, useRef } from "react";
import { Building2, Loader2, CheckCircle2, Save, XCircle } from "lucide-react";

import type { OAConfig, OACredential, OASession } from "./api";
import {
  oaListConfigs,
  oaListCredentials,
  oaExecuteFiling,
  oaGetSession,
  onOASessionProgress,
} from "./api";
import type { Case } from "@/lib/types";
import { parseJsonArray } from "@/lib/types";

interface Props {
  caseData: Case;
}

interface OAFilingDraft {
  selectedConfigId?: string;
  selectedCredId?: string;
  shouliDate?: string;
  claimAmount?: string;
  chargeMethod?: string;
  chargeAmount?: string;
  handlingLawyers?: string;
  proxyStage?: string;
  proxySide?: string;
  proxyPermission?: string;
  savedAt?: string;
}

const OA_FILING_DRAFT_PREFIX = "caseboard.oa_filing_draft.";

function todayISO() {
  const now = new Date();
  const local = new Date(now.getTime() - now.getTimezoneOffset() * 60_000);
  return local.toISOString().slice(0, 10);
}

function draftKey(caseId: string) {
  return `${OA_FILING_DRAFT_PREFIX}${caseId}`;
}

function loadDraft(caseId: string): OAFilingDraft | null {
  try {
    const raw = localStorage.getItem(draftKey(caseId));
    if (!raw) return null;
    const parsed = JSON.parse(raw) as OAFilingDraft;
    return parsed && typeof parsed === "object" ? parsed : null;
  } catch {
    return null;
  }
}

function saveDraft(caseId: string, draft: OAFilingDraft) {
  try {
    localStorage.setItem(
      draftKey(caseId),
      JSON.stringify({ ...draft, savedAt: new Date().toISOString() }),
    );
  } catch {
    // 本地存储不可用不应阻断 OA 推送。
  }
}

function defaultShouliDate(caseData: Case) {
  return caseData.agg_filed_at?.slice(0, 10) ?? todayISO();
}

function defaultClaimAmount(caseData: Case) {
  return caseData.agg_claim_amount != null ? String(caseData.agg_claim_amount) : "";
}

export function OAFilingSection({ caseData }: Props) {
  const [initialDraft] = useState(() => loadDraft(caseData.id));
  const [configs, setConfigs] = useState<OAConfig[]>([]);
  const [selectedConfigId, setSelectedConfigId] = useState<string>(initialDraft?.selectedConfigId ?? "");
  const [credentials, setCredentials] = useState<OACredential[]>([]);
  const [selectedCredId, setSelectedCredId] = useState<string>(initialDraft?.selectedCredId ?? "");
  const [loading, setLoading] = useState(true);
  const [executing, setExecuting] = useState(false);
  const [session, setSession] = useState<OASession | null>(null);
  const [proxyPermission, setProxyPermission] = useState(initialDraft?.proxyPermission ?? "");
  const [chargeAmount, setChargeAmount] = useState(initialDraft?.chargeAmount ?? "");
  const [chargeMethod, setChargeMethod] = useState(initialDraft?.chargeMethod ?? "计件收费");
  const [handlingLawyers, setHandlingLawyers] = useState(initialDraft?.handlingLawyers ?? "");
  const [proxyStage, setProxyStage] = useState(initialDraft?.proxyStage ?? "一审");
  const [proxySide, setProxySide] = useState(initialDraft?.proxySide ?? "");
  const [claimAmount, setClaimAmount] = useState(initialDraft?.claimAmount ?? defaultClaimAmount(caseData));
  const [shouliDate, setShouliDate] = useState(initialDraft?.shouliDate ?? defaultShouliDate(caseData));
  const [validationError, setValidationError] = useState("");
  const [saveMessage, setSaveMessage] = useState("");
  const pollRef = useRef<ReturnType<typeof setInterval> | undefined>(undefined);

  const courtName = caseData.agg_court || caseData.court || "";
  const causeName = caseData.agg_cause || caseData.cause || "";
  const plaintiffs = parseJsonArray(caseData.agg_plaintiffs);
  const defendants = parseJsonArray(caseData.agg_defendants);
  const thirdParties = parseJsonArray(caseData.agg_third_parties);
  const canSubmit = Boolean(
    selectedCredId &&
    shouliDate.trim() &&
    causeName.trim() &&
    courtName.trim() &&
    claimAmount.trim() &&
    chargeMethod.trim() &&
    chargeAmount.trim() &&
    handlingLawyers.trim() &&
    proxyStage.trim() &&
    proxySide.trim() &&
    proxyPermission.trim(),
  );

  useEffect(() => {
    const draft = loadDraft(caseData.id);
    setSelectedConfigId(draft?.selectedConfigId ?? configs[0]?.id ?? "");
    setSelectedCredId(draft?.selectedCredId ?? "");
    setProxyPermission(draft?.proxyPermission ?? "");
    setChargeAmount(draft?.chargeAmount ?? "");
    setChargeMethod(draft?.chargeMethod ?? "计件收费");
    setHandlingLawyers(draft?.handlingLawyers ?? "");
    setProxyStage(draft?.proxyStage ?? "一审");
    setProxySide(draft?.proxySide ?? "");
    setClaimAmount(draft?.claimAmount ?? defaultClaimAmount(caseData));
    setShouliDate(draft?.shouliDate ?? defaultShouliDate(caseData));
    setValidationError("");
    setSaveMessage("");
    setSession(null);
  }, [caseData.id]);

  useEffect(() => {
    oaListConfigs()
      .then((list) => {
        const enabled = list.filter((c) => c.is_enabled);
        setConfigs(enabled);
        setSelectedConfigId((current) => {
          if (current && enabled.some((cfg) => cfg.id === current)) return current;
          return enabled[0]?.id ?? "";
        });
      })
      .finally(() => setLoading(false));
  }, []);

  // 加载凭证
  useEffect(() => {
    if (!selectedConfigId) return;
    oaListCredentials(selectedConfigId).then((list) => {
      setCredentials(list);
      const draft = loadDraft(caseData.id);
      setSelectedCredId((current) => {
        if (current && list.some((cred) => cred.id === current)) return current;
        if (
          draft?.selectedConfigId === selectedConfigId &&
          draft.selectedCredId &&
          list.some((cred) => cred.id === draft.selectedCredId)
        ) {
          return draft.selectedCredId;
        }
        return list[0]?.id ?? "";
      });
    });
  }, [caseData.id, selectedConfigId]);

  // 监听进度
  useEffect(() => {
    const unlisten = onOASessionProgress((p) => {
      setSession((prev) =>
        prev && prev.id === p.session_id
          ? { ...prev, progress_pct: p.pct, progress_msg: p.msg }
          : prev,
      );
    });
    return () => { unlisten.then((fn) => fn()); };
  }, []);

  // 清理轮询
  useEffect(() => {
    return () => { if (pollRef.current) clearInterval(pollRef.current); };
  }, []);

  const pollSession = useCallback((sessionId: string) => {
    if (pollRef.current) clearInterval(pollRef.current);
    pollRef.current = setInterval(async () => {
      try {
        const s = await oaGetSession(sessionId);
        if (!s) return;
        setSession(s);
        if (s.status === "completed" || s.status === "failed") {
          if (pollRef.current) clearInterval(pollRef.current);
          setExecuting(false);
        }
      } catch {
        if (pollRef.current) clearInterval(pollRef.current);
        setExecuting(false);
      }
    }, 3000);
  }, []);

  const buildDraft = useCallback(
    (): OAFilingDraft => ({
      selectedConfigId,
      selectedCredId,
      shouliDate: shouliDate.trim(),
      claimAmount: claimAmount.trim(),
      chargeMethod: chargeMethod.trim(),
      chargeAmount: chargeAmount.trim(),
      handlingLawyers: handlingLawyers.trim(),
      proxyStage: proxyStage.trim(),
      proxySide: proxySide.trim(),
      proxyPermission: proxyPermission.trim(),
    }),
    [
      selectedConfigId,
      selectedCredId,
      shouliDate,
      claimAmount,
      chargeMethod,
      chargeAmount,
      handlingLawyers,
      proxyStage,
      proxySide,
      proxyPermission,
    ],
  );

  const handleSaveDraft = useCallback(() => {
    saveDraft(caseData.id, buildDraft());
    setValidationError("");
    setSaveMessage("已保存，下次进入本案会自动带出这些填写内容。");
  }, [buildDraft, caseData.id]);

  const handleExecute = async () => {
    if (!selectedConfigId || !selectedCredId) return;
    if (!canSubmit) {
      setValidationError("请先补齐受理日期、案由、法院、标的额、收费方式、委托费用、经办律师、代理阶段、代理方和代理权限。");
      return;
    }
    setValidationError("");
    saveDraft(caseData.id, buildDraft());
    setSaveMessage("已保存，并开始推送到 OA 立案。");
    setExecuting(true);
    setSession(null);
    try {
      const s = await oaExecuteFiling(selectedConfigId, caseData.id, selectedCredId, {
        shouli_date: shouliDate.trim(),
        claim_amount: Number(claimAmount),
        charge_method: chargeMethod.trim(),
        charge_amount: Number(chargeAmount),
        handling_lawyers: handlingLawyers.trim(),
        proxy_stage: proxyStage.trim(),
        proxy_side: proxySide.trim(),
        proxy_permission: proxyPermission.trim(),
        plaintiffs,
        defendants,
        third_parties: thirdParties,
        baseTypeName: "民事案件",
        case_category: "合同、准合同纠纷",
        shouli_type: "3",
      });
      setSession(s);
      if (s.status === "pending" || s.status === "running") {
        pollSession(s.id);
      } else {
        setExecuting(false);
      }
    } catch (e) {
      setExecuting(false);
      setSession({
        id: "",
        oa_config_id: "",
        session_type: "filing",
        status: "failed",
        case_id: null,
        progress_pct: 0,
        progress_msg: "",
        result_json: null,
        error_message: String(e),
        started_at: null,
        completed_at: null,
        created_at: "",
        updated_at: "",
      });
    }
  };

  if (loading) {
    return (
      <div className="rounded border border-border p-4">
        <div className="flex items-center gap-2 text-muted-foreground">
          <Loader2 className="size-4 animate-spin" />
          <span className="text-xs">加载 OA 配置...</span>
        </div>
      </div>
    );
  }

  if (configs.length === 0) {
    return (
      <div className="rounded border border-dashed border-border p-4">
        <div className="flex items-center gap-2 text-muted-foreground">
          <Building2 className="size-4" />
          <span className="text-xs">未配置 OA 系统 · 请先在工具模块中添加</span>
        </div>
      </div>
    );
  }

  return (
    <div className="rounded border border-border p-4 space-y-3">
      <div className="flex items-center gap-2">
        <Building2 className="size-4 text-muted-foreground" />
        <h3 className="text-xs font-semibold text-foreground">OA 立案</h3>
      </div>

      {/* 案件信息摘要 */}
      <div className="rounded bg-muted/30 px-3 py-2 text-[13px] grid grid-cols-2 gap-1">
        <div><span className="text-muted-foreground">案件：</span>{caseData.name}</div>
        {caseData.case_no && <div><span className="text-muted-foreground">案号：</span>{caseData.case_no}</div>}
        {causeName && <div><span className="text-muted-foreground">案由：</span>{causeName}</div>}
        {courtName && <div><span className="text-muted-foreground">法院：</span>{courtName}</div>}
      </div>

      <div className="grid grid-cols-2 gap-3">
        <div>
          <label className="mb-1 block text-[11px] text-muted-foreground">受理日期</label>
          <input
            type="date"
            value={shouliDate}
            onChange={(e) => setShouliDate(e.target.value)}
            className="w-full rounded border border-border bg-background px-2 py-1.5 text-xs"
            disabled={executing}
          />
        </div>
        <div>
          <label className="mb-1 block text-[11px] text-muted-foreground">标的额</label>
          <input
            type="number"
            min="0"
            value={claimAmount}
            onChange={(e) => setClaimAmount(e.target.value)}
            className="w-full rounded border border-border bg-background px-2 py-1.5 text-xs"
            disabled={executing}
          />
        </div>
        <div>
          <label className="mb-1 block text-[11px] text-muted-foreground">收费方式</label>
          <select
            value={chargeMethod}
            onChange={(e) => setChargeMethod(e.target.value)}
            className="w-full rounded border border-border bg-background px-2 py-1.5 text-xs"
            disabled={executing}
          >
            <option value="计件收费">计件收费</option>
            <option value="风险代理收费">风险代理收费</option>
            <option value="免费">免费</option>
            <option value="另案已收">另案已收</option>
          </select>
        </div>
        <div>
          <label className="mb-1 block text-[11px] text-muted-foreground">委托费用</label>
          <input
            type="number"
            min="0"
            value={chargeAmount}
            onChange={(e) => setChargeAmount(e.target.value)}
            className="w-full rounded border border-border bg-background px-2 py-1.5 text-xs"
            disabled={executing}
          />
        </div>
        <div>
          <label className="mb-1 block text-[11px] text-muted-foreground">经办律师</label>
          <input
            value={handlingLawyers}
            onChange={(e) => setHandlingLawyers(e.target.value)}
            placeholder="例如：高澄、王婷婷"
            className="w-full rounded border border-border bg-background px-2 py-1.5 text-xs"
            disabled={executing}
          />
        </div>
        <div>
          <label className="mb-1 block text-[11px] text-muted-foreground">代理阶段</label>
          <input
            value={proxyStage}
            onChange={(e) => setProxyStage(e.target.value)}
            className="w-full rounded border border-border bg-background px-2 py-1.5 text-xs"
            disabled={executing}
          />
        </div>
        <div>
          <label className="mb-1 block text-[11px] text-muted-foreground">代理方</label>
          <select
            value={proxySide}
            onChange={(e) => setProxySide(e.target.value)}
            className="w-full rounded border border-border bg-background px-2 py-1.5 text-xs"
            disabled={executing}
          >
            <option value="">请选择</option>
            <option value="原告">代理原告</option>
            <option value="被告">代理被告</option>
            <option value="第三人">代理第三人</option>
          </select>
        </div>
        <div>
          <label className="mb-1 block text-[11px] text-muted-foreground">代理权限</label>
          <select
            value={proxyPermission}
            onChange={(e) => setProxyPermission(e.target.value)}
            className="w-full rounded border border-border bg-background px-2 py-1.5 text-xs"
            disabled={executing}
          >
            <option value="">请选择</option>
            <option value="一般代理">一般代理</option>
            <option value="特别授权">特别授权</option>
          </select>
        </div>
      </div>

      {validationError && (
        <div className="rounded bg-red-50 px-3 py-2 text-xs text-red-700">
          {validationError}
        </div>
      )}
      {saveMessage && (
        <div className="rounded bg-green-50 px-3 py-2 text-xs text-green-700">
          {saveMessage}
        </div>
      )}

      {/* 选择 OA 和凭证 */}
      <div className="flex gap-3">
        <div className="flex-1">
          <label className="mb-1 block text-[11px] text-muted-foreground">OA 系统</label>
          <select value={selectedConfigId} onChange={(e) => setSelectedConfigId(e.target.value)}
            className="w-full rounded border border-border bg-background px-2 py-1.5 text-xs" disabled={executing}>
            {configs.map((c) => <option key={c.id} value={c.id}>{c.site_name}</option>)}
          </select>
        </div>
        <div className="flex-1">
          <label className="mb-1 block text-[11px] text-muted-foreground">登录账号</label>
          <select value={selectedCredId} onChange={(e) => setSelectedCredId(e.target.value)}
            className="w-full rounded border border-border bg-background px-2 py-1.5 text-xs" disabled={executing || credentials.length === 0}>
            {credentials.length === 0 ? (
              <option value="">请先添加账号</option>
            ) : (
              credentials.map((c) => (
                <option key={c.id} value={c.id}>
                  {c.display_name || c.account}
                </option>
              ))
            )}
          </select>
        </div>
      </div>

      {/* 操作按钮 */}
      <div className="flex items-center gap-3">
        <button onClick={handleSaveDraft}
          disabled={executing}
          className="inline-flex items-center gap-1 rounded border border-border bg-background px-3 py-1.5 text-xs font-medium text-foreground hover:bg-muted disabled:opacity-50">
          <Save className="size-3" />
          保存填写信息
        </button>
        <button onClick={handleExecute}
          disabled={executing || !selectedCredId}
          className="inline-flex items-center gap-1 rounded bg-primary px-3 py-1.5 text-xs font-medium text-primary-foreground hover:bg-primary/90 disabled:opacity-50">
          {executing ? <Loader2 className="size-3 animate-spin" /> : <Building2 className="size-3" />}
          {executing ? "推送中..." : "推送到 OA 立案"}
        </button>
      </div>

      {/* 进度 / 结果 */}
      {session && (
        <div className="space-y-2">
          {session.status === "running" && (
            <div className="space-y-1">
              <div className="h-1.5 w-full overflow-hidden rounded bg-muted">
                <div className="h-full bg-primary transition-all" style={{ width: `${session.progress_pct}%` }} />
              </div>
              <p className="text-[11px] text-muted-foreground">{session.progress_msg}</p>
            </div>
          )}
          {session.status === "completed" && (
            <div className="flex items-center gap-1.5 rounded bg-green-50 px-3 py-2 text-xs text-green-700">
              <CheckCircle2 className="size-3.5" /> OA 立案完成
            </div>
          )}
          {session.status === "failed" && (
            <div className="flex items-center gap-1.5 rounded bg-red-50 px-3 py-2 text-xs text-red-700">
              <XCircle className="size-3.5" /> {session.error_message || "立案失败"}
            </div>
          )}
        </div>
      )}
    </div>
  );
}
