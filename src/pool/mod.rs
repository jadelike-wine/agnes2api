//! key 池：**唯一知道「上游长什么样」的地方**（相当于 kiro2api 的 `kiro/`）。
//!
//! | 文件 | 职责 | 不许依赖 |
//! |---|---|---|
//! | [`repo`] | key 池的**存储形态**：索引、进程级快照、写消除、`pending_stats`、损坏恢复 | `server` / `admin` / `core::protocol` |
//! | [`dispatcher`] | 转发**热路径**：选 key → 发上游 → 分档记账 → 换下一把 | `admin` / `core::admin` |
//!
//! 🔴 **只有一条 relay 内核**：[`dispatcher::dispatch`]。
//! 四条协议路由是它的适配器，**不许任何协议自己拉一条到上游的链路**。
//! 违反它的现象很好认：「OpenAI 那条能用、Anthropic 那条选 key 的行为不一样」。

pub mod dispatcher;
pub mod repo;
