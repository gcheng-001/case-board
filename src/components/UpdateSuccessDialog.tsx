/**
 * 升级成功提示(应用内更新重启后弹一次)。
 *
 * 2026-06-12 v0.3.14:应用内更新装好、relaunch 后,启动时 consumeJustUpdated() 命中
 * 则弹本框,告诉用户「已升级到 vX.Y.Z + 本次更新内容」。只弹一次(localStorage 已清)。
 */

import { CheckCircle2, X } from "lucide-react";

import { Button } from "@/components/ui/button";

interface Props {
  version: string;
  notes: string | null;
  onClose: () => void;
}

export function UpdateSuccessDialog({ version, notes, onClose }: Props) {
  return (
    <div className="fixed inset-0 z-[100] flex items-center justify-center bg-black/40 p-4 animate-in fade-in-0 duration-200">
      <div className="w-full max-w-md overflow-hidden rounded-lg border border-border bg-card shadow-2xl animate-in zoom-in-95 fade-in-0 duration-300">
        <header className="flex items-start justify-between gap-3 border-b border-border bg-card/95 px-5 py-4">
          <div className="flex items-center gap-2">
            <CheckCircle2 className="size-5 text-emerald-500" />
            <div>
              <h2 className="text-base font-semibold">已升级成功</h2>
              <p className="mt-0.5 text-xs text-muted-foreground">
                当前版本 <span className="font-mono font-semibold text-foreground">v{version}</span>
              </p>
            </div>
          </div>
          <button
            type="button"
            onClick={onClose}
            className="rounded p-1 text-muted-foreground hover:bg-accent hover:text-foreground"
            aria-label="关闭"
          >
            <X className="size-4" />
          </button>
        </header>

        {notes && (
          <div className="border-b border-border bg-muted/30 px-5 py-3">
            <p className="text-label font-medium uppercase tracking-wide text-muted-foreground">
              本次更新内容
            </p>
            <p className="mt-1.5 whitespace-pre-wrap text-xs leading-relaxed text-foreground">
              {notes}
            </p>
          </div>
        )}

        <footer className="flex items-center justify-end border-t border-border bg-card/95 px-5 py-3">
          <Button size="sm" onClick={onClose}>
            知道了
          </Button>
        </footer>
      </div>
    </div>
  );
}
