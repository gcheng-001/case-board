//! 前缀缓存稳定性观测(被动诊断,不改任何发出的请求)。
//!
//! DeepSeek 的自动前缀缓存只在请求的**字节前缀**与上一次完全一致时命中;system prompt
//! 漂移、工具列表重排、历史被改写,都会让"变化字节之后"的所有 token 全价重算(详
//! `docs/DeepSeek-思考模式与工具调用-官方说明.md` §2)。本模块把"不可变前缀"
//! (system prompt + 工具 schema)指纹化,用来**离线看出**哪一轮把缓存打破了。
//!
//! 三区模型:
//! ```text
//!   ┌─ 不可变前缀:system + tool schemas   ← 缓存命中候选,应每轮稳定
//!   ├─ append-only 历史:assistant/tool 轮  ← 单调增长,保留旧轮前缀
//!   └─ 最新 user 轮                          ← 每次请求唯一的新内容
//! ```
//!
//! 移植自 CodeWhale(<https://github.com/Hmbown/CodeWhale>,MIT,
//! `crates/tui/src/prefix_cache.rs`)。Copyright (c) 2024-2025 DeepSeek CLI Contributors,MIT。
//! 两处适配:① 哈希从 SHA-256 换成本仓已有的 `md5`(零新依赖;这是漂移**诊断**指纹,
//! 非安全场景,碰撞抗性无关紧要);② 工具类型从 CodeWhale 的 `models::Tool` 换成本仓
//! 实际发给 API 的 `serde_json::Value`(已是纯 API JSON,无内部字段需剥离)。
//!
//! **隐私**:指纹是单向哈希,只反映"两个前缀是否相同",不含也不可还原案件内容。

use serde_json::Value;

/// 不可变前缀的指纹快照。`combined` 相同 ⇒ 序列化出的字节前缀相同。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrefixFingerprint {
    /// system prompt 文本的 md5。
    pub system: String,
    /// 工具目录 JSON(名/描述/schema)的 md5。
    pub tools: String,
    /// `system:tools` 组合的 md5。
    pub combined: String,
}

impl PrefixFingerprint {
    /// 从 system prompt 文本 + 工具 schema 列表算指纹。
    ///
    /// 工具按其 API JSON 文本**字典序排序后**再哈希 —— 因此工具注册顺序变化不算漂移,
    /// 而描述/schema 的实质变化会被捕获。空工具列表等价于空串哈希。
    pub fn compute(system_text: &str, tools: &[Value]) -> Self {
        let system = md5_hex(system_text.as_bytes());

        let tools = if tools.is_empty() {
            md5_hex(b"")
        } else {
            let mut serialized: Vec<String> = tools
                .iter()
                .filter_map(|t| serde_json::to_string(t).ok())
                .collect();
            serialized.sort();
            md5_hex(serialized.join("\n").as_bytes())
        };

        let combined = md5_hex(format!("{system}:{tools}").as_bytes());

        Self {
            system,
            tools,
            combined,
        }
    }

    /// 组合指纹前 12 位(给紧凑的 metrics / 日志用)。
    pub fn short(&self) -> &str {
        short12(&self.combined)
    }

    /// system 指纹前 12 位。
    pub fn system_short(&self) -> &str {
        short12(&self.system)
    }

    /// tools 指纹前 12 位。
    pub fn tools_short(&self) -> &str {
        short12(&self.tools)
    }
}

/// 一次前缀漂移的变更记录。
#[derive(Debug, Clone)]
pub struct PrefixChange {
    pub old: PrefixFingerprint,
    pub new: PrefixFingerprint,
    /// system prompt 分量是否变化。
    pub system_changed: bool,
    /// 工具集分量是否变化。
    pub tools_changed: bool,
}

impl PrefixChange {
    /// 人读的变更描述。
    pub fn description(&self) -> String {
        let mut parts = Vec::new();
        if self.system_changed {
            parts.push("system prompt");
        }
        if self.tools_changed {
            parts.push("工具集");
        }
        if parts.is_empty() {
            return "未知(指纹不一致但未定位到分量)".to_string();
        }
        format!("前缀缓存失效:{} 变了", parts.join(" 和 "))
    }

    /// 短标签。
    pub fn label(&self) -> &'static str {
        match (self.system_changed, self.tools_changed) {
            (true, true) => "sys+tools",
            (true, false) => "sys",
            (false, true) => "tools",
            (false, false) => "prefix",
        }
    }
}

/// 跨轮监控前缀缓存稳定性。
///
/// **当前未接入运行时**:今晚只用 [`PrefixFingerprint`] 把每次 agent_loop 的前缀指纹落
/// `agent_metrics.jsonl`(跨记录比对即看出漂移)。本管理器是给后续"session 级实时漂移
/// 追踪"留的、已带单测的地基件 —— 接入时把它挂在 chat session 生命周期上即可。
#[allow(dead_code)]
#[derive(Debug, Clone, Default)]
pub struct PrefixStabilityManager {
    pinned: Option<PrefixFingerprint>,
    current: Option<PrefixFingerprint>,
    last_change: Option<PrefixChange>,
    change_count: u64,
    check_count: u64,
}

#[allow(dead_code)]
impl PrefixStabilityManager {
    /// 新建并立即钉住首个指纹。
    pub fn new(system_text: &str, tools: &[Value]) -> Self {
        let fp = PrefixFingerprint::compute(system_text, tools);
        Self {
            pinned: Some(fp.clone()),
            current: Some(fp),
            last_change: None,
            change_count: 0,
            check_count: 0,
        }
    }

    /// 新建"未钉住"态 —— 首次 `check_and_update` 时自动钉。
    pub fn new_unpinned() -> Self {
        Self::default()
    }

    /// 比较当前前缀与钉住指纹。
    /// - `Ok(true)`:稳定(指纹一致);
    /// - `Err(change)`:漂移,调用方应上报;之后自动重钉到新前缀。
    pub fn check_and_update(
        &mut self,
        system_text: &str,
        tools: &[Value],
    ) -> Result<bool, Box<PrefixChange>> {
        let fp = PrefixFingerprint::compute(system_text, tools);
        let old_fp = self.current.replace(fp.clone());
        self.check_count += 1;

        let pinned = match &self.pinned {
            Some(p) => p.clone(),
            None => {
                self.pinned = Some(fp);
                self.last_change = None;
                return Ok(true);
            }
        };

        if fp.combined == pinned.combined {
            Ok(true)
        } else {
            let old = old_fp.unwrap_or_else(|| pinned.clone());
            let system_changed = fp.system != pinned.system;
            let tools_changed = fp.tools != pinned.tools;
            let change = PrefixChange {
                old,
                new: fp.clone(),
                system_changed,
                tools_changed,
            };
            self.last_change = Some(change.clone());
            self.change_count += 1;
            self.pinned = Some(fp);
            Err(Box::new(change))
        }
    }

    pub fn last_change(&self) -> Option<&PrefixChange> {
        self.last_change.as_ref()
    }

    pub fn pinned_fingerprint(&self) -> Option<&PrefixFingerprint> {
        self.pinned.as_ref()
    }

    pub fn current_fingerprint(&self) -> Option<&PrefixFingerprint> {
        self.current.as_ref()
    }

    pub fn change_count(&self) -> u64 {
        self.change_count
    }

    pub fn check_count(&self) -> u64 {
        self.check_count
    }

    /// 前缀稳定率(0.0–1.0);没做过检查时返回 1.0 避免除零。
    pub fn stability_ratio(&self) -> f64 {
        if self.check_count == 0 {
            1.0
        } else {
            (self.check_count - self.change_count) as f64 / self.check_count as f64
        }
    }
}

/// md5 十六进制摘要(跟 `local_kb::hash` 同算法,复用现成依赖)。
fn md5_hex(bytes: &[u8]) -> String {
    format!("{:x}", md5::compute(bytes))
}

/// 取前 12 位(不足则全取)。
fn short12(s: &str) -> &str {
    if s.len() >= 12 {
        &s[..12]
    } else {
        s
    }
}
