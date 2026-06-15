/**
 * 2026-06-13 · 案件待办清单卡(胡彬律师反馈)。
 *
 * 仿滴答清单:顶部输入框回车加一条;未完成列表打钩即完成(从列表消失);
 * 「已完成」可展开查看/撤销/删除。诉讼详情 + 执行详情共用本组件。
 */
import { useEffect, useState } from "react";
import {
  CalendarDays,
  Check,
  ChevronDown,
  ChevronRight,
  Plus,
  Trash2,
} from "lucide-react";

import {
  addTodo,
  deleteTodo,
  listTodos,
  updateTodo,
  type Todo,
} from "@/lib/api";
import { confirmDialog } from "@/lib/dialog";
import { cn } from "@/lib/utils";

/** 待办日期 → 一个小徽章(逾期红 / 近 7 天琥珀 / 其余淡蓝),null 则不显示。 */
function DueBadge({ due }: { due: string | null }) {
  if (!due) return null;
  const today = new Date();
  today.setHours(0, 0, 0, 0);
  const d = new Date(due + "T00:00:00");
  const days = Math.round((d.getTime() - today.getTime()) / 86400000);
  const tone =
    days < 0
      ? "bg-red-50 text-red-700 dark:bg-red-950/30 dark:text-red-300"
      : days <= 7
        ? "bg-amber-50 text-amber-700 dark:bg-amber-950/30 dark:text-amber-300"
        : "bg-sky-50 text-sky-700 dark:bg-sky-950/30 dark:text-sky-300";
  const suffix =
    days < 0 ? `逾期${-days}天` : days === 0 ? "今天" : `${days}天后`;
  return (
    <span
      className={cn(
        "inline-flex shrink-0 items-center gap-1 rounded px-1.5 py-0.5 text-[11px] font-medium",
        tone,
      )}
    >
      <CalendarDays className="size-3" />
      {due.slice(5)} · {suffix}
    </span>
  );
}

export function TodosCard({ caseId }: { caseId: string }) {
  const [todos, setTodos] = useState<Todo[]>([]);
  const [input, setInput] = useState("");
  const [dueInput, setDueInput] = useState(""); // 可选日期(YYYY-MM-DD)
  const [adding, setAdding] = useState(false);
  const [showDone, setShowDone] = useState(false);

  useEffect(() => {
    let cancelled = false;
    listTodos(caseId)
      .then((t) => {
        if (!cancelled) setTodos(t);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [caseId]);

  const pending = todos.filter((t) => t.done === 0);
  const done = todos.filter((t) => t.done === 1);

  const handleAdd = async () => {
    const title = input.trim();
    if (!title || adding) return;
    setAdding(true);
    try {
      const t = await addTodo({
        case_id: caseId,
        title,
        due_date: dueInput || null,
      });
      setTodos((prev) => [t, ...prev]);
      setInput("");
      setDueInput("");
    } catch (e) {
      alert(`添加失败:${e}`);
    } finally {
      setAdding(false);
    }
  };

  const handleToggle = async (todo: Todo) => {
    const next = todo.done === 1 ? 0 : 1;
    // 乐观更新
    setTodos((prev) =>
      prev.map((t) => (t.id === todo.id ? { ...t, done: next } : t)),
    );
    try {
      await updateTodo(todo.id, { done: next });
    } catch (e) {
      // 回滚
      setTodos((prev) =>
        prev.map((t) => (t.id === todo.id ? { ...t, done: todo.done } : t)),
      );
      alert(`更新失败:${e}`);
    }
  };

  const handleDelete = async (todo: Todo) => {
    if (
      !(await confirmDialog(`删除待办「${todo.title}」?`, {
        okLabel: "删除",
        danger: true,
      }))
    )
      return;
    try {
      await deleteTodo(todo.id);
      setTodos((prev) => prev.filter((t) => t.id !== todo.id));
    } catch (e) {
      alert(`删除失败:${e}`);
    }
  };

  return (
    <div className="space-y-3">
      {/* 添加输入框(回车保存) */}
      <div className="flex items-center gap-2 rounded-md border border-border bg-card px-2.5 py-1.5">
        <Plus className="size-4 shrink-0 text-muted-foreground" />
        <input
          type="text"
          value={input}
          onChange={(e) => setInput(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              void handleAdd();
            }
          }}
          placeholder="添加待办…(回车保存)"
          className="flex-1 bg-transparent text-sm outline-none placeholder:text-muted-foreground"
        />
        <input
          type="date"
          value={dueInput}
          onChange={(e) => setDueInput(e.target.value)}
          title="可选:设个日期 → 进首页日程日历"
          className="shrink-0 bg-transparent text-xs text-muted-foreground outline-none"
        />
        {input.trim() && (
          <button
            type="button"
            onClick={() => void handleAdd()}
            disabled={adding}
            className="rounded bg-sky-600 px-2.5 py-1 text-xs font-medium text-white hover:bg-sky-700 disabled:opacity-60"
          >
            添加
          </button>
        )}
      </div>

      {/* 未完成列表 */}
      {pending.length === 0 ? (
        <p className="px-1 py-2 text-xs text-muted-foreground">
          {done.length > 0 ? "未完成的都做完了 🎉" : "暂无待办"}
        </p>
      ) : (
        <ul className="space-y-1">
          {pending.map((todo) => (
            <li
              key={todo.id}
              className="group flex items-center gap-2.5 rounded-md px-1.5 py-1.5 hover:bg-muted/40"
            >
              <button
                type="button"
                onClick={() => void handleToggle(todo)}
                aria-label="标记完成"
                className="flex size-4 shrink-0 items-center justify-center rounded-[4px] border border-muted-foreground/50 hover:border-sky-600"
              />
              <span className="flex-1 text-sm text-foreground">{todo.title}</span>
              <DueBadge due={todo.due_date} />
              <button
                type="button"
                onClick={() => void handleDelete(todo)}
                aria-label="删除"
                className="shrink-0 rounded p-1 text-muted-foreground opacity-0 transition hover:bg-destructive/10 hover:text-destructive group-hover:opacity-100"
              >
                <Trash2 className="size-3.5" />
              </button>
            </li>
          ))}
        </ul>
      )}

      {/* 已完成(可展开) */}
      {done.length > 0 && (
        <div className="border-t border-border/60 pt-2">
          <button
            type="button"
            onClick={() => setShowDone((v) => !v)}
            className="flex items-center gap-1 text-xs text-muted-foreground hover:text-foreground"
          >
            {showDone ? (
              <ChevronDown className="size-3.5" />
            ) : (
              <ChevronRight className="size-3.5" />
            )}
            已完成 {done.length}
          </button>
          {showDone && (
            <ul className="mt-1 space-y-1">
              {done.map((todo) => (
                <li
                  key={todo.id}
                  className="group flex items-center gap-2.5 rounded-md px-1.5 py-1.5 hover:bg-muted/40"
                >
                  <button
                    type="button"
                    onClick={() => void handleToggle(todo)}
                    aria-label="取消完成"
                    className="flex size-4 shrink-0 items-center justify-center rounded-[4px] border border-sky-600 bg-sky-600 text-white"
                  >
                    <Check className="size-3" />
                  </button>
                  <span className="flex-1 text-sm text-muted-foreground line-through">
                    {todo.title}
                  </span>
                  <button
                    type="button"
                    onClick={() => void handleDelete(todo)}
                    aria-label="删除"
                    className="shrink-0 rounded p-1 text-muted-foreground opacity-0 transition hover:bg-destructive/10 hover:text-destructive group-hover:opacity-100"
                  >
                    <Trash2 className="size-3.5" />
                  </button>
                </li>
              ))}
            </ul>
          )}
        </div>
      )}
    </div>
  );
}
