// 私人专属功能注册接缝(双轨发布模型)。
// 开源版:返回 [] 的桩 —— 共享代码(ModuleTabs / App)只依赖本接缝,不直接 import 任何私人实现,
// 故开源仓没有 dokuritsu/* 也能编译。
import type { ComponentType, ReactNode } from "react";

export interface PrivateTopTab {
  id: string;
  label: string;
  icon: ComponentType<{ className?: string }>;
  render: () => ReactNode;
}

export function getPrivateTopTabs(): PrivateTopTab[] {
  return [];
}
