//! 流式工具调用参数的确定性 JSON 修复(strategy A:修不好也派发空对象,绝不炸整轮)。
//!
//! DeepSeek 把 `tool_calls.function.arguments` 按 SSE delta 分块流式吐出(见
//! `agent_loop.rs` 的 `StreamingToolCall::arguments_buf` 累积)。两类常见损坏:
//! (a) chunk 边界切在 JSON 字符串中间,拼回来留下尾逗号 / 未闭合括号;
//! (b) 某些本地后端在 JSON 字符串值里塞了字面控制字符。
//! 拼好的 buffer 因此过不了严格 `serde_json::from_str` —— 旧逻辑一坏就 `Err` 炸整轮,
//! 体感像"模型本来要调工具改文书,却莫名其妙崩了"。本模块用确定性阶梯救回:
//!
//!  1. 严格解析 —— 能过即返回(正常情况零开销、行为与旧逻辑完全一致)。
//!  2. 剥字符串内的控制字符。
//!  3. 删 `}` / `]` 前的尾逗号。
//!  4. 补齐缺失的右括号 / 右方括号。
//!  5. 删多余的右括号。
//!  6. 兜底空对象 `{}` —— 让派发继续、由工具自身校验报错,而不是死回合。
//!
//! 移植自 CodeWhale(<https://github.com/Hmbown/CodeWhale>,MIT,
//! `crates/tui/src/tools/arg_repair.rs`)。Copyright (c) 2024-2025 DeepSeek CLI Contributors,
//! MIT License。仅做一处适配:严格解析(stage 1)对**任意长度**都先尝试,`MAX_ARG_LEN`
//! 上限只挡"坏且超大"的输入进入昂贵修复阶梯 —— 因为本项目 `save_artifact` 可能把整篇
//! 长文书作为参数传入(max_tokens 已放到 384K),**合法的超大参数不能被误拒**。

use serde_json::{Map, Value};

/// 进入修复阶梯(stage 2+)的最大原始参数长度(4 MiB)。
/// 384K 输出 token 的中文文书最坏约 2MB,留足余量;只在严格解析失败后才用此上限挡 pathological 超大输入。
const MAX_ARG_LEN: usize = 4 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum ArgRepairError {
    #[error("参数超过 {0} 字符且非合法 JSON,拒绝修复")]
    TooLarge(usize),
}

/// 把原始 JSON 参数串修成合法的 `serde_json::Value`。
///
/// 跑确定性阶梯;成功返回解析值。最终兜底是空对象 `{}`,保证派发始终能继续。
/// 唯一的 `Err` 是"严格解析失败 **且** 超过 `MAX_ARG_LEN`"的 pathological 情况。
pub fn repair(raw: &str) -> Result<Value, ArgRepairError> {
    // Stage 1: 严格解析 —— 任意大小都先试,保证"巨大但合法"的参数(如长文书 save_artifact)零开销通过。
    if let Ok(v) = serde_json::from_str(raw) {
        return Ok(v);
    }
    // 修复阶梯对 pathological 超大输入开销大(balance_braces 多轮全串扫描),仅对它设上限。
    if raw.len() > MAX_ARG_LEN {
        return Err(ArgRepairError::TooLarge(raw.len()));
    }
    // Stage 2: 剥字符串内的控制字符
    let mut s = strip_control_chars_in_strings(raw);
    if let Ok(v) = serde_json::from_str(&s) {
        return Ok(v);
    }
    // Stage 3: 删尾逗号
    s = strip_trailing_commas(&s);
    if let Ok(v) = serde_json::from_str(&s) {
        return Ok(v);
    }
    // Stage 4: 补齐括号
    s = balance_braces(&s, 50);
    if let Ok(v) = serde_json::from_str(&s) {
        return Ok(v);
    }
    // Stage 5: 删多余右括号
    s = strip_excess_closers(&s);
    if let Ok(v) = serde_json::from_str(&s) {
        return Ok(v);
    }
    // 兜底:空对象
    Ok(Value::Object(Map::new()))
}

/// 剥掉 JSON 字符串值内的 ASCII 控制字符(0x00–0x1F,保留 \t \n \r)。
/// 逐字符走,跟踪是否在字符串内(未转义的双引号之间)。
fn strip_control_chars_in_strings(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_string = false;
    let mut escape = false;
    for ch in s.chars() {
        if escape {
            out.push(ch);
            escape = false;
            continue;
        }
        if ch == '\\' {
            escape = true;
            out.push(ch);
            continue;
        }
        if ch == '"' {
            in_string = !in_string;
            out.push(ch);
            continue;
        }
        if in_string && (ch as u32) < 0x20 && ch != '\t' && ch != '\n' && ch != '\r' {
            // 丢弃字符串内的控制字符
            continue;
        }
        out.push(ch);
    }
    out
}

/// 删 `}` 或 `]` 前的尾逗号。
fn strip_trailing_commas(s: &str) -> String {
    // 反复替换 ",}" 和 ",]" 直到稳定(处理嵌套)。
    let mut out = s.to_string();
    loop {
        let prev = out.clone();
        out = out.replace(",}", "}").replace(",]", "]");
        // 处理串尾的尾逗号
        out = out.trim_end_matches(',').to_string();
        if out == prev {
            break;
        }
    }
    out
}

/// 补齐括号:数 `{`/`}` 和 `[`/`]`,正差(开多于闭)就补右括号。
/// 限制迭代次数,避免极端损坏输入死循环。
fn balance_braces(s: &str, max_iter: usize) -> String {
    let mut out = s.to_string();
    for _ in 0..max_iter {
        let brace_delta: i32 = out
            .chars()
            .map(|ch| match ch {
                '{' => 1,
                '}' => -1,
                _ => 0,
            })
            .sum();
        let bracket_delta: i32 = out
            .chars()
            .map(|ch| match ch {
                '[' => 1,
                ']' => -1,
                _ => 0,
            })
            .sum();
        if brace_delta <= 0 && bracket_delta <= 0 {
            break;
        }
        // 按正确嵌套顺序补(先方括号后花括号)。
        for _ in 0..bracket_delta.max(0) {
            out.push(']');
        }
        for _ in 0..brace_delta.max(0) {
            out.push('}');
        }
    }
    out
}

/// 负差(闭多于开)时删多余右括号。
fn strip_excess_closers(s: &str) -> String {
    let mut brace_depth: i32 = 0;
    let mut bracket_depth: i32 = 0;
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '}' => {
                if brace_depth > 0 {
                    brace_depth -= 1;
                    out.push(ch);
                }
                // 否则丢弃多余右括号
            }
            ']' => {
                if bracket_depth > 0 {
                    bracket_depth -= 1;
                    out.push(ch);
                }
            }
            '{' => {
                brace_depth += 1;
                out.push(ch);
            }
            '[' => {
                bracket_depth += 1;
                out.push(ch);
            }
            _ => out.push(ch),
        }
    }
    out
}
