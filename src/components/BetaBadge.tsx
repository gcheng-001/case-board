import { cn } from "@/lib/utils";

/**
 * 「Beta」小徽标 —— 标注实验性/新功能(2026-06-18)。
 * 刑事 tab、非诉合同审查等尚在打磨的功能用,提示用户结果需自行核对。
 */
export function BetaBadge({ className }: { className?: string }) {
  return (
    <span
      className={cn(
        "inline-flex items-center rounded-full border border-amber-500/40 bg-amber-500/10 px-1.5 text-[10px] font-semibold uppercase leading-4 tracking-wide text-amber-600 dark:text-amber-400",
        className,
      )}
    >
      Beta
    </span>
  );
}
