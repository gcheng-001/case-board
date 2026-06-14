//! 查询缓存键算法,跟 Python `~/.claude/skills/yuandian-legal-search/scripts/cache.py`
//! 的 `_query_hash` 100% 对齐(D1.5)。
//!
//! Python 实现:
//!   raw = f"{query_type}:{json.dumps(params, sort_keys=True, ensure_ascii=False)}"
//!   h = hashlib.md5(raw.encode("utf-8")).hexdigest()[:12]
//!   return f"{query_type}-{h}"
//!
//! 跟 Python 一致的 4 个细节(决定了 hash 是否漂移,**改这个文件前请重跑 fixture 测**):
//!   1. **键按字典序排序**(对应 `sort_keys=True`)
//!   2. **中文不转义**(对应 `ensure_ascii=False`)— 直接输出 UTF-8 字节
//!   3. **分隔符是 `, ` 跟 `: `**(Python `json.dumps` 默认值,**有空格**)
//!   4. **嵌套对象递归同规则**
//!
//! Rust 自带的 `serde_json::to_string` 是紧凑模式(无空格),所以这里**不能直接用** —
//! 必须手写一个 emit。

use serde_json::Value;

/// 跟 Python `_query_hash` 完全等价。
pub fn query_hash(query_type: &str, params: &Value) -> String {
    let canonical = canonical_json_str(params);
    let raw = format!("{}:{}", query_type, canonical);
    let digest = md5::compute(raw.as_bytes());
    let hex = format!("{:x}", digest);
    format!("{}-{}", query_type, &hex[..12])
}

/// JSON 规范化:`sort_keys=True, ensure_ascii=False, separators=(', ', ': ')`。
fn canonical_json_str(v: &Value) -> String {
    let mut out = String::new();
    emit(v, &mut out);
    out
}

fn emit(v: &Value, out: &mut String) {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        // serde_json::Number 的 Display 对整数输出 "10",对浮点输出 "10.0",跟
        // Python `json.dumps` 默认一致(整数无小数点,浮点保留 .0)。
        Value::Number(n) => out.push_str(&n.to_string()),
        Value::String(s) => emit_string(s, out),
        Value::Array(arr) => {
            out.push('[');
            for (i, item) in arr.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                emit(item, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            // 键按字典序排序(Python `sort_keys=True`)
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (i, k) in keys.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                emit_string(k, out);
                out.push_str(": ");
                emit(&map[k.as_str()], out);
            }
            out.push('}');
        }
    }
}

/// 字符串 emit:跟 Python `json.dumps(..., ensure_ascii=False)` 严格一致。
/// 转义集合 = `"` `\` `\n` `\r` `\t` `\b` `\f` + 控制字符 `\u00XX`,其他全部直接输出 UTF-8。
fn emit_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{0008}' => out.push_str("\\b"),
            '\u{000C}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                // 其他控制字符:Python 用 \u00XX 小写 hex 输出
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}
