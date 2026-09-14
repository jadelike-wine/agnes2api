//! 日志扇出：把一条事件同时送进多个 sink。
//!
//! 移植自 TS 版 `src/adapters/logger-multi.ts`（15 行，判据 `logger-multi` 5 格）。
//!
//! # 🔴 它唯一的契约：**一个 sink 坏掉不许影响别的 sink，更不许影响调用方**
//!
//! 日志是**旁路**。TS 版原文：「把主流程搞挂是本末倒置」。
//! 在 TS 里「坏掉」= `throw`；在 Rust 里 [`Logger::log`] 没有 `Result`，
//! 所以「坏掉」只剩一种形态 —— **panic**。于是这条契约在 Rust 侧的落法是
//! [`std::panic::catch_unwind`]，而不是「忽略 `Err`」。
//!
//! ⚠️ **`panic = "abort"` 下这道网不存在。** `catch_unwind` 只能接住 unwind。
//! 本仓的 `[profile.release]` 没有设 `panic = "abort"`（见 `Cargo.toml`），
//! 这条网因此成立；**谁将来要设它，必须先在这里留一句「这条契约随之失效」**，
//! 别让它静默退化 —— 静默退化的现象是「某天一条 sink panic，整个请求 500」。
//!
//! ⚠️ **`AssertUnwindSafe` 是经过权衡的，不是图省事**：一个 panic 过的 sink
//! 内部状态可能已经不一致（例如它的缓冲区只写了一半）。我们**接受**这一点，
//! 因为替代方案是把 panic 往上抛到业务路径 —— 而那正是本模块存在要挡的事。
//! 各 sink 之间互不共享状态，所以一个 sink 的不一致传染不到别人。

use crate::ports::logger::{LogEntry, Logger};
use std::panic::AssertUnwindSafe;
use std::sync::Arc;

/// 扇出 logger。
///
/// 装配形态照 TS：`multi_logger([console_logger, store_logger])`。
/// **顺序有意义**：sink 按声明顺序被调用，而 console 那一路总是排在落盘那一路前面
/// —— 落盘那一路会 panic 的时候，console 那条已经打出去了。
pub struct MultiLogger {
    sinks: Vec<Arc<dyn Logger>>,
}

impl MultiLogger {
    /// 空 `sinks` 是合法的（等价于 [`crate::ports::logger::NullLogger`]），
    /// 但**装配里出现空扇出基本上是接线错误**：现象是「事件板块里查无此事」。
    /// 这里不拦，因为拦了就得定义「拦」是什么意思（panic？降级？）——
    /// 而这件事属于装配层，装配层有它自己的判据。
    pub fn new(sinks: Vec<Arc<dyn Logger>>) -> Self {
        Self { sinks }
    }

    /// 本扇出挂了几个 sink。**只给装配层的判据用**，不参与任何业务决策。
    pub fn len(&self) -> usize {
        self.sinks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sinks.is_empty()
    }
}

impl Logger for MultiLogger {
    fn log(&self, entry: LogEntry) {
        for sink in &self.sinks {
            // 每个 sink 拿到**自己的一份**：sink 的签名是按值收的，而且
            // 「上一个 sink 把 entry 消费掉了导致下一个收到半条」这种形态
            // 不许存在。`LogEntry` 是 `Clone` 的，克隆代价在日志这条旁路上可忽略。
            let e = entry.clone();
            let s: &dyn Logger = sink.as_ref();
            // 结果刻意丢弃：**panic 不许扩散**。
            // ⚠️ panic 本身仍会走默认 hook 打到 stderr —— 那是我们要的：
            //    吞掉 panic 不等于吞掉证据。
            let _ = std::panic::catch_unwind(AssertUnwindSafe(|| s.log(e)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::logger::LogLevel;
    use parking_lot::Mutex;

    /// 记录型 sink：判据靠它观察「这个 sink 到底收到了什么」。
    #[derive(Default)]
    struct Recording(Mutex<Vec<LogEntry>>);

    impl Recording {
        fn events(&self) -> Vec<&'static str> {
            self.0.lock().iter().map(|e| e.event).collect()
        }
    }

    impl Logger for Recording {
        fn log(&self, entry: LogEntry) {
            self.0.lock().push(entry);
        }
    }

    /// 会炸的 sink。**这是被测的那个反例**，不是噪音。
    struct Exploding;

    impl Logger for Exploding {
        fn log(&self, _entry: LogEntry) {
            panic!("sink 故意炸给判据看");
        }
    }

    /// 顺序记录型 sink：往一个共享的序列里按调用顺序追加自己的名字。
    struct Ordered(&'static str, Arc<Mutex<Vec<&'static str>>>);

    impl Logger for Ordered {
        fn log(&self, _entry: LogEntry) {
            self.1.lock().push(self.0);
        }
    }

    fn entry() -> LogEntry {
        LogEntry::new(LogLevel::Error, "pool.load_failed", "boom").with("n", 1)
    }

    /// 🔴 **本模块的全部理由**：中间那个 sink panic 了，两侧的 sink 照样收到，
    /// 而且调用方**正常返回**（这一行如果没返回，测试进程会直接因 panic 而红）。
    ///
    /// 回归对象：把 `catch_unwind` 去掉 —— 那时 `a` 收到、`c` 收不到、
    /// 调用方被 panic 打穿。**三条断言各自能独立抓住这次回退。**
    #[test]
    fn one_exploding_sink_affects_neither_peers_nor_caller() {
        let a = Arc::new(Recording::default());
        let c = Arc::new(Recording::default());
        let multi = MultiLogger::new(vec![a.clone(), Arc::new(Exploding), c.clone()]);

        multi.log(entry());

        assert_eq!(a.events(), vec!["pool.load_failed"], "前一个 sink 必须收到");
        assert_eq!(c.events(), vec!["pool.load_failed"], "后一个 sink 必须收到");
    }

    /// panic 过一次之后，**下一条事件照常扇出** —— 扇出器本身不许被一次 panic 毒死。
    ///
    /// 回归对象：把 `catch_unwind` 换成「记一个 poisoned 标志然后跳过后续」。
    #[test]
    fn fanout_still_works_after_a_sink_exploded() {
        let a = Arc::new(Recording::default());
        let multi = MultiLogger::new(vec![Arc::new(Exploding), a.clone()]);

        multi.log(LogEntry::new(LogLevel::Warn, "config.invalid", "一"));
        multi.log(LogEntry::new(LogLevel::Warn, "config.invalid", "二"));

        assert_eq!(a.events().len(), 2, "第二条必须照样送到");
    }

    /// 每个 sink 拿到的是**完整的同一条**：级别、事件名、正文、结构化字段都不许缺。
    ///
    /// 回归对象：为了省一次 clone 而把 `entry` 只给第一个 sink、给后面的一个空壳。
    #[test]
    fn every_sink_receives_an_identical_full_entry() {
        let a = Arc::new(Recording::default());
        let b = Arc::new(Recording::default());
        let multi = MultiLogger::new(vec![a.clone(), b.clone()]);

        multi.log(entry());

        let ga = a.0.lock();
        let gb = b.0.lock();
        assert_eq!(ga.len(), 1);
        assert_eq!(gb.len(), 1);
        assert_eq!(ga[0].level, LogLevel::Error);
        assert_eq!(gb[0].level, LogLevel::Error);
        assert_eq!(ga[0].msg, "boom");
        assert_eq!(gb[0].msg, "boom");
        // 结构化字段整份到齐 —— 期望值手写，不从被测对象现算。
        assert_eq!(
            gb[0].fields.get("n").and_then(serde_json::Value::as_i64),
            Some(1)
        );
    }

    /// sink 按**声明顺序**被调用。
    ///
    /// 为什么这件事要钉：装配里 console 排在落盘前面是有理由的
    /// （落盘那一路 panic 时，console 那条已经打出去了）。顺序一旦变成
    /// 「随便哪个先」，那个理由就失效了，而且没人会注意到。
    #[test]
    fn sinks_are_called_in_declaration_order() {
        let seq = Arc::new(Mutex::new(Vec::new()));
        let multi = MultiLogger::new(vec![
            Arc::new(Ordered("first", seq.clone())),
            Arc::new(Ordered("second", seq.clone())),
            Arc::new(Ordered("third", seq.clone())),
        ]);

        multi.log(entry());

        assert_eq!(*seq.lock(), vec!["first", "second", "third"]);
    }

    /// 空扇出不 panic、不 hang。
    ///
    /// ⚠️ 这一格**单独存在没有说服力**（04 §3.3 第 ① 条：空集合下整组断言全绿），
    /// 所以它只断言「不炸」这一件事，真正的契约由上面四格用**非空** sink 钉住。
    #[test]
    fn empty_fanout_is_a_no_op() {
        let multi = MultiLogger::new(vec![]);
        multi.log(entry());
        assert_eq!(multi.len(), 0);
        assert!(multi.is_empty());
    }
}
