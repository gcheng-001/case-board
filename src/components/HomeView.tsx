/**
 * 首页:案件看板(2026-05-23 晚七加)。
 *
 * 作者要的参考是 lawcasemanager.com,组件结构:
 * - 左上:OVERVIEW · 个人化问候
 * - 右上:重要日期 widget(开庭 / 保全续封 / 上诉期 等近期事件)
 * - 主区:在办案件 卡片网格(名字 / 案号 / 法院 / 法官 / 金额 / 阶段标签)
 * - 点案件卡片 → 进详情页
 *
 * V0.1 数据基础:cases 表已有 agg_* 字段(由 aggregator 写),
 * "重要日期" 没有专门表,所以 V0.1 只能从 agg_filed_at 推一个占位
 * (V0.2 加 events 表 / case_preservations 后才会有真正的"即将到期")。
 */
import { useEffect, useState } from "react";
import {
  FolderOpen,
  FolderSearch,
  CalendarClock,
  ChevronRight,
  ChevronDown,
  Check,
  GripVertical,
  Gavel,
  AlertTriangle,
  ShieldAlert,
} from "lucide-react";

import { CalendarBoard } from "./CalendarBoard";
import {
  DndContext,
  type DragEndEvent,
  PointerSensor,
  closestCenter,
  useSensor,
  useSensors,
} from "@dnd-kit/core";
import {
  SortableContext,
  arrayMove,
  rectSortingStrategy,
  useSortable,
} from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";

import { Button } from "@/components/ui/button";
import { formatYuan } from "@/lib/format";
import {
  getCaseWithDocs,
  getSettings,
  updateHomeCaseOrder,
  updateWorkflowStatus,
} from "@/lib/api";
import type { Case, Document } from "@/lib/types";
import { parseJsonArray } from "@/lib/types";
import { cn } from "@/lib/utils";
import {
  compareCasesByStatusThenTime,
  resolveCaseStatus,
  STATUS_LIST,
  type StatusId,
} from "@/modules/litigation/lib/inferStatus";

export interface HomeViewProps {
  cases: Case[];
  userDisplayName: string | null;
  onPickCase: (caseId: string) => void;
  onImport: () => void;
  onBatchImport: () => void;
  /** 从日历事件导入案件文件夹（传入事件标题作为建议案件名） */
  onImportFolder?: (eventTitle: string) => void;
}

export function HomeView({
  cases,
  userDisplayName,
  onPickCase,
  onImport,
  onBatchImport,
  onImportFolder,
}: HomeViewProps) {
  // 个人化问候(早/午/晚)
  const greeting = getGreeting(userDisplayName);
  const monthLabel = new Date()
    .toLocaleString("en-US", { month: "short", year: "numeric" })
    .toUpperCase();

  // 重要日期(V0.1 占位:从 agg_filed_at 推近期立案的案件,V0.2 接 events 表)
  // 已结案 / 已调解的案件不显示 — 它的立案日已经没意义了(算到下面用 casesSorted 过滤)

  /**
   * 2026-05-24 e:案件 → 文档列表 缓存,用于推断工作流状态。
   * 拉每个 case 的 docs(N 次 IPC,SQLite 本机 N<50 总耗时 <500ms 可接受)。
   * 卡片在 docs 还没拿到时也能显示(走默认推断 = "接案")。
   */
  const [docsByCase, setDocsByCase] = useState<Record<string, Document[]>>({});
  /** 状态手工覆盖的本地乐观更新(避免等 IPC 返回才闪显) */
  const [statusOverride, setStatusOverride] = useState<Record<string, StatusId | null>>({});

  useEffect(() => {
    let cancelled = false;
    Promise.all(
      cases.map(async (c) => {
        try {
          const r = await getCaseWithDocs(c.id);
          return [c.id, r.documents] as const;
        } catch {
          return [c.id, [] as Document[]] as const;
        }
      }),
    ).then((pairs) => {
      if (cancelled) return;
      setDocsByCase(Object.fromEntries(pairs));
    });
    return () => {
      cancelled = true;
    };
  }, [cases]);

  // 2026-05-26 V0.1.13:首页用户拖动后的卡片顺序(从 settings.json 拿,null=没排过)
  const [userOrder, setUserOrder] = useState<string[] | null>(null);
  useEffect(() => {
    let cancelled = false;
    getSettings()
      .then((s) => {
        if (!cancelled) setUserOrder(s.home_case_order);
      })
      .catch(() => {
        /* 读不到也无所谓,走默认排序 */
      });
    return () => {
      cancelled = true;
    };
  }, []);

  // 应用乐观覆盖:把 statusOverride 写回 cases.workflow_status
  const casesWithOverride = cases.map((c) =>
    c.id in statusOverride
      ? { ...c, workflow_status: statusOverride[c.id] }
      : c,
  );

  // 算每个案件的当前状态 → 默认排序(已结案排末尾,其他 updated_at 倒序)
  const defaultSorted = [...casesWithOverride]
    .map((c) => ({
      caseData: c,
      status: resolveCaseStatus(c, docsByCase[c.id] ?? []),
    }))
    .sort((a, b) =>
      compareCasesByStatusThenTime(
        a.status.id,
        a.caseData.updated_at,
        b.status.id,
        b.caseData.updated_at,
      ),
    );

  // 用户拖过 → 按 userOrder 重排,没排过的(新案件 / userOrder 没覆盖到的)按默认顺序追加。
  // 已删的 case id 留在 userOrder 里也无害(idMap 找不到自动 filter)。
  const casesSorted = (() => {
    if (!userOrder || userOrder.length === 0) return defaultSorted;
    const byId = new Map(defaultSorted.map((c) => [c.caseData.id, c]));
    const result: typeof defaultSorted = [];
    const seen = new Set<string>();
    for (const id of userOrder) {
      const c = byId.get(id);
      if (c && !seen.has(id)) {
        result.push(c);
        seen.add(id);
      }
    }
    for (const c of defaultSorted) {
      if (!seen.has(c.caseData.id)) {
        result.push(c);
        seen.add(c.caseData.id);
      }
    }
    return result;
  })();

  // 已结案 / 已调解的不在"重要日期"显示(用算好的 status 过滤,比 cases 原 workflow_status 准 — 后者可能为 null 走自动推断)
  const activeCases = casesSorted
    .filter(({ status }) => status.id !== "closed" && status.id !== "mediated")
    .map(({ caseData }) => caseData);
  const upcomingEvents = buildUpcomingEvents(activeCases);

  const handleChangeStatus = async (caseId: string, status: StatusId | null) => {
    setStatusOverride((m) => ({ ...m, [caseId]: status }));
    try {
      await updateWorkflowStatus(caseId, status);
    } catch (e) {
      console.warn("updateWorkflowStatus failed", e);
    }
  };

  // ===== 卡片拖拽排序(2026-05-26 V0.1.13)=====
  // PointerSensor 距离 5px 才激活,防止点卡片(打开案件)被误判为拖
  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 5 } }),
  );
  const sortedIds = casesSorted.map((c) => c.caseData.id);

  const handleDragEnd = async (event: DragEndEvent) => {
    const { active, over } = event;
    if (!over || active.id === over.id) return;
    const oldIdx = sortedIds.indexOf(String(active.id));
    const newIdx = sortedIds.indexOf(String(over.id));
    if (oldIdx === -1 || newIdx === -1) return;
    const newOrder = arrayMove(sortedIds, oldIdx, newIdx);
    setUserOrder(newOrder); // 乐观更新,UI 立刻变
    try {
      await updateHomeCaseOrder(newOrder);
    } catch (e) {
      console.warn("updateHomeCaseOrder failed", e);
    }
  };

  return (
    <main className="flex h-full w-full flex-col bg-background">
      {/* 顶部 nav */}
      <header className="border-b border-border bg-card/50 px-8 py-3">
        <div className="mx-auto flex max-w-6xl items-center">
          <h1 className="text-sm font-semibold tracking-tight text-foreground">
            案件看板
          </h1>
        </div>
      </header>

      {/* 主体 */}
      <div className="flex-1 overflow-auto">
        <div className="mx-auto max-w-6xl px-8 py-8">
          {/* Hero:问候 + 重要日期(2 列) */}
          <div className="mb-10 grid grid-cols-1 gap-6 md:grid-cols-2">
            <div>
              <p className="font-mono text-caption uppercase tracking-wider text-muted-foreground">
                OVERVIEW · {monthLabel}
              </p>
              <h1 className="mt-2 text-4xl font-semibold tracking-tight text-foreground">
                {greeting}
              </h1>
              <p className="mt-2 text-sm text-muted-foreground">
                你正在办 {cases.length} 个案件,扫一眼今天的进度。
              </p>
              <div className="mt-5 flex gap-2">
                <Button onClick={onImport} className="bg-foreground text-background hover:bg-foreground/90">
                  <FolderOpen className="size-3.5" />
                  导入案件文件夹
                </Button>
                <Button variant="outline" onClick={onBatchImport}>
                  <FolderSearch className="size-3.5" />
                  批量扫描目录
                </Button>
              </div>
            </div>

            {/* 重要日期 widget */}
            <ImportantDates events={upcomingEvents} onPickCase={onPickCase} />
          </div>

          {/* 月历板 */}
          <div className="mb-8">
            <CalendarBoard localEvents={upcomingEvents} onPickCase={onPickCase} onImportFolder={onImportFolder} />
          </div>

          {/* 在办案件 - 卡片网格 */}
          <div>
            <div className="mb-4 flex items-baseline gap-3">
              <h2 className="text-lg font-semibold tracking-tight">在办案件</h2>
              <span className="font-mono text-caption uppercase tracking-wider text-muted-foreground">
                {cases.length} CASES
              </span>
            </div>

            {cases.length === 0 ? (
              <EmptyCases onImport={onImport} />
            ) : (
              <DndContext
                sensors={sensors}
                collisionDetection={closestCenter}
                onDragEnd={handleDragEnd}
              >
                <SortableContext items={sortedIds} strategy={rectSortingStrategy}>
                  <div className="grid grid-cols-1 gap-4 md:grid-cols-2">
                    {casesSorted.map(({ caseData, status }) => (
                      <SortableCaseCard
                        key={caseData.id}
                        caseData={caseData}
                        status={status}
                        onClick={() => onPickCase(caseData.id)}
                        onChangeStatus={(s) => handleChangeStatus(caseData.id, s)}
                      />
                    ))}
                  </div>
                </SortableContext>
              </DndContext>
            )}
          </div>
        </div>
      </div>
    </main>
  );
}

/* ============ 案件卡片 ============ */

import { type StatusDef } from "@/modules/litigation/lib/inferStatus";

/**
 * SortableCaseCard — useSortable 包装的案件卡片(2026-05-26 V0.1.13)。
 *
 * 把 sortable transform/transition 挂在外层 div,dragHandle(GripVertical 按钮)
 * 单独接 listeners — 这样按拖把手才能拖,点卡片其他部分(标题 / 状态 chip /
 * "打开"区域)能正常点击进详情页。
 */
function SortableCaseCard(props: {
  caseData: Case;
  status: StatusDef;
  onClick: () => void;
  onChangeStatus: (s: StatusId | null) => void;
}) {
  const {
    attributes,
    listeners,
    setNodeRef,
    transform,
    transition,
    isDragging,
  } = useSortable({ id: props.caseData.id });
  const style: React.CSSProperties = {
    transform: CSS.Transform.toString(transform),
    transition,
    opacity: isDragging ? 0.5 : 1,
    zIndex: isDragging ? 10 : undefined,
  };
  return (
    <div ref={setNodeRef} style={style}>
      <CaseCard
        {...props}
        dragHandleProps={{ attributes, listeners }}
        isDragging={isDragging}
      />
    </div>
  );
}

function CaseCard({
  caseData,
  status,
  onClick,
  onChangeStatus,
  dragHandleProps,
  isDragging,
}: {
  caseData: Case;
  status: StatusDef;
  onClick: () => void;
  onChangeStatus: (s: StatusId | null) => void;
  /** 由 SortableCaseCard 注入:GripVertical 拖把手套 listeners + attributes */
  dragHandleProps?: {
    attributes: ReturnType<typeof useSortable>["attributes"];
    listeners: ReturnType<typeof useSortable>["listeners"];
  };
  /** 拖动中:虚线边框表示「被拿起」 */
  isDragging?: boolean;
}) {
  const plaintiffs = parseJsonArray(caseData.agg_plaintiffs);
  const defendants = parseJsonArray(caseData.agg_defendants);
  const judges = parseJsonArray(caseData.agg_judges);
  const isClosed = status.id === "closed";

  // 当事人对峙简写
  const partySummary = (() => {
    const left = plaintiffs[0] || "—";
    const right = defendants[0] || "—";
    const leftMore = plaintiffs.length > 1 ? `等${plaintiffs.length}人` : "";
    const rightMore = defendants.length > 1 ? `等${defendants.length}人` : "";
    return `${left}${leftMore} vs ${right}${rightMore}`;
  })();

  const amountText = caseData.agg_claim_amount
    ? formatYuan(caseData.agg_claim_amount)
    : null;

  return (
    <div
      className={cn(
        "group relative flex cursor-pointer flex-col rounded-xl border border-border bg-card p-5 text-left shadow-sm transition-all hover:border-foreground/30 hover:bg-foreground/[0.025] hover:shadow-lg",
        isDragging && "border-dashed",
        isClosed && "opacity-60",
      )}
      onClick={onClick}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          onClick();
        }
      }}
      role="button"
      tabIndex={0}
      aria-label={`打开案件 ${caseData.agg_cause || caseData.name}`}
    >
      {/* 左上角拖把手 — hover 才显色;按住才能拖,点卡片其他位置正常打开案件 */}
      {dragHandleProps && (
        <button
          type="button"
          aria-label="拖动调整顺序"
          title="按住拖动调整卡片顺序"
          onClick={(e) => e.stopPropagation()}
          className="absolute left-1.5 top-1.5 cursor-grab touch-none rounded p-1 text-muted-foreground/30 opacity-20 transition-all hover:bg-accent hover:text-foreground group-hover:opacity-100 active:cursor-grabbing"
          {...dragHandleProps.attributes}
          {...dragHandleProps.listeners}
        >
          <GripVertical className="size-3.5" />
        </button>
      )}

      {/* 状态 chip(右上,点开下拉手工选)— stopPropagation 防止冒泡触发卡片 onClick */}
      <StatusPicker
        status={status}
        isManual={caseData.workflow_status != null}
        onPick={onChangeStatus}
      />

      {/* 案由(标题) */}
      <h3 className="pr-16 text-lg font-semibold leading-tight text-foreground">
        {caseData.source_folder === "__DEMO__" && (
          <span className="mr-2 inline-flex items-center rounded bg-amber-100 px-1.5 py-0.5 text-caption font-medium text-amber-800 align-middle dark:bg-amber-900/40 dark:text-amber-200">
            📌 示例
          </span>
        )}
        {caseData.agg_cause || caseData.name}
      </h3>

      {/* 当事人 */}
      <p className="mt-1 text-sm text-muted-foreground">{partySummary}</p>

      {/* 详细字段(2 列) */}
      <dl className="mt-4 grid grid-cols-2 gap-x-4 gap-y-2 text-xs">
        <Item label="案号" value={caseData.agg_case_no} mono />
        <Item label="法院" value={caseData.agg_court} />
        <Item label="承办法官" value={judges.length > 0 ? judges.join("、") : null} />
        <Item label="诉讼金额" value={amountText} mono highlight />
      </dl>

      {/* 底部指示 */}
      <div className="mt-4 flex items-center justify-between border-t border-border pt-3 text-caption text-muted-foreground">
        <span className="font-mono">
          {caseData.agg_computed_at ? "已抽取" : "抽取中…"}
        </span>
        <span className="inline-flex items-center gap-0.5 text-foreground/60 transition-colors group-hover:text-foreground">
          打开 <ChevronRight className="size-3" />
        </span>
      </div>
    </div>
  );
}

/* ============ 状态选择器(右上角 chip + 下拉) ============ */
function StatusPicker({
  status,
  isManual,
  onPick,
}: {
  status: StatusDef;
  isManual: boolean;
  onPick: (s: StatusId | null) => void;
}) {
  const [open, setOpen] = useState(false);

  useEffect(() => {
    if (!open) return;
    const onClick = () => setOpen(false);
    window.addEventListener("click", onClick);
    return () => window.removeEventListener("click", onClick);
  }, [open]);

  return (
    <div
      // 展开时抬高整个容器的层叠上下文:否则下拉(z-20)被封闭在父级 z-10 上下文内,
      // 会被 DOM 中靠后的同 z-10 兄弟卡片(及其状态徽章)盖住。
      className={cn("absolute right-3 top-3", open ? "z-50" : "z-10")}
      onClick={(e) => e.stopPropagation()}
      onKeyDown={(e) => e.stopPropagation()}
    >
      <button
        type="button"
        onClick={(e) => {
          e.stopPropagation();
          setOpen((v) => !v);
        }}
        className={cn(
          "inline-flex items-center gap-1 rounded-full px-2.5 py-0.5 text-caption font-medium transition-opacity hover:opacity-80",
          status.color,
        )}
        title={isManual ? "手工设置 · 点击修改" : "自动推断 · 点击手工选择"}
      >
        {status.label}
        <ChevronDown className="size-3 opacity-70" />
      </button>

      {open && (
        <div className="absolute right-0 top-full z-20 mt-1 w-32 overflow-hidden rounded-md border border-border bg-card shadow-lg">
          {STATUS_LIST.map((s) => (
            <button
              key={s.id}
              type="button"
              onClick={(e) => {
                e.stopPropagation();
                onPick(s.id);
                setOpen(false);
              }}
              className="flex w-full items-center justify-between px-3 py-1.5 text-left text-xs hover:bg-accent"
            >
              <span className="flex items-center gap-1.5">
                <span
                  className={cn(
                    "inline-block size-2 rounded-full",
                    s.color.split(" ")[0],
                  )}
                />
                {s.label}
              </span>
              {s.id === status.id && <Check className="size-3 text-foreground" />}
            </button>
          ))}
          {isManual && (
            <>
              <div className="border-t border-border" />
              <button
                type="button"
                onClick={(e) => {
                  e.stopPropagation();
                  onPick(null);
                  setOpen(false);
                }}
                className="block w-full px-3 py-1.5 text-left text-label text-muted-foreground hover:bg-accent hover:text-foreground"
                title="清除手工设置,恢复自动推断"
              >
                ↺ 恢复自动推断
              </button>
            </>
          )}
        </div>
      )}
    </div>
  );
}

function Item({
  label,
  value,
  mono = false,
  highlight = false,
}: {
  label: string;
  value: string | null;
  mono?: boolean;
  highlight?: boolean;
}) {
  return (
    <div>
      <dt className="text-caption uppercase tracking-wider text-muted-foreground">
        {label}
      </dt>
      <dd
        className={cn(
          "mt-0.5 truncate text-foreground",
          mono && "font-mono",
          highlight && value && "font-semibold"
        )}
      >
        {value || <span className="text-muted-foreground/40">—</span>}
      </dd>
    </div>
  );
}

/* ============ 重要日期 widget ============ */

/**
 * 两类事件:
 * - hearing(开庭):取 agg_key_dates 里 event ∈ {开庭, 二审开庭} 的 date,只看未来
 *   开完的庭不再倒计时(无意义)。
 * - deadline(到期):取 agg_key_dates 里有 expires_at 的事件(保全/续封/上诉期/
 *   还款期等),过期 30 天内仍然红色显示(没续封是要紧事)。
 *
 * 排序:hearing 优先 → 距今天数升序。最多 8 条。
 */
type EventKind = "hearing" | "deadline";

export interface UpcomingEvent {
  kind: EventKind;
  date: string; // YYYY-MM-DD
  daysFromNow: number;
  type: string; // 开庭 / 续封 / 还款期 ...
  note?: string | null; // LLM 抽的备注,如"第二次开庭" / "庭前会议"
  caseName: string;
  caseId: string;
  court?: string | null;
}

// 保全 / 续封 / 查封类到期(用 ShieldAlert 图标 + "需提前续封"提示,区别于一般到期)
const PRESERVATION_RE = /保全|续封|查封|冻结/;

/** 紧急度分级(决定放大 vs 缩小 + 配色):
 *  overdue = 已过期(续封超期最要命) / urgent = 开庭≤30天 或 续封类到期≤90天 / normal = 其余常规显示。 */
function eventUrgency(e: UpcomingEvent): "overdue" | "urgent" | "normal" {
  if (e.daysFromNow < 0) return "overdue";
  if (e.kind === "hearing") return e.daysFromNow <= 30 ? "urgent" : "normal";
  // deadline(续封 / 查封到期 / 还款期等):≤90 天就提醒(老板:续封不足90天单独醒目提醒)
  return e.daysFromNow <= 90 ? "urgent" : "normal";
}

function ImportantDates({
  events,
  onPickCase,
}: {
  events: UpcomingEvent[];
  onPickCase: (caseId: string) => void;
}) {
  // 主次分明:紧急(开庭≤30 / 续封≤90 / 超期)放大成卡片,其余缩成一行
  const prominent = events.filter((e) => eventUrgency(e) !== "normal");
  const later = events.filter((e) => eventUrgency(e) === "normal");
  return (
    <div className="rounded-xl border border-border bg-card p-5">
      <div className="mb-3 flex items-baseline justify-between">
        <h2 className="text-sm font-semibold tracking-tight">重要日期</h2>
        <span className="font-mono text-caption uppercase tracking-wider text-muted-foreground">
          {events.length} EVENTS
        </span>
      </div>
      {events.length === 0 ? (
        <div className="flex flex-col items-center justify-center py-8 text-center">
          <CalendarClock className="size-6 text-muted-foreground/40" />
          <p className="mt-2 text-xs text-muted-foreground">暂无近期事件</p>
          <p className="mt-1 text-caption text-muted-foreground/70">
            导入案件后,开庭日 / 保全续封会自动出现在这里
          </p>
        </div>
      ) : (
        <div className="space-y-3">
          {/* 紧急块:开庭≤30天 / 续封≤90天 / 已超期 —— 放大 + 倒计时醒目 */}
          {prominent.length > 0 && (
            <ul className="space-y-2">
              {prominent.map((e, i) => (
                <EventRow
                  key={`p${i}`}
                  e={e}
                  variant="prominent"
                  onPick={() => onPickCase(e.caseId)}
                />
              ))}
            </ul>
          )}
          {/* 常规块:其余开庭 / 较远到期 —— 缩成一行 */}
          {later.length > 0 && (
            <div>
              {prominent.length > 0 && (
                <p className="mb-1.5 mt-1 text-caption uppercase tracking-wider text-muted-foreground/50">
                  其他日程
                </p>
              )}
              <ul className="space-y-0.5">
                {later.map((e, i) => (
                  <EventRow
                    key={`l${i}`}
                    e={e}
                    variant="compact"
                    onPick={() => onPickCase(e.caseId)}
                  />
                ))}
              </ul>
            </div>
          )}
        </div>
      )}
    </div>
  );
}

function EventRow({
  e,
  variant,
  onPick,
}: {
  e: UpcomingEvent;
  variant: "prominent" | "compact";
  onPick: () => void;
}) {
  const urgency = eventUrgency(e);
  // 配色:超期 / ≤7天 红;其余紧急 橙;常规 灰
  const tone =
    urgency === "overdue" || (urgency === "urgent" && e.daysFromNow <= 7)
      ? "red"
      : urgency === "urgent"
        ? "amber"
        : "muted";
  const isPreserv = e.kind === "deadline" && PRESERVATION_RE.test(e.type);
  const Icon = e.kind === "hearing" ? Gavel : isPreserv ? ShieldAlert : AlertTriangle;

  // 倒计时:今天 D-DAY,未来 D-N,已过期「逾期N天」
  const countdown =
    e.daysFromNow === 0
      ? "D-DAY"
      : e.daysFromNow > 0
        ? `D-${e.daysFromNow}`
        : `逾期${-e.daysFromNow}天`;

  if (variant === "compact") {
    const cdCls =
      tone === "red"
        ? "text-red-700 dark:text-red-300"
        : tone === "amber"
          ? "text-amber-700 dark:text-amber-300"
          : "text-muted-foreground";
    return (
      <li>
        <button
          type="button"
          onClick={onPick}
          className="flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left transition-colors hover:bg-muted/50"
          title={`打开案件 · ${e.caseName}`}
        >
          <Icon className="size-3 shrink-0 text-muted-foreground/60" />
          <span className={`shrink-0 font-mono text-caption font-medium ${cdCls}`}>
            {countdown}
          </span>
          <span className="shrink-0 text-xs text-foreground">{e.type}</span>
          <span className="truncate text-caption text-muted-foreground">
            · {e.caseName}
          </span>
        </button>
      </li>
    );
  }

  // prominent —— 紧急事件放大成卡片
  const box = {
    red: "bg-red-50 ring-1 ring-red-300/60 dark:bg-red-950/30 dark:ring-red-700/40",
    amber: "bg-amber-50 ring-1 ring-amber-300/60 dark:bg-amber-950/30 dark:ring-amber-700/40",
    muted: "bg-muted/40",
  }[tone];
  const cdCls = {
    red: "text-red-700 dark:text-red-300",
    amber: "text-amber-800 dark:text-amber-300",
    muted: "text-muted-foreground",
  }[tone];
  const iconCls = {
    red: "text-red-600 dark:text-red-400",
    amber: "text-amber-600 dark:text-amber-400",
    muted: "text-foreground/60",
  }[tone];
  // 续封专属提示(超期 / 临近都点出来,这是执行律师最容易漏的)
  const hint =
    urgency === "overdue"
      ? isPreserv
        ? "已超期,尽快续封"
        : "已逾期"
      : urgency === "urgent" && isPreserv
        ? "需提前申请续封"
        : null;
  const hintCls =
    tone === "red"
      ? "bg-red-100 text-red-700 dark:bg-red-900/40 dark:text-red-300"
      : "bg-amber-100 text-amber-800 dark:bg-amber-900/40 dark:text-amber-300";

  return (
    <li>
      <button
        type="button"
        onClick={onPick}
        className={`flex w-full items-center gap-3.5 rounded-lg px-3.5 py-3 text-left transition-colors hover:brightness-95 dark:hover:brightness-110 ${box}`}
        title={`打开案件 · ${e.caseName}`}
      >
        <div className="shrink-0 text-center">
          <div className={`font-mono text-xl font-bold leading-none ${cdCls}`}>
            {countdown}
          </div>
          <div className="mt-1 font-mono text-caption text-muted-foreground">
            {e.date.slice(5)}
          </div>
        </div>
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-1.5">
            <Icon className={`size-3.5 shrink-0 ${iconCls}`} />
            <span className="text-sm font-semibold text-foreground">{e.type}</span>
            {hint && (
              <span className={`rounded px-1.5 py-0.5 text-caption font-medium ${hintCls}`}>
                {hint}
              </span>
            )}
          </div>
          {e.note && (
            <p className="mt-0.5 truncate text-xs text-muted-foreground">{e.note}</p>
          )}
          <p className="mt-0.5 truncate text-xs text-foreground/80">{e.caseName}</p>
          {e.court && (
            <p className="mt-0.5 truncate text-caption text-muted-foreground/70">
              {e.court}
            </p>
          )}
        </div>
      </button>
    </li>
  );
}

function buildUpcomingEvents(cases: Case[]): UpcomingEvent[] {
  // 两类事件合一(2026-05-26 V0.1.15):
  //   1) 开庭(event ∈ {开庭, 二审开庭}, 用 date 字段)— 过去的庭不显示
  //   2) 到期(任意 event,有 expires_at)— 续封/上诉期/还款期等,过期 30 天内仍显示
  // 同案件可能 (开庭 + 还款期)各自抽到,都列出。
  const events: UpcomingEvent[] = [];
  const now = new Date();
  now.setHours(0, 0, 0, 0);

  for (const c of cases) {
    const kdJson = c.agg_key_dates;
    if (!kdJson) continue;
    let arr: Array<{
      date?: string;
      event?: string;
      note?: string;
      expires_at?: string;
    }>;
    try {
      const parsed = JSON.parse(kdJson);
      arr = Array.isArray(parsed) ? parsed : [];
    } catch {
      continue;
    }
    const caseName = c.agg_cause || c.name;

    // 开庭:时间从法院传票 PDF 抽取(作者把传票放进案件原始文件夹 → 系统抽到开庭时间)。
    // 规则(作者):① 只显示**未来**的开庭(过去的传票=已开过,不显示)② 一个案件同一时间
    // 只有**一个最新的未来开庭**(开完没审清才会通知下一次)→ 每案只取最近的那个。
    // 匹配放宽到 event **含「开庭」**(传票措辞多样:开庭/二审开庭/第一次开庭/开庭传票…)。
    let nearestHearing: UpcomingEvent | null = null;

    for (const kd of arr) {
      // (1) 开庭:date 字段,只取未来最近的一个
      if (kd.event && kd.event.includes("开庭") && kd.date) {
        const d = new Date(kd.date);
        if (!isNaN(d.getTime())) {
          const daysFromNow = Math.round((d.getTime() - now.getTime()) / 86400000);
          if (daysFromNow >= 0 && daysFromNow <= 365) {
            if (!nearestHearing || daysFromNow < nearestHearing.daysFromNow) {
              nearestHearing = {
                kind: "hearing",
                date: kd.date,
                daysFromNow,
                type: kd.event,
                note: kd.note ?? null,
                caseName,
                caseId: c.id,
                court: c.agg_court,
              };
            }
          }
        }
      }

      // (2) 到期:expires_at 字段(续封 / 查封到期 / 还款期等),超期 30 天内 ~ 未来一年。
      // 续封类 ≤90 天会被放大成醒目提醒,>90 天常规显示(已超期 30 天以上不再提醒)。
      if (kd.expires_at) {
        const d = new Date(kd.expires_at);
        if (!isNaN(d.getTime())) {
          const daysFromNow = Math.round((d.getTime() - now.getTime()) / 86400000);
          if (daysFromNow >= -30 && daysFromNow <= 365) {
            events.push({
              kind: "deadline",
              date: kd.expires_at,
              daysFromNow,
              type: kd.event ?? "到期",
              note: kd.note ?? null,
              caseName,
              caseId: c.id,
              court: c.agg_court,
            });
          }
        }
      }
    }
    // 每案只放最近的那个未来开庭(到期事件不去重:续封 + 还款期可并存)
    if (nearestHearing) events.push(nearestHearing);
  }

  // 排序规则:① 按紧急度分组(超期 → 紧急 → 常规),让 ImportantDates 能把前两组放大
  // ② 组内距今天数升序(越近越靠前;超期组里越久越靠前)③ 同天 hearing 优先。
  const rank = { overdue: 0, urgent: 1, normal: 2 } as const;
  return events
    .sort((a, b) => {
      const ra = rank[eventUrgency(a)];
      const rb = rank[eventUrgency(b)];
      if (ra !== rb) return ra - rb;
      if (a.daysFromNow !== b.daysFromNow) return a.daysFromNow - b.daysFromNow;
      if (a.kind !== b.kind) return a.kind === "hearing" ? -1 : 1;
      return 0;
    })
    .slice(0, 12);
}

/* ============ 工具 ============ */

function getGreeting(name: string | null): string {
  const who = name && name.trim().length > 0 ? name.trim() : "律师";
  const h = new Date().getHours();
  if (h < 6) return `深夜好,${who}`;
  if (h < 12) return `上午好,${who}`;
  if (h < 14) return `中午好,${who}`;
  if (h < 18) return `下午好,${who}`;
  return `晚上好,${who}`;
}

function EmptyCases({ onImport }: { onImport: () => void }) {
  return (
    <div className="flex flex-col items-center justify-center rounded-xl border border-dashed border-border bg-card/30 px-6 py-16 text-center">
      <FolderOpen className="size-10 text-muted-foreground/40" />
      <p className="mt-4 text-base font-medium text-foreground">
        还没有导入任何案件
      </p>
      <p className="mt-1 text-sm text-muted-foreground">
        选择一个案件文件夹开始
      </p>
      <Button onClick={onImport} className="mt-6">
        <FolderOpen className="size-3.5" />
        导入案件文件夹
      </Button>
    </div>
  );
}
