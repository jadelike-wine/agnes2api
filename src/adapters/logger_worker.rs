//! Workers 形态的控制台 logger：往 `console.*` 写，对齐 `wrangler tail` 与面板实时日志。
//!
//! 与 [`logger_console`] 的差别只在出口 —— `tracing`/`eprintln!` 在 wasm 上
//! 没有可见的 stderr（tracing-subscriber 的 fmt 写到 void），所以这里直接调
//! `worker::console_log!`/`console_warn!`/`console_error!`。**渲染规则与
//! `logger_console` 完全一致**（同一份 logfmt），grep 锚点两形态不变。

use crate::ports::logger::{LogEntry, LogLevel, Logger};

/// Worker 形态的 `console.*` logger。
///
/// 复用 `logger_console::render_logfmt` 的渲染，只换出口。
pub struct WorkerLogger;

impl Logger for WorkerLogger {
    fn log(&self, entry: LogEntry) {
        let line = super::logger_console::render_logfmt(&entry);
        match entry.level {
            LogLevel::Info => worker::console_log!("{line}"),
            LogLevel::Warn => worker::console_warn!("{line}"),
            LogLevel::Error => worker::console_error!("{line}"),
        }
    }
}
