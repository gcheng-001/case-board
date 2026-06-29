import { useState } from "react";
import { ChevronDown, Folder } from "lucide-react";

import { type Case } from "@/lib/types";
import { cn } from "@/lib/utils";

export function CaseSwitcher({
  cases,
  selectedId,
  onSwitch,
}: {
  cases: Case[];
  selectedId: string | null;
  onSwitch: (id: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const selected = cases.find((c) => c.id === selectedId);
  return (
    <div className="relative">
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        className="inline-flex items-baseline gap-1.5 text-xl font-semibold tracking-tight text-foreground transition-opacity hover:opacity-70"
      >
        <span>{selected?.name ?? "—"}</span>
        <ChevronDown className="size-4 self-center text-muted-foreground" />
      </button>
      {open && (
        <>
          {/* 点击外部关闭 */}
          <div
            className="fixed inset-0 z-10"
            onClick={() => setOpen(false)}
            aria-hidden
          />
          <ul className="absolute left-0 top-full z-20 mt-1 min-w-64 overflow-hidden rounded-md border border-border bg-popover shadow-lg animate-in zoom-in-95 fade-in-0 duration-150 ease-out">
            {cases.map((c) => (
              <li key={c.id}>
                <button
                  type="button"
                  onClick={() => {
                    onSwitch(c.id);
                    setOpen(false);
                  }}
                  className={cn(
                    "block w-full px-3 py-2 text-left text-sm transition-colors hover:bg-accent",
                    c.id === selectedId && "bg-accent/50",
                  )}
                >
                  <div className="font-medium text-foreground">{c.name}</div>
                  <div className="mt-0.5 flex min-w-0 items-center gap-1 text-[11px] text-muted-foreground/70">
                    <Folder className="size-3 shrink-0 text-amber-500/50" />
                    <span className="min-w-0 truncate">
                      {c.source_folder.split("/").filter(Boolean).slice(-2).join(" > ")}
                    </span>
                  </div>
                </button>
              </li>
            ))}
          </ul>
        </>
      )}
    </div>
  );
}
