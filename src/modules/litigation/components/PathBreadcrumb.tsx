import { ChevronRight, Folder } from "lucide-react";

import { cn } from "@/lib/utils";

export function PathBreadcrumb({
  path,
  className,
  onClick,
}: {
  path: string;
  className?: string;
  onClick?: () => void;
}) {
  const parts = path.split("/").filter(Boolean);
  const maxVisible = 4;

  const segments =
    parts.length <= maxVisible
      ? parts.map((label, index) => ({ label, isLast: index === parts.length - 1 }))
      : [
          { label: parts[0] },
          { label: "...", isEllipsis: true },
          { label: parts[parts.length - 2] },
          { label: parts[parts.length - 1], isLast: true },
        ];

  return (
    <button
      type="button"
      onClick={onClick}
      className={cn(
        "group mt-1 inline-flex max-w-full items-center gap-0.5 rounded-md px-1.5 py-0.5",
        "text-[11px] text-muted-foreground transition-colors",
        "hover:bg-accent/60 hover:text-foreground",
        className,
      )}
      title={`在 Finder 中打开:${path}`}
    >
      <Folder className="mr-1 size-3 shrink-0 text-amber-500/70 group-hover:text-amber-500" />
      {segments.map((segment, index) => (
        <span key={`${segment.label}-${index}`} className="inline-flex items-center gap-0.5">
          {index > 0 && (
            <ChevronRight className="size-2.5 shrink-0 text-muted-foreground/40" />
          )}
          <span
            className={cn(
              "max-w-[100px] truncate",
              segment.isEllipsis && "font-medium text-muted-foreground/50",
              segment.isLast
                ? "font-medium text-foreground/80"
                : "text-muted-foreground/70",
            )}
          >
            {segment.label}
          </span>
        </span>
      ))}
    </button>
  );
}
