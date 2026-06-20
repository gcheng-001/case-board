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
