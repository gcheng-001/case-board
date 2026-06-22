import { useState } from "react";
import {
  Code2,
  ExternalLink,
  Loader2,
  RefreshCw,
  TerminalSquare,
} from "lucide-react";

import { Button } from "@/components/ui/button";
import { toast } from "@/components/ui/toast";
import {
  type ClaudeHistoryRecord,
  openCaseInClaudeCode,
  openCaseInVSCode,
  revealInFinder,
  syncClaudeHistoryForCase,
} from "@/lib/api";
import type { Case } from "@/lib/types";

export function AgentExecutionPanel({
  caseData,
  compact = false,
}: {
  caseData: Case;
  compact?: boolean;
}) {
  const [syncing, setSyncing] = useState(false);
  const [records, setRecords] = useState<ClaudeHistoryRecord[]>([]);
  const [notePath, setNotePath] = useState<string | null>(null);
  const [latestAt, setLatestAt] = useState<string | null>(null);
  const sourceFolder = caseData.source_folder;

  const openVSCode = async () => {
    try {
      await openCaseInVSCode(sourceFolder);
    } catch (e) {
      toast(`打开 VS Code 失败:${e}`, "error");
    }
  };

  const openClaude = async () => {
    try {
      await openCaseInClaudeCode(sourceFolder);
    } catch (e) {
      toast(`打开 Claude Code 失败:${e}`, "error");
    }
  };

  const syncClaudeHistory = async () => {
    setSyncing(true);
    try {
      const result = await syncClaudeHistoryForCase({
        case_name: caseData.name,
        source_folder: sourceFolder,
        limit: 80,
      });
      setRecords(result.records);
      setNotePath(result.note_path);
      setLatestAt(result.latest_at);
      if (result.imported_count > 0) {
        toast(`已提炼 ${result.imported_count} 条案件相关记录`, "success");
      } else {
        toast("没有提炼到可用的案件参考内容", "info");
      }
    } catch (e) {
      toast(`同步失败:${e}`, "error");
    } finally {
      setSyncing(false);
    }
  };

  if (compact) {
    return (
      <section className="shrink-0 border-b border-border bg-background/95 px-3 py-2">
        <div className="flex flex-wrap items-center gap-1.5">
          <span className="mr-1 text-xs font-medium text-muted-foreground">
            Agent
          </span>
          <Button type="button" size="sm" variant="outline" onClick={openVSCode}>
            <Code2 className="size-3.5" />
            VS Code
          </Button>
          <Button type="button" size="sm" variant="outline" onClick={openClaude}>
            <TerminalSquare className="size-3.5" />
            Claude
          </Button>
          <Button
            type="button"
            size="sm"
            onClick={syncClaudeHistory}
            disabled={syncing}
          >
            {syncing ? (
              <Loader2 className="size-3.5 animate-spin" />
            ) : (
              <RefreshCw className="size-3.5" />
            )}
            提炼
          </Button>
          {notePath && (
            <Button
              type="button"
              size="sm"
              variant="outline"
              onClick={() => revealInFinder(notePath)}
              title={notePath}
            >
              <ExternalLink className="size-3.5" />
              笔记
            </Button>
          )}
          {records.length > 0 && (
            <span className="ml-1 truncate text-xs text-muted-foreground">
              已提炼 {records.length} 条{latestAt ? ` · ${latestAt}` : ""}
            </span>
          )}
        </div>
      </section>
    );
  }

  return (
    <section className="rounded-md border border-border bg-card/70 p-3 shadow-sm">
      <div className="mb-2 flex items-start justify-between gap-2">
        <div>
          <h2 className="flex items-center gap-2 text-sm font-semibold text-foreground">
            <TerminalSquare className="size-4" />
            VS Code 提炼同步
          </h2>
          <p className="mt-1 text-xs text-muted-foreground">
            在 VS Code / Claude Code 里正常提问,回来点提炼,系统只保留案件结论、问题、待办和材料线索。
          </p>
        </div>
        <Button
          type="button"
          size="sm"
          onClick={syncClaudeHistory}
          disabled={syncing}
        >
          {syncing ? (
            <Loader2 className="size-3.5 animate-spin" />
          ) : (
            <RefreshCw className="size-3.5" />
          )}
          提炼
        </Button>
      </div>

      <div className="mb-2 flex flex-wrap gap-1.5">
        <Button type="button" size="sm" variant="outline" onClick={openVSCode}>
          <Code2 className="size-3.5" />
          VS Code
        </Button>
        <Button type="button" size="sm" variant="outline" onClick={openClaude}>
          <TerminalSquare className="size-3.5" />
          Claude
        </Button>
        {notePath && (
          <Button
            type="button"
            size="sm"
            variant="outline"
            onClick={() => revealInFinder(notePath)}
            title={notePath}
          >
            <ExternalLink className="size-3.5" />
            笔记
          </Button>
        )}
      </div>

      {records.length === 0 ? (
        <p className="rounded-md bg-muted/40 px-3 py-2 text-xs text-muted-foreground">
          提炼会读取当前案件文件夹对应的 Claude Code 历史,过滤过程信息后生成案件参考笔记。
        </p>
      ) : (
        <div className="space-y-2">
          {records.slice(-8).map((record, index) => (
            <article
              key={`${record.timestamp ?? "record"}-${index}`}
              className="rounded-md border border-border bg-background px-3 py-2"
            >
              <div className="mb-1 flex flex-wrap items-center gap-2 text-xs">
                <span className="font-medium text-foreground">
                  {record.role === "user" ? "你" : "Claude"}
                </span>
                {record.timestamp && (
                  <span className="text-muted-foreground">{record.timestamp}</span>
                )}
              </div>
              <p className="line-clamp-4 whitespace-pre-wrap text-xs leading-relaxed text-muted-foreground">
                {record.content}
              </p>
            </article>
          ))}
        </div>
      )}
    </section>
  );
}
