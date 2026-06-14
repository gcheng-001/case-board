//! agent_loop 的 4 条 cap(V0.2 D3-D4.B,详 § 6.5)。
//!
//! 在 chat agent 多轮工具调用循环里,防止"无限调"、"反复调同一个工具"、"调用堆积太久"、
//! "thinking 模型 reasoning token 爆炸"四种失控情况。
//!
//! 每轮 LLM 请求前调 `check_iter_cap` + `check_duration_cap`;每次准备发起 tool 调用前调
//! `check_duplicate_tool_call`;LLM 返回 usage 后调 `add_reasoning_tokens`。
//!
//! 任何一个 cap 触发 → 返回 `LoopGuardViolation`,agent_loop 终止本轮并把信息塞回 LLM 让它
//! 收尾(或者直接 abort,看上层策略)。

use std::collections::HashSet;
use std::time::{Duration, Instant};

use serde::Serialize;
use thiserror::Error;

/// 4 条 cap 中触发哪一条。
#[derive(Debug, Clone, Serialize, Error)]
pub enum LoopGuardViolation {
    #[error("超过本会话最大轮数(max={max})")]
    IterCapExceeded { max: u32 },
    #[error("LLM 反复调同一工具 + 同参数:tool={tool},循环模式拦下")]
    DuplicateToolCall { tool: String },
    #[error("本会话总耗时超 {limit_secs}s,可能后端慢或卡死,提前 abort")]
    DurationCapExceeded { limit_secs: u64 },
    #[error("reasoning token 累计超 {limit},thinking 模型可能跑飞")]
    ReasoningTokenCapExceeded { limit: u64 },
}

pub struct LoopGuard {
    iter_count: u32,
    max_iters: u32,
    seen_tool_args: HashSet<(String, String)>,
    started_at: Instant,
    max_duration: Duration,
    reasoning_tokens: u64,
    max_reasoning_tokens: u64,
}

impl LoopGuard {
    /// 用 settings 配置 4 条 cap。settings 字段为 None 时用默认值。
    pub fn from_settings(s: &crate::settings::Settings) -> Self {
        Self {
            iter_count: 0,
            // 复杂法律任务(法规+案例+企业+校验+综合)轮数偏多,默认放到 16
            //(2026-05-31 从 12 上调:执行案法律依据曾贴满 12 轮靠 force-finish 收尾,可能漏法条)
            max_iters: s.chat_loop_max_iters.unwrap_or(16),
            seen_tool_args: HashSet::new(),
            started_at: Instant::now(),
            // 思考模型 + 多轮工具 + 写长答案,120s 墙钟太紧;放到 300s。
            // 真跑飞仍有 max_iters / max_reasoning_tokens 双重兜底。
            max_duration: Duration::from_secs(300),
            reasoning_tokens: 0,
            // thinking 模型每轮 reasoning 几千 token,多轮累积易超 8000;放到 64000。
            max_reasoning_tokens: 64_000,
        }
    }

    /// 完全用默认值(单测用)。

    /// 自定义 4 条 cap(单测用)。

    pub fn iter_count(&self) -> u32 {
        self.iter_count
    }

    /// 进入新一轮(发请求前调)。失败返回 `IterCapExceeded`。
    pub fn check_iter_cap(&mut self) -> Result<(), LoopGuardViolation> {
        if self.iter_count >= self.max_iters {
            return Err(LoopGuardViolation::IterCapExceeded {
                max: self.max_iters,
            });
        }
        self.iter_count += 1;
        Ok(())
    }

    /// 派发工具前调:同一 tool + 同参数 hash 之前调过就拒绝(防 LLM 死循环)。
    /// `args` 用 canonical JSON(local_kb::hash::query_hash 不带 prefix)做 dedupe key。
    pub fn check_duplicate_tool_call(
        &mut self,
        tool: &str,
        args: &serde_json::Value,
    ) -> Result<(), LoopGuardViolation> {
        // 用同种 canonical 算法跟 KB cache 对齐,sort_keys + ensure_ascii=False
        let canonical = crate::local_kb::hash::query_hash("", args);
        let key = (tool.to_string(), canonical);
        if !self.seen_tool_args.insert(key) {
            return Err(LoopGuardViolation::DuplicateToolCall {
                tool: tool.to_string(),
            });
        }
        Ok(())
    }

    /// 每次工具调用 / LLM 调用后调一次。超过 2 分钟就 abort。
    pub fn check_duration_cap(&self) -> Result<(), LoopGuardViolation> {
        if self.started_at.elapsed() > self.max_duration {
            return Err(LoopGuardViolation::DurationCapExceeded {
                limit_secs: self.max_duration.as_secs(),
            });
        }
        Ok(())
    }

    /// LLM 返回 usage 时累计 reasoning_tokens(thinking 模型 usage.reasoning_tokens)。
    pub fn add_reasoning_tokens(&mut self, n: u64) -> Result<(), LoopGuardViolation> {
        self.reasoning_tokens = self.reasoning_tokens.saturating_add(n);
        if self.reasoning_tokens > self.max_reasoning_tokens {
            return Err(LoopGuardViolation::ReasoningTokenCapExceeded {
                limit: self.max_reasoning_tokens,
            });
        }
        Ok(())
    }
}
