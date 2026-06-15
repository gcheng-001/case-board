// 设置页 · 「法律向量检索」维护卡(法条 + 案例 + 企业 语义索引)
//
// 公开功能(进开源版):显示索引规模 + 手动重建(带进度)+ 自动维护开关。
// 自动维护:出报告 / 启动 / chat 完成后后台增量索引(见 Rust spawn_kb_auto_index);
// 这里只管「显示状态 + 手动重建 + 开关」。
import { useCallback, useEffect, useState } from "react";
import { Database, RefreshCw } from "lucide-react";
import { listen } from "@tauri-apps/api/event";
import {
  getLocalKbIndexStats,
  buildLocalKbSemanticIndex,
  type KbIndexStats,
} from "../lib/api";

interface IndexProgress {
  done: number;
  total: number;
  phase: string;
}

export function KbSemanticIndexCard({
  embeddingConfigured,
  autoIndex,
  onAutoChange,
}: {
  embeddingConfigured: boolean;
  autoIndex: boolean;
  onAutoChange: (v: boolean) => void;
}) {
  const [stats, setStats] = useState<KbIndexStats | null>(null);
  const [building, setBuilding] = useState(false);
  const [progress, setProgress] = useState<IndexProgress | null>(null);
  const [msg, setMsg] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      setStats(await getLocalKbIndexStats());
    } catch {
      /* 没启用 KB 时静默 */
    }
  }, []);

  useEffect(() => {
    void refresh();
    const un = listen<IndexProgress>("kb_index_progress", (e) => {
      const p = e.payload;
      if (p.phase === "needs_manual") {
        setMsg(
          `检测到 ${p.total} 个待索引文件(法律/案例较多),自动索引已跳过——请点「重建索引」手动建一次。`,
        );
        return;
      }
      setProgress(p);
      if (p.phase === "done") {
        setTimeout(() => setProgress(null), 1500);
        void refresh();
      }
    });
    return () => {
      void un.then((f) => f());
    };
  }, [refresh]);

  const onBuild = useCallback(async () => {
    setBuilding(true);
    setMsg(null);
    setProgress({ done: 0, total: 0, phase: "start" });
    try {
      const s = await buildLocalKbSemanticIndex();
      setStats(s);
      setMsg(`索引就绪:${s.files} 个文件 / ${s.chunks} 个切片。`);
    } catch (e) {
      setMsg(`建索引失败:${String(e)}`);
    } finally {
      setBuilding(false);
    }
  }, []);

  return (
    <section>
      <div className="mb-3">
        <h3 className="text-sm font-semibold text-foreground">法律向量检索</h3>
        <p className="mt-0.5 text-xs text-muted-foreground">
          把本地法条 / 案例 / 企业资料建成语义索引,AI 查法条优先命中本地、省元典积分。
        </p>
      </div>
      <div className="space-y-3 rounded-lg border border-border bg-background/50 p-4">
        {!embeddingConfigured && (
          <p className="rounded-md bg-amber-50 px-3 py-2 text-xs text-amber-800">
            需先在上方「硅基流动」配置并验证 embedding,才能建向量索引(bge-m3 免费)。
          </p>
        )}

        <div className="flex items-center justify-between text-sm">
          <span className="text-muted-foreground">已索引</span>
          <span className="font-medium text-foreground">
            {stats ? `${stats.files} 文件 · ${stats.chunks} 切片` : "未建 / 读取中"}
          </span>
        </div>

        {progress && (
          <div className="space-y-1">
            <div className="flex justify-between text-xs text-muted-foreground">
              <span>{progress.phase === "done" ? "完成" : "正在 embed…"}</span>
              <span>
                {progress.done}
                {progress.total > 0 ? ` / ${progress.total}` : ""}
              </span>
            </div>
            <div className="h-1.5 w-full overflow-hidden rounded-full bg-muted">
              <div
                className="h-full bg-sky-500 transition-all"
                style={{
                  width:
                    progress.total > 0
                      ? `${Math.round((progress.done / progress.total) * 100)}%`
                      : "10%",
                }}
              />
            </div>
          </div>
        )}

        <div className="flex items-center gap-2">
          <button
            type="button"
            onClick={onBuild}
            disabled={building || !embeddingConfigured}
            className="inline-flex items-center gap-1.5 rounded-md border border-sky-200 bg-sky-50 px-3 py-1.5 text-sm font-medium text-sky-700 transition-colors hover:bg-sky-100 disabled:opacity-50"
          >
            {building ? (
              <RefreshCw className="size-4 animate-spin" />
            ) : (
              <Database className="size-4" />
            )}
            {building ? "建索引中…" : "重建索引"}
          </button>
          <label className="ml-auto inline-flex cursor-pointer items-center gap-2 text-xs text-muted-foreground">
            <input
              type="checkbox"
              checked={autoIndex}
              onChange={(e) => onAutoChange(e.target.checked)}
              className="size-3.5 accent-sky-600"
            />
            自动维护(出报告 / 启动后台增量)
          </label>
        </div>

        {msg && (
          <div className="rounded-md bg-sky-50 px-3 py-2 text-xs text-sky-800">
            {msg}
          </div>
        )}
        <p className="text-xs text-muted-foreground">
          首次建索引会把整部法律按法条切片 embed,文件多时需几分钟(进度见上),之后增量很快。
        </p>
      </div>
    </section>
  );
}
