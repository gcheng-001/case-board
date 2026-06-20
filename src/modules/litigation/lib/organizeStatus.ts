/**
 * 「AI 自动整理」运行态的跨组件存储(2026-06-19)。
 *
 * 为什么要单独存:`organizing` 若放 CaseView 的 useState,切走标签页 → CaseView 卸载 →
 * 状态丢失 → 切回来按钮不再显示「整理中」(但后端其实还在跑)。这里按 case_id 记在
 * 模块级单例里,并**在模块初始化时挂一次全局 Tauri 监听**(App 生命周期常驻)清除完成/失败的,
 * 这样无论 CaseView 怎么卸载重挂,spinner 状态都正确。
 */
import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";

const organizing = new Set<string>();
const subs = new Set<() => void>();

function notify() {
  subs.forEach((f) => f());
}

/** 发起整理时标记某案件「整理中」。 */
export function markOrganizeStarted(caseId: string): void {
  organizing.add(caseId);
  notify();
}

function clearOrganizing(caseId: string): void {
  if (organizing.delete(caseId)) notify();
}

// 模块级全局监听:后端跑完(无论前端在哪个页面)都清掉对应 case 的「整理中」。
// 单例、不 unlisten(随 App 生命周期常驻)。
void listen<{ case_id: string }>("organize_done", (e) =>
  clearOrganizing(e.payload.case_id),
);
void listen<{ case_id: string }>("organize_failed", (e) =>
  clearOrganizing(e.payload.case_id),
);

/** 订阅某案件是否「整理中」(跨 CaseView 卸载重挂保持)。 */
export function useOrganizing(caseId: string | undefined): boolean {
  const [v, setV] = useState(() => !!caseId && organizing.has(caseId));
  useEffect(() => {
    const f = () => setV(!!caseId && organizing.has(caseId));
    f();
    subs.add(f);
    return () => {
      subs.delete(f);
    };
  }, [caseId]);
  return v;
}
