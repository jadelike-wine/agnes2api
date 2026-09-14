//! 日志端口。
//!
//! 它与 `tracing` **不是一回事，两者都要**：
//! - `tracing` 是进程日志（`docker logs` 的第一现场），由 `main.rs` 初始化；
//! - 这条端口是**结构化事件**，有白名单事件名、会落进存储、是面板事件板块的数据源。
//!
//! P1 照 TS 版的白名单事件 + 事件环形态；把 `tracing` 全量捕获成事件流是 P1.5
//! （规格 §7 矛盾 5：改成全量捕获会让写预算常量的**分母彻底改变**，
//! 那件事必须与「落盘侧节流 + 容量上界」同一轮做）。

use serde_json::Value;
use std::collections::BTreeMap;

/// 日志级别。**只有三档**，与 TS 版逐字对应。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

impl LogLevel {
    /// 落盘与 logfmt 里的字面量。🔴 **这些串是运维的 grep 锚点，一个字都不许改。**
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }
}

/// 一条结构化事件。
#[derive(Debug, Clone)]
pub struct LogEntry {
    pub level: LogLevel,
    /// 事件名。🔴 **取自白名单全表（`pool.load_failed` / `config.invalid` / …），
    /// Rust 侧一个字都不许改**：它们是运维的 grep 锚点，也是面板事件板块的筛选依据。
    pub event: &'static str,
    /// 人能读的一句话。**它不是对外契约**，措辞可以改。
    pub msg: String,
    /// 相关性 id（一次请求内的若干条事件串起来）。没有就是 `None`。
    pub corr: Option<String>,
    /// 结构化字段。
    ///
    /// 🔴 **只放标量，永不放凭据**。用 `BTreeMap` 而不是 `HashMap`：
    /// logfmt 渲染要**确定性的字段顺序**，否则同一条事件在不同进程里长得不一样，
    /// 日志 diff 与判据都会变成随机的。
    pub fields: BTreeMap<String, Value>,
}

impl LogEntry {
    /// 造一条最小的事件。`fields` 与 `corr` 用 builder 追加。
    pub fn new(level: LogLevel, event: &'static str, msg: impl Into<String>) -> Self {
        Self {
            level,
            event,
            msg: msg.into(),
            corr: None,
            fields: BTreeMap::new(),
        }
    }

    /// 追加一个结构化字段。🔴 **调用点要自己保证它不是凭据。**
    #[must_use]
    pub fn with(mut self, key: impl Into<String>, value: impl Into<Value>) -> Self {
        self.fields.insert(key.into(), value.into());
        self
    }
}

/// 日志端口。
///
/// **刻意是同步的**：调用点遍布纯逻辑与热路径，做成 async 会把 `.await` 传染到
/// 每一个只想「记一句」的函数上。真正要 IO 的那个实现（事件落存储）在内部攒一个环，
/// 由中间件择机 flush —— 落盘的时机因此是**显式的**，而不是散落在每个调用点上。
///
/// 🔴 **实现绝不许把错误抛给调用方**：记日志失败不能改变业务路径的行为
/// （`logger-multi` 那 5 格判据钉的就是「一个 sink 抛错不影响别的 sink，
/// 更不影响调用方」）。
pub trait Logger: Send + Sync {
    fn log(&self, entry: LogEntry);
}

/// 什么都不做的 logger。
///
/// ⚠️ **它只该出现在测试与「调用方明确不要日志」的地方**。生产装配里如果某条路径
/// 拿到了它，现象是「事件板块里查无此事」——TS 版踩过：一个入口忘了传 logger，
/// 配置告警静默落到 NULL_LOGGER，而那正是最需要被看见的一类事件。
pub struct NullLogger;

impl Logger for NullLogger {
    fn log(&self, _entry: LogEntry) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 级别串是运维的 grep 锚点，钉成字面量。
    ///
    /// **期望值手写，不从被测对象现算**（04 §3.3 第 ⑥ 条假阳性：
    /// `assert_eq!(x.as_str(), x.as_str())` 这种同义反复）。
    #[test]
    fn level_strings_are_pinned_literals() {
        assert_eq!(LogLevel::Info.as_str(), "info");
        assert_eq!(LogLevel::Warn.as_str(), "warn");
        assert_eq!(LogLevel::Error.as_str(), "error");
    }

    /// `fields` 必须有确定性顺序，否则 logfmt 输出在不同进程里长得不一样。
    #[test]
    fn fields_iterate_in_deterministic_order() {
        let e = LogEntry::new(LogLevel::Warn, "config.invalid", "x")
            .with("zebra", 1)
            .with("alpha", 2)
            .with("mid", 3);
        let keys: Vec<&str> = e.fields.keys().map(String::as_str).collect();
        assert_eq!(keys, vec!["alpha", "mid", "zebra"]);
    }

    /// `NullLogger` 吃掉一切且不 panic —— 这是它唯一的契约。
    #[test]
    fn null_logger_swallows_everything() {
        let l = NullLogger;
        l.log(LogEntry::new(LogLevel::Error, "pool.load_failed", "boom"));
    }
}
