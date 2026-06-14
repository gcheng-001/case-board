//! V0.2 D4-D5.B · 模型路由(V0.3 重构:统一到 `settings.cloud_llm_model` 单一字段)。
//!
//! 把 `(TaskType, user_message, Settings)` 映射到具体模型 + 温度 + max_tokens。
//! 模型名和 max_output_tokens 从 `providers::preset_for(settings)` 取,不再硬编码。
//!
//! **用户在设置里只有一个选择 `cloud_llm_model`(= 三档「模型档位」)**:
//!   - 快档(默认)= 全局走快速模型(便宜)。
//!   - 强档 = 全局走强力模型(更准更贵)。
//!   - `"auto"` = 自动挡:简单任务走快档、复杂任务走强档(下面的 task 路由表)。
//!
//! 关键:**非 auto 档绝不"偷偷"把某个任务升到强档**。
//! 自动挡(auto)下才按任务复杂度分流:
//!   - 4 个工具/分析型(法律依据/类案/校验/模拟对抗) → 强档
//!   - FreeChat → 启发式:短问/无 reasoning 关键词 = 快档,否则 强档

use serde::Serialize;

use super::context::TaskType;
use crate::llm::providers::{self, ProviderPreset};
use crate::settings::Settings;

/// 路由结果。给 agent_loop / commands 用,代替原来硬编码的 temperature / max_tokens。
#[derive(Debug, Clone, Serialize)]
pub struct ModelChoice {
    pub model: String,
    pub temperature: f32,
    pub max_tokens: u32,
}

impl ModelChoice {
    /// 快档模型(按提供商预设取模型名和 max_tokens)。
    pub fn flash(preset: &ProviderPreset) -> Self {
        Self {
            model: preset.flash_model.to_string(),
            temperature: 0.3,
            max_tokens: preset.max_output_tokens,
        }
    }

    /// 强档模型。`with_reasoning=true` 且提供商有 thinking 模型时切到 thinking 模型。
    pub fn pro(preset: &ProviderPreset, with_reasoning: bool) -> Self {
        let model = if with_reasoning {
            preset
                .thinking_model
                .unwrap_or(preset.pro_model)
                .to_string()
        } else {
            preset.pro_model.to_string()
        };
        Self {
            model,
            temperature: 0.15,
            max_tokens: preset.max_output_tokens,
        }
    }

    /// 把用户在 Settings 强制选定的 model 字符串包装成 ModelChoice。
    pub fn from_forced(model: &str, preset: &ProviderPreset) -> Self {
        let is_pro = model.contains("pro");
        Self {
            model: model.to_string(),
            temperature: if is_pro { 0.15 } else { 0.3 },
            max_tokens: preset.max_output_tokens,
        }
    }
}

/// 路由主入口。统一读 `settings.cloud_llm_model` 这一个「模型档位」字段。
pub fn route_model(task: TaskType, user_message: &str, settings: &Settings) -> ModelChoice {
    let preset = providers::preset_for(settings);
    let mode = settings
        .cloud_llm_model
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(preset.flash_model);

    if mode != "auto" {
        return ModelChoice::from_forced(mode, preset);
    }

    match task {
        TaskType::CompileLegalBasis
        | TaskType::FindSimilarCases
        | TaskType::VerifyMyDraft
        | TaskType::SimulateOpposition
        | TaskType::DeepAnalysis => ModelChoice::pro(preset, false),
        TaskType::FreeChat => route_free_chat(user_message, preset),
    }
}

/// 启发式:短问(<30 字)或不带"推理类"关键词 → 快档;否则强档。
fn route_free_chat(msg: &str, preset: &ProviderPreset) -> ModelChoice {
    let chars = msg.chars().count();
    if chars < 30 {
        return ModelChoice::flash(preset);
    }
    const REASONING_KEYWORDS: &[&str] = &[
        "建议",
        "分析",
        "为什么",
        "怎么办",
        "如何",
        "拒执",
        "风险",
        "怎么处理",
        "策略",
        "对比",
        "评估",
        "推理",
    ];
    if REASONING_KEYWORDS.iter().any(|k| msg.contains(k)) {
        ModelChoice::pro(preset, false)
    } else {
        ModelChoice::flash(preset)
    }
}
