//! 把结构化事件写到进程日志（`docker logs` 的第一现场）。
//!
//! # 为什么是 logfmt 而不是 JSON
//!
//! 它服务的是「运维 `docker logs | grep`」这个场景。JSON 一行的可读性在终端里很差，
//! 而 logfmt（`k=v` 空格分隔）既能 grep 又能读。
//!
//! 🔴 **引用规则是承重的，不是美观**：调用方的数据里出现空格 / 等号 / 引号时，
//! 不引用就会把字段结构**撕开** —— 一个带空格的 `msg` 会让后面的 `field=value`
//! 看起来像是 `msg` 的一部分，而运维正是靠字段边界在排障。

use crate::ports::logger::{LogEntry, LogLevel, Logger};
use serde_json::Value;

/// 往 `tracing` 里转发的 logger。
///
/// 借道 `tracing` 而不是直接 `eprintln!`，是为了让这条路与进程日志共用同一套
/// 级别过滤（`RUST_LOG`）与同一个输出目的地 —— 两套输出会让「日志里到底有没有
/// 这条」变成一个要读代码才能回答的问题。
pub struct ConsoleLogger;

impl Logger for ConsoleLogger {
    fn log(&self, entry: LogEntry) {
        let line = render_logfmt(&entry);
        match entry.level {
            LogLevel::Info => tracing::info!("{line}"),
            LogLevel::Warn => tracing::warn!("{line}"),
            LogLevel::Error => tracing::error!("{line}"),
        }
    }
}

/// 渲染成一行 logfmt。字段顺序确定：`event` → `msg` → `corr` → 其余按键名字典序。
///
/// `pub(crate)`：`WorkerLogger`（wasm）复用同一份渲染，只换出口。
pub(crate) fn render_logfmt(e: &LogEntry) -> String {
    let mut out = String::new();
    out.push_str("event=");
    out.push_str(&quote(e.event));
    out.push_str(" msg=");
    out.push_str(&quote(&e.msg));
    if let Some(corr) = &e.corr {
        out.push_str(" corr=");
        out.push_str(&quote(corr));
    }
    for (k, v) in &e.fields {
        out.push(' ');
        out.push_str(k);
        out.push('=');
        out.push_str(&quote(&scalar(v)));
    }
    out
}

/// 需要时加引号。**判据是「会不会撕开字段结构」**，不是「好不好看」。
fn quote(s: &str) -> String {
    let needs = s.is_empty()
        || s.chars()
            .any(|c| c.is_whitespace() || c == '"' || c == '=' || c.is_control());
    if !needs {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// 结构化字段渲染。**字符串不带 JSON 的那层引号**（logfmt 自己会按需加）。
fn scalar(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry() -> LogEntry {
        LogEntry::new(LogLevel::Warn, "config.invalid", "存储中的配置值非法")
    }

    /// 🔴 **带空格的值必须被引起来**，否则字段结构被撕开。
    ///
    /// 回归对象：`msg=存储中的配置值非法 field=maxStrikes` 里如果 msg 不引用，
    /// 而 msg 自己含空格，后面的 `field=...` 会被读成 msg 的一部分。
    #[test]
    fn values_with_spaces_are_quoted() {
        let e = entry().with("raw", "两 个 词");
        let line = render_logfmt(&e);
        assert!(line.contains(r#"raw="两 个 词""#), "{line}");
    }

    /// 等号与引号同样会撕开结构。
    #[test]
    fn equals_and_quotes_are_escaped() {
        let e = entry().with("raw", r#"a=b"c"#);
        let line = render_logfmt(&e);
        assert!(line.contains(r#"raw="a=b\"c""#), "{line}");
    }

    /// 换行必须转义 —— 一条日志变成两行会让后半截伪装成一条独立记录。
    #[test]
    fn newlines_never_split_a_record_in_two() {
        let e = entry().with("raw", "第一行\n第二行");
        let line = render_logfmt(&e);
        assert!(!line.contains('\n'), "一条记录必须占一行: {line}");
        assert!(line.contains(r"\n"));
    }

    /// 空串也要引起来，否则 `raw=` 后面接的下一个字段会被当成它的值。
    #[test]
    fn empty_values_are_quoted() {
        let line = render_logfmt(&entry().with("raw", ""));
        assert!(line.contains(r#"raw="""#), "{line}");
    }

    /// 不需要引号的值就不加 —— 否则 grep 的写法要分两种。
    #[test]
    fn plain_values_stay_unquoted() {
        let line = render_logfmt(&entry().with("field", "maxStrikes").with("fallback", 3));
        assert!(line.contains("field=maxStrikes"), "{line}");
        assert!(line.contains("fallback=3"), "{line}");
    }

    /// 字段顺序确定：`event` 在最前，其余按键名字典序。
    #[test]
    fn field_order_is_deterministic() {
        let e = entry().with("zzz", 1).with("aaa", 2);
        let line = render_logfmt(&e);
        assert!(line.starts_with("event=config.invalid msg="), "{line}");
        assert!(line.find("aaa=").unwrap() < line.find("zzz=").unwrap());
    }
}
