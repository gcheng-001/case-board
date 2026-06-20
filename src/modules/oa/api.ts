/**
 * OA 系统对接 — 前端 API 层。
 *
 * 封装所有 Tauri invoke 调用,对应 Rust 端 `oa/mod.rs` 的命令。
 */

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

// ─────────── 类型定义 ───────────

export interface OAConfig {
  id: string;
  site_name: string;
  login_url: string;
  oa_type: string;
  is_enabled: number;
  field_mapping: string;
  created_at: string;
  updated_at: string;
}

export interface OACredential {
  id: string;
  oa_config_id: string;
  account: string;
  display_name: string | null;
  cookies_path: string | null;
  last_login_at: string | null;
  created_at: string;
  updated_at: string;
}

export interface OASession {
  id: string;
  oa_config_id: string;
  session_type: string;
  status: string;
  case_id: string | null;
  progress_pct: number;
  progress_msg: string;
  result_json: string | null;
  error_message: string;
  started_at: string | null;
  completed_at: string | null;
  created_at: string;
  updated_at: string;
}

export interface OAApprovalOptions {
  risk_fee_amount?: number | null;
  conflict_reviewed?: boolean | null;
  conflict_memo?: string | null;
  risk_contract_confirmed?: boolean | null;
  risk_notice_confirmed?: boolean | null;
  fee_reviewed?: boolean | null;
  fee_memo?: string | null;
  min_fee?: number | null;
  low_ratio?: number | null;
  high_ratio?: number | null;
  risk_base_fee_min?: number | null;
}

export interface OAApprovalListRow {
  id?: number | string;
  lawcaseId?: number | string;
  no?: string | null;
  preNo?: string | null;
  status?: number;
  statusName?: string | null;
  wtrNames?: string | null;
  dsrNames?: string | null;
  tosNames?: string | null;
  empNames?: string | null;
  causeAction?: string | null;
  chargeMethodName?: string | null;
  chargeAmount?: number | string | null;
  yishou?: number | string | null;
  weishou?: number | string | null;
  shouliDate?: string | null;
}

export interface OAApprovalPendingResult {
  filing: OAApprovalListRow[];
  closing: OAApprovalListRow[];
  new_filing?: OAApprovalListRow[];
  counts?: { filing?: number; closing?: number; total?: number; new_filing?: number };
  fetched_at?: string;
}

export interface OAApprovalReview {
  lawcase_id: number;
  case_no?: string | null;
  status?: number;
  status_name?: string | null;
  summary?: Record<string, unknown>;
  completeness_review?: Record<string, unknown>;
  conflict_review?: Record<string, unknown>;
  duplicate_filing_review?: Record<string, unknown>;
  local_case_check?: Record<string, unknown>;
  risk_charge_review?: Record<string, unknown>;
  fee_reasonableness_review?: Record<string, unknown>;
  fallback_review?: Record<string, unknown>;
  recommendation?: { result?: string; label?: string; reasons?: string[] };
}

// ─────────── OA 配置 ───────────

export function oaListConfigs(): Promise<OAConfig[]> {
  return invoke<OAConfig[]>("oa_list_configs");
}

export function oaCreateConfig(config: {
  site_name: string;
  login_url: string;
  oa_type?: string;
  field_mapping?: string;
}): Promise<OAConfig> {
  return invoke<OAConfig>("oa_create_config", { config });
}

export function oaUpdateConfig(
  id: string,
  patch: {
    site_name?: string;
    login_url?: string;
    oa_type?: string;
    is_enabled?: number;
    field_mapping?: string;
  },
): Promise<OAConfig> {
  return invoke<OAConfig>("oa_update_config", { id, patch });
}

export function oaDeleteConfig(id: string): Promise<void> {
  return invoke<void>("oa_delete_config", { id });
}

// ─────────── 凭证 ───────────

export function oaListCredentials(configId: string): Promise<OACredential[]> {
  return invoke<OACredential[]>("oa_list_credentials", { configId });
}

export function oaCreateCredential(
  configId: string,
  account: string,
  password: string,
  displayName?: string,
): Promise<OACredential> {
  return invoke<OACredential>("oa_create_credential", {
    configId,
    account,
    password,
    displayName: displayName ?? null,
  });
}

export function oaDeleteCredential(id: string): Promise<void> {
  return invoke<void>("oa_delete_credential", { id });
}

// ─────────── 会话 ───────────

export function oaListSessions(configId?: string, limit?: number): Promise<OASession[]> {
  return invoke<OASession[]>("oa_list_sessions", {
    configId: configId ?? null,
    limit: limit ?? null,
  });
}

export function oaGetSession(id: string): Promise<OASession | null> {
  return invoke<OASession | null>("oa_get_session", { id });
}

// ─────────── 操作 ───────────

export function oaExecuteFiling(
  configId: string,
  caseId: string,
  credentialId: string,
  filingOptions?: Record<string, unknown>,
): Promise<OASession> {
  return invoke<OASession>("oa_execute_filing", {
    configId,
    caseId,
    credentialId,
    filingOptions: filingOptions ?? null,
  });
}

export interface OADownloadEngagementResult {
  lawcase_id: number;
  case_no?: string | null;
  templates?: string[];
  path?: string;
  filename?: string;
  size_bytes?: number;
  client_is_legal_person?: boolean;
  sync?: { added: number; updated: number; unchanged: number; deleted: number };
}

export function oaDownloadEngagementDocuments(
  configId: string,
  caseId: string,
  credentialId: string,
  lawcaseId: number,
): Promise<OADownloadEngagementResult> {
  return invoke<OADownloadEngagementResult>("oa_download_engagement_documents", {
    configId,
    caseId,
    credentialId,
    lawcaseId,
  });
}

export function oaStartCaseImport(
  configId: string,
  credentialId: string,
): Promise<OASession> {
  return invoke<OASession>("oa_start_case_import", { configId, credentialId });
}

export function oaStartClientImport(
  configId: string,
  credentialId: string,
): Promise<OASession> {
  return invoke<OASession>("oa_start_client_import", { configId, credentialId });
}

export function oaPendingApprovals(
  configId: string,
  credentialId: string,
): Promise<OAApprovalPendingResult> {
  return invoke<OAApprovalPendingResult>("oa_pending_approvals", { configId, credentialId });
}

export function oaApprovalMonitorSnapshot(
  configId: string,
  credentialId: string,
  options?: OAApprovalOptions,
): Promise<OAApprovalPendingResult> {
  return invoke<OAApprovalPendingResult>("oa_approval_monitor_snapshot", {
    configId,
    credentialId,
    options: options ?? null,
  });
}

export function oaApprovalCheck(
  configId: string,
  credentialId: string,
  lawcaseId: number,
  options?: OAApprovalOptions,
): Promise<OAApprovalReview> {
  return invoke<OAApprovalReview>("oa_approval_check", {
    configId,
    credentialId,
    lawcaseId,
    options: options ?? null,
  });
}

export function oaApproveCase(
  configId: string,
  credentialId: string,
  lawcaseId: number,
  memo: string,
  options?: OAApprovalOptions,
): Promise<OASession> {
  return invoke<OASession>("oa_approve_case", {
    configId,
    credentialId,
    lawcaseId,
    memo,
    options: options ?? null,
  });
}

export function oaRejectCase(
  configId: string,
  credentialId: string,
  lawcaseId: number,
  memo: string,
  options?: OAApprovalOptions,
): Promise<OASession> {
  return invoke<OASession>("oa_reject_case", {
    configId,
    credentialId,
    lawcaseId,
    memo,
    options: options ?? null,
  });
}

// ─────────── 事件监听 ───────────

export function onOASessionProgress(
  callback: (payload: { session_id: string; pct: number; msg: string }) => void,
) {
  return listen<{ session_id: string; pct: number; msg: string }>(
    "oa-session-progress",
    (e) => callback(e.payload),
  );
}

export function onOASessionError(
  callback: (payload: { session_id: string; error: string }) => void,
) {
  return listen<{ session_id: string; error: string }>(
    "oa-session-error",
    (e) => callback(e.payload),
  );
}
