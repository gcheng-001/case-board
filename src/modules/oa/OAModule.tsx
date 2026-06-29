/**
 * OA 系统对接 — 主模块。
 *
 * 三态:
 *   - 列表态:显示已配置的 OA 站点
 *   - 配置态:添加/编辑 OA 站点
 *   - 操作态:立案/导入操作界面
 */

import { useEffect, useState, useCallback } from "react";
import {
  ArrowLeft,
  Building2,
  Plus,
  Settings,
  Trash2,
  Loader2,
  FileText,
  CheckCircle2,
  XCircle,
  Clock,
} from "lucide-react";

import type { OAConfig, OACredential, OASession } from "./api";
import {
  oaListConfigs,
  oaCreateConfig,
  oaDeleteConfig,
  oaListCredentials,
  oaCreateCredential,
  oaDeleteCredential,
  oaListSessions,
  onOASessionProgress,
} from "./api";
import { toast } from "@/components/ui/toast";

type ViewMode = "list" | "add-config" | "manage" | "sessions";

export function OAModule() {
  const [configs, setConfigs] = useState<OAConfig[]>([]);
  const [loading, setLoading] = useState(true);
  const [view, setView] = useState<ViewMode>("list");

  // 添加配置表单
  const [formSiteName, setFormSiteName] = useState("");
  const [formLoginUrl, setFormLoginUrl] = useState("");
  const [formOaType, setFormOaType] = useState("nedev");

  // 凭证管理。Nedev/摩尚 AgentAPI 使用 API Key,复用 password 的 Keychain 存储位。
  const [selectedConfig, setSelectedConfig] = useState<OAConfig | null>(null);
  const [credentials, setCredentials] = useState<OACredential[]>([]);
  const [credAccount, setCredAccount] = useState("");
  const [credPassword, setCredPassword] = useState("");
  const [credDisplayName, setCredDisplayName] = useState("");

  // 会话列表
  const [sessions, setSessions] = useState<OASession[]>([]);

  const loadConfigs = useCallback(async () => {
    try {
      const list = await oaListConfigs();
      setConfigs(list);
    } catch {
      toast("加载 OA 配置失败", "error");
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    loadConfigs();
  }, [loadConfigs]);

  // 监听进度事件
  useEffect(() => {
    const unlisten = onOASessionProgress((p) => {
      setSessions((prev) =>
        prev.map((s) =>
          s.id === p.session_id
            ? { ...s, progress_pct: p.pct, progress_msg: p.msg }
            : s,
        ),
      );
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  const handleAddConfig = async () => {
    if (!formSiteName.trim() || !formLoginUrl.trim()) {
      toast("请填写站点名称和登录地址", "error");
      return;
    }
    try {
      await oaCreateConfig({
        site_name: formSiteName.trim(),
        login_url: formLoginUrl.trim(),
        oa_type: formOaType,
      });
      toast("OA 配置已添加", "success");
      setFormSiteName("");
      setFormLoginUrl("");
      setView("list");
      loadConfigs();
    } catch (e) {
      toast(`添加失败: ${e}`, "error");
    }
  };

  const handleDeleteConfig = async (id: string) => {
    if (!confirm("确定删除此 OA 配置？关联的凭证和会话也会被删除。")) return;
    try {
      await oaDeleteConfig(id);
      toast("已删除", "success");
      loadConfigs();
    } catch (e) {
      toast(`删除失败: ${e}`, "error");
    }
  };

  const loadCredentials = async (config: OAConfig) => {
    setSelectedConfig(config);
    setView("manage");
    try {
      const list = await oaListCredentials(config.id);
      setCredentials(list);
    } catch {
      toast("加载凭证失败", "error");
    }
  };

  const handleAddCredential = async () => {
    if (!selectedConfig || !credAccount.trim() || !credPassword.trim()) {
      toast(selectedConfig?.oa_type === "nedev" ? "请填写标识和 API Key" : "请填写账号和密码", "error");
      return;
    }
    try {
      await oaCreateCredential(
        selectedConfig.id,
        credAccount.trim(),
        credPassword.trim(),
        credDisplayName.trim() || undefined,
      );
      toast("凭证已添加", "success");
      setCredAccount("");
      setCredPassword("");
      setCredDisplayName("");
      loadCredentials(selectedConfig);
    } catch (e) {
      toast(`添加凭证失败: ${e}`, "error");
    }
  };

  const handleDeleteCredential = async (id: string) => {
    if (!confirm("确定删除此凭证？")) return;
    try {
      await oaDeleteCredential(id);
      toast("已删除", "success");
      if (selectedConfig) loadCredentials(selectedConfig);
    } catch (e) {
      toast(`删除失败: ${e}`, "error");
    }
  };

  const loadSessions = async (configId?: string) => {
    setView("sessions");
    try {
      const list = await oaListSessions(configId, 50);
      setSessions(list);
    } catch {
      toast("加载会话失败", "error");
    }
  };

  // ─────────── 状态标签 ───────────
  const StatusBadge = ({ status }: { status: string }) => {
    const map: Record<string, { icon: typeof CheckCircle2; color: string; label: string }> = {
      pending: { icon: Clock, color: "text-yellow-600", label: "待开始" },
      running: { icon: Loader2, color: "text-blue-600", label: "进行中" },
      completed: { icon: CheckCircle2, color: "text-green-600", label: "已完成" },
      failed: { icon: XCircle, color: "text-red-600", label: "失败" },
    };
    const info = map[status] ?? map.pending;
    const Icon = info.icon;
    return (
      <span className={`inline-flex items-center gap-1 text-xs ${info.color}`}>
        <Icon className={`size-3 ${status === "running" ? "animate-spin" : ""}`} />
        {info.label}
      </span>
    );
  };

  const sessionTypeLabel = (t: string) =>
    t === "filing" ? "OA 立案" : t === "case_import" ? "案件导入" : t === "client_import" ? "客户导入" : t;
  const selectedUsesApiKey = selectedConfig?.oa_type === "nedev";

  // ─────────── 渲染 ───────────

  if (loading) {
    return (
      <div className="flex h-full items-center justify-center">
        <Loader2 className="size-5 animate-spin text-muted-foreground" />
      </div>
    );
  }

  // 添加配置表单
  if (view === "add-config") {
    return (
      <main className="flex h-full w-full flex-col bg-background">
        <header className="flex shrink-0 items-center gap-3 border-b border-border bg-card/50 px-6 py-2.5">
          <button type="button" onClick={() => setView("list")}
            className="inline-flex items-center gap-1 rounded px-2 py-1 text-xs text-muted-foreground transition-colors hover:bg-accent hover:text-foreground">
            <ArrowLeft className="size-3.5" /> 返回
          </button>
          <span className="text-muted-foreground/40">·</span>
          <h2 className="text-sm font-medium text-foreground">添加 OA 系统</h2>
        </header>
        <div className="mx-auto max-w-lg space-y-4 px-6 py-6">
          <div>
            <label className="mb-1 block text-xs font-medium text-muted-foreground">站点名称</label>
            <input value={formSiteName} onChange={(e) => setFormSiteName(e.target.value)}
              placeholder="如: 摩尚OA" className="w-full rounded border border-border bg-background px-3 py-2 text-sm" />
          </div>
          <div>
            <label className="mb-1 block text-xs font-medium text-muted-foreground">登录地址</label>
            <input value={formLoginUrl} onChange={(e) => setFormLoginUrl(e.target.value)}
              placeholder="如: https://moshang2.ycq6.com/" className="w-full rounded border border-border bg-background px-3 py-2 text-sm" />
          </div>
          <div>
            <label className="mb-1 block text-xs font-medium text-muted-foreground">OA 平台类型</label>
            <select value={formOaType} onChange={(e) => setFormOaType(e.target.value)}
              className="w-full rounded border border-border bg-background px-3 py-2 text-sm">
              <option value="nedev">Nedev（能迪）</option>
              <option value="jtn">金诚同达</option>
              <option value="generic">通用 Web OA</option>
            </select>
          </div>
          <button onClick={handleAddConfig}
            className="w-full rounded bg-primary px-4 py-2 text-sm font-medium text-primary-foreground hover:bg-primary/90">
            保存配置
          </button>
        </div>
      </main>
    );
  }

  // 凭证管理
  if (view === "manage" && selectedConfig) {
    return (
      <main className="flex h-full w-full flex-col bg-background">
        <header className="flex shrink-0 items-center gap-3 border-b border-border bg-card/50 px-6 py-2.5">
          <button type="button" onClick={() => { setView("list"); setSelectedConfig(null); }}
            className="inline-flex items-center gap-1 rounded px-2 py-1 text-xs text-muted-foreground transition-colors hover:bg-accent hover:text-foreground">
            <ArrowLeft className="size-3.5" /> 返回
          </button>
          <span className="text-muted-foreground/40">·</span>
          <h2 className="text-sm font-medium text-foreground">{selectedConfig.site_name} · 账号管理</h2>
        </header>
        <div className="mx-auto max-w-lg space-y-6 px-6 py-6">
          {/* 添加凭证 */}
          <div className="rounded border border-border p-4 space-y-3">
            <h3 className="text-xs font-semibold text-muted-foreground">
              {selectedUsesApiKey ? "添加 AgentAPI Key" : "添加登录账号"}
            </h3>
            <input value={credAccount} onChange={(e) => setCredAccount(e.target.value)}
              placeholder={selectedUsesApiKey ? "标识名,如: 高澄" : "OA 登录账号"}
              className="w-full rounded border border-border bg-background px-3 py-2 text-sm" />
            <input value={credPassword} onChange={(e) => setCredPassword(e.target.value)} type="password"
              placeholder={selectedUsesApiKey ? "Agent API Key" : "OA 登录密码"}
              className="w-full rounded border border-border bg-background px-3 py-2 text-sm" />
            <input value={credDisplayName} onChange={(e) => setCredDisplayName(e.target.value)}
              placeholder="显示名(可选,如: 张三律师)" className="w-full rounded border border-border bg-background px-3 py-2 text-sm" />
            <p className="text-[11px] text-muted-foreground">
              {selectedUsesApiKey ? "API Key 存储在 macOS Keychain 中,不落盘不明文。" : "密码存储在 macOS Keychain 中,不落盘不明文。"}
            </p>
            <button onClick={handleAddCredential}
              className="rounded bg-primary px-4 py-1.5 text-xs font-medium text-primary-foreground hover:bg-primary/90">
              {selectedUsesApiKey ? "添加 API Key" : "添加账号"}
            </button>
          </div>

          {/* 凭证列表 */}
          <div className="space-y-2">
            <h3 className="text-xs font-semibold text-muted-foreground">
              {selectedUsesApiKey ? "已保存 API Key" : "已保存账号"}
            </h3>
            {credentials.length === 0 ? (
              <p className="text-xs text-muted-foreground">{selectedUsesApiKey ? "暂无 API Key" : "暂无账号"}</p>
            ) : (
              credentials.map((c) => (
                <div key={c.id} className="flex items-center justify-between rounded border border-border px-3 py-2">
                  <div>
                    <span className="text-sm font-medium">{c.account}</span>
                    {c.display_name && <span className="ml-2 text-xs text-muted-foreground">{c.display_name}</span>}
                    {c.last_login_at && <span className="ml-2 text-[11px] text-muted-foreground">上次登录: {c.last_login_at}</span>}
                  </div>
                  <button onClick={() => handleDeleteCredential(c.id)} className="text-red-500 hover:text-red-700">
                    <Trash2 className="size-3.5" />
                  </button>
                </div>
              ))
            )}
          </div>
        </div>
      </main>
    );
  }

  // 会话列表
  if (view === "sessions") {
    return (
      <main className="flex h-full w-full flex-col bg-background">
        <header className="flex shrink-0 items-center gap-3 border-b border-border bg-card/50 px-6 py-2.5">
          <button type="button" onClick={() => setView("list")}
            className="inline-flex items-center gap-1 rounded px-2 py-1 text-xs text-muted-foreground transition-colors hover:bg-accent hover:text-foreground">
            <ArrowLeft className="size-3.5" /> 返回
          </button>
          <span className="text-muted-foreground/40">·</span>
          <h2 className="text-sm font-medium text-foreground">OA 操作记录</h2>
        </header>
        <div className="mx-auto max-w-2xl space-y-3 px-6 py-6">
          {sessions.length === 0 ? (
            <p className="text-xs text-muted-foreground">暂无操作记录</p>
          ) : (
            sessions.map((s) => (
              <div key={s.id} className="rounded border border-border p-3 space-y-2">
                <div className="flex items-center justify-between">
                  <div className="flex items-center gap-2">
                    <span className="text-xs font-medium">{sessionTypeLabel(s.session_type)}</span>
                    <StatusBadge status={s.status} />
                  </div>
                  <span className="text-[11px] text-muted-foreground">{s.created_at}</span>
                </div>
                {s.status === "running" && (
                  <div className="space-y-1">
                    <div className="h-1.5 w-full overflow-hidden rounded bg-muted">
                      <div className="h-full bg-primary transition-all" style={{ width: `${s.progress_pct}%` }} />
                    </div>
                    <p className="text-[11px] text-muted-foreground">{s.progress_msg}</p>
                  </div>
                )}
                {s.status === "failed" && s.error_message && (
                  <p className="text-xs text-red-600">{s.error_message}</p>
                )}
              </div>
            ))
          )}
        </div>
      </main>
    );
  }

  // 默认:列表态
  return (
    <main className="flex h-full w-full flex-col bg-background">
      <div className="flex-1 overflow-auto">
        <div className="mx-auto max-w-3xl space-y-6 px-8 py-6">
          <div className="flex items-center justify-between">
            <div>
              <h2 className="text-sm font-semibold text-foreground">OA 系统对接</h2>
              <p className="mt-0.5 text-xs text-muted-foreground">
                对接律所 OA 系统,实现立案推送、案件导入、客户导入
              </p>
            </div>
            <button onClick={() => setView("add-config")}
              className="inline-flex items-center gap-1 rounded bg-primary px-3 py-1.5 text-xs font-medium text-primary-foreground hover:bg-primary/90">
              <Plus className="size-3" /> 添加 OA 系统
            </button>
          </div>

          {configs.length === 0 ? (
            <div className="rounded border border-dashed border-border p-8 text-center">
              <Building2 className="mx-auto size-8 text-muted-foreground/40" />
              <p className="mt-2 text-sm text-muted-foreground">尚未配置 OA 系统</p>
              <p className="mt-1 text-xs text-muted-foreground">点击上方按钮添加你的律所 OA</p>
            </div>
          ) : (
            <div className="space-y-3">
              {configs.map((cfg) => (
                <div key={cfg.id} className="rounded border border-border p-4">
                  <div className="flex items-center justify-between">
                    <div className="flex items-center gap-3">
                      <Building2 className="size-5 text-muted-foreground" />
                      <div>
                        <h3 className="text-sm font-medium">{cfg.site_name}</h3>
                        <p className="text-[11px] text-muted-foreground">{cfg.login_url}</p>
                      </div>
                    </div>
                    <div className="flex items-center gap-2">
                      <span className={`rounded px-1.5 py-0.5 text-[10px] ${cfg.is_enabled ? "bg-green-100 text-green-700" : "bg-muted text-muted-foreground"}`}>
                        {cfg.is_enabled ? "启用" : "禁用"}
                      </span>
                      <button onClick={() => loadCredentials(cfg)} className="text-muted-foreground hover:text-foreground" title="管理账号">
                        <Settings className="size-3.5" />
                      </button>
                      <button onClick={() => handleDeleteConfig(cfg.id)} className="text-red-500 hover:text-red-700" title="删除">
                        <Trash2 className="size-3.5" />
                      </button>
                    </div>
                  </div>

                  {/* 操作按钮 */}
                  <div className="mt-3 flex gap-2">
                    <button onClick={() => loadSessions(cfg.id)}
                      className="inline-flex items-center gap-1 rounded border border-border px-2.5 py-1 text-xs text-muted-foreground hover:bg-accent hover:text-foreground">
                      <FileText className="size-3" /> 操作记录
                    </button>
                  </div>
                </div>
              ))}
            </div>
          )}

          {/* 最近操作 */}
          {configs.length > 0 && (
            <button onClick={() => loadSessions()}
              className="text-xs text-muted-foreground hover:text-foreground">
              查看所有操作记录 →
            </button>
          )}
        </div>
      </div>
    </main>
  );
}
