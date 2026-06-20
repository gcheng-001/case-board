/**
 * 案件「领域」归类:把一个案件判为刑事 / 非刑事(民事·诉讼)。
 *
 * 背景:刑事 tab(复刻诉讼看板框架)只显示刑事案件,诉讼 tab 排除刑事案件。
 * 导入 / OCR / 全局抽取 / 数据层全部共享 —— 案件先导进来,再用本启发式归类。
 *
 * 设计取舍(2026-06-17):
 *   - **纯前端启发式,零 migration、不碰 LLM 全局抽取 schema**(避开已知坑 #14/#20)。
 *   - 命中即判刑事:案号含「刑」(刑初/刑终/刑核…)/ 案由含「罪」/ 出现刑事**专属**词
 *     (起诉书·公诉·被告人·犯罪嫌疑人·逮捕·刑事拘留·取保候审·看守所·量刑·缓刑·羁押·认罪认罚)。
 *   - **关键风险方向是「民事/行政案件被误吞进刑事 tab、从诉讼 tab 消失」**(不是漏判刑事)——
 *     因手动「移出」开关尚未做,误吞无法救回。故关键词只收**刑事专属**词:刻意排除「检察院 /
 *     公安局 / 公安机关 / 侦查」——这些在交通事故、行政诉讼(告公安局)、国家赔偿等民事/行政
 *     案件里也常出现,留着会误吞。「被告人」「逮捕」「量刑」等是刑事专属(民事用「被告」、无「逮捕/量刑」)。
 *   - 真实刑事案件几乎都有罪名(案由含「罪」)或「刑」字案号,主信号足够;关键词只作次要兜底。
 *   - **手动「标记为刑事 / 移出」开关是挂起项**(见 docs/刑事标签页-实施进度.md),需新 migration
 *     列或 settings 字段,后续再加;在此之前归类不可由用户纠正,故宁可漏判勿误吞。
 */

import type { Case } from "@/lib/types";

/**
 * 刑事**专属**关键词(出现在案由 / 状态文字 / 摘要里基本可断定刑事)。
 * 刻意不含「检察院 / 公安局 / 公安机关 / 侦查」—— 那些在民事/行政案件里也常见,会误吞(见文件头注释)。
 */
const CRIMINAL_KEYWORDS = [
  "犯罪嫌疑人",
  "被告人", // 刑事专属;民事用「被告」(无「人」)
  "公诉",
  "起诉书",
  "逮捕", // 刑事专属;行政是「行政拘留」、民事无
  "刑事拘留",
  "取保候审",
  "看守所",
  "量刑",
  "缓刑",
  "羁押",
  "认罪认罚",
];

/** 把若干可能为空的字段拼成一个待检索字符串(含案件/文件夹名,便于导入即时判定)。 */
function haystack(c: Case): string {
  return [
    c.name,
    c.cause,
    c.agg_cause,
    c.court,
    c.agg_court,
    c.agg_court_type,
    c.agg_status_text,
    c.case_summary,
    c.workflow_status,
  ]
    .filter(Boolean)
    .join(" ");
}

/**
 * 判断一个案件是否为刑事案件(启发式)。
 *
 * 命中任一信号即判刑事:
 *   1. 案号含「刑」字(刑初 / 刑终 / 刑核 / 刑申 …)—— 最强信号;
 *   2. 案由(cause / agg_cause)含「罪」字 —— 罪名几乎只出现在刑事;
 *   3. 任一拼接字段命中刑事专属关键词。
 */
export function isCriminalCase(c: Case): boolean {
  // 案号或案件/文件夹名含「刑」(刑初/刑终…)—— 最强信号,导入瞬间(agg 字段未抽出)即可判。
  const caseNo = `${c.case_no ?? ""} ${c.agg_case_no ?? ""} ${c.name ?? ""}`;
  if (caseNo.includes("刑")) return true;

  const cause = `${c.cause ?? ""} ${c.agg_cause ?? ""} ${c.name ?? ""}`;
  if (cause.includes("罪")) return true;

  const hay = haystack(c);
  return CRIMINAL_KEYWORDS.some((kw) => hay.includes(kw));
}

/** 拆分案件列表为 { civil, criminal } 两组(刑事 tab / 诉讼 tab 各取一组)。 */
export function splitCasesByDomain(cases: Case[]): {
  civil: Case[];
  criminal: Case[];
} {
  const civil: Case[] = [];
  const criminal: Case[] = [];
  for (const c of cases) {
    (isCriminalCase(c) ? criminal : civil).push(c);
  }
  return { civil, criminal };
}
