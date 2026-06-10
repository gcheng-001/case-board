/**
 * 律师费计算 — 100% 移植自 lawtools.top/fee.html 的业务逻辑。
 *
 * 数值参考(温州同行常用):
 *   - 固定收费分档:1万 / 20万 / 50万 / 100万 / 250万 / 500万 / 1000万 / 1000万以上
 *   - 风险代理:总期望 = 一口价 × 1.8;按案件难度分基础费比例和风险费率
 */

/**
 * 一口价(固定收费)计算。
 * @param amount 标的额(单位:万元)
 * @returns 律师费(万元)
 */
export function calculateFixed(amount: number): number {
  if (amount <= 0) return 0;
  if (amount <= 1) return 1;
  if (amount <= 20) return Math.max(1, amount * 0.1);
  if (amount <= 50) return 2 + (amount - 20) * 0.05;
  if (amount <= 100) return 3.5 + (amount - 50) * 0.03;
  if (amount <= 250) return 5 + (amount - 100) * 0.04;
  if (amount <= 500) return 11 + (amount - 250) * 0.028;
  if (amount <= 1000) return 18 + (amount - 500) * 0.02;
  return 28 + (amount - 1000) * 0.015;
}

/** 风险代理总期望(万元) = 一口价 × 1.8 */
export function calculateRiskTotal(amount: number): number {
  return calculateFixed(amount) * 1.8;
}

export type Difficulty = "simple" | "normal" | "hard";

export interface DifficultyDef {
  /** 风险代理费率(%)— 实际打官司赢的部分按这个比例收 */
  riskRate: number;
  /** 期望回款率(%)— 简单案件预期 90% 能回款,困难只 20% */
  expectRate: number;
  /** 基础费 / 风险费 比例 [base%, risk%] */
  ratio: [number, number];
  label: string;
}

export const DIFFICULTY: Record<Difficulty, DifficultyDef> = {
  simple: { riskRate: 7, expectRate: 90, ratio: [30, 70], label: "简单案件" },
  normal: { riskRate: 10, expectRate: 60, ratio: [40, 60], label: "一般案件" },
  hard: { riskRate: 15, expectRate: 20, ratio: [50, 50], label: "困难案件" },
};

/**
 * 风险代理详细计算结果。
 * @param amount 标的额(万元)
 * @param difficulty 案件难度
 */
export function calculateRiskBreakdown(
  amount: number,
  difficulty: Difficulty,
): {
  total: number;
  baseFee: number;
  expectAmount: number;
  actualRiskFee: number;
  def: DifficultyDef;
} {
  const d = DIFFICULTY[difficulty];
  const totalExpected = calculateRiskTotal(amount);
  const baseFee = totalExpected * (d.ratio[0] / 100);
  const expectAmount = amount * (d.expectRate / 100);
  const actualRiskFee = expectAmount * (d.riskRate / 100);
  return {
    total: baseFee + actualRiskFee,
    baseFee,
    expectAmount,
    actualRiskFee,
    def: d,
  };
}

/**
 * 金额格式化(万元 / 亿元自适应)。
 * 用 0.5 万元精度 round(避免显示 12.347 这种)
 */
export function formatMoneyWan(num: number): string {
  const rounded = Math.round(num * 2) / 2;
  if (rounded >= 10000) return (rounded / 10000).toFixed(2) + " 亿元";
  if (rounded >= 100) return Math.round(rounded) + " 万元";
  if (rounded >= 1) return rounded.toFixed(1) + " 万元";
  return rounded.toFixed(2) + " 万元";
}
