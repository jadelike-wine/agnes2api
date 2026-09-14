//! 端口层 —— **它的用途只有一个：测试接缝。写死在这里，不许扩大。**
//!
//! # 为什么这一层存在（它是对 kiro2api 形状的一次刻意偏离）
//!
//! kiro2api **没有任何端口抽象**：`Pool` 直接持 `reqwest::Client`，存储就是
//! `stats/persist.rs` 的具体函数。本仓保留端口层，理由**不是**「将来换后端」——
//! P0 砍掉 Cloudflare Worker 之后没有第二个运行时，也不打算有。
//!
//! 理由是：**一整族买过单的判据，用「无抽象 + wiremock」写不出来**。举证（规格 §3.3）：
//!
//! - 🔴 `admin-apikeys-quota`：1000 次错口令**恰好**产生 1 次存储 `get` —— 要数精确次数；
//! - 🔴 `quota-panel`：面板轮询路径上**零 `list()`**、零额外读 —— 同样是精确次数；
//! - 🔴 `wiring`：`/health` **零存储 I/O**，且计数桩必须 `get/put/delete/list` **四种全数**
//!   （上一版只数 `get`，而被违反的恰恰是写那一侧）；
//! - 🔴 `storage`：端口契约「过期后 `get` 与 `list` 都看不见」要**对每种实现各跑一遍**——
//!   没有端口就没有「每种实现」这件事；
//! - 🔴 `pool-index-corrupt`：要构造「`get` 抛错而 `list` 不抛」这种精确组合。
//!
//! 这不是「测试友好」的偏好。判据清点的结论是 166 个判据文件里 160 个命中「记录着真实
//! 事故」——丢掉接缝就是丢掉这一族事故的回归。
//!
//! # 边界（🔴 写死，改要在评审里被点名）
//!
//! 1. **只有 3 条**：[`storage::Storage`] / [`fetcher::Fetcher`] / [`logger::Logger`]。
//!    新增端口必须回答「它挡住的是哪一格判据」。
//! 2. **端口不是可移植层。** `Storage` 的第二个实现只应该出现在 `#[cfg(test)]` 里。
//!    ⚠️ 例外：storage 契约判据是「对每种实现各跑一遍」，P1 的「每种实现」＝
//!    [`crate::adapters::storage_file::FileStorage`] + 测试里的 `MemoryStorage`。
//! 3. **`Runtime` 端口砍掉**（TS 版有）：它的用途是抹平 Worker 与 Node 的进程指标差异，
//!    P0 之后只剩一种。Rust 侧直接读 `std::process::id()` + `/proc/self/statm`，
//!    失败时如实回 `null`，**绝不伪造 0**。⚠️ 这让本仓显式依赖 Linux，要写进部署文档。
//! 4. **邮箱通道没有成为一个端口，如实登记这处偏离。**
//!    它落在 [`crate::registrar::mint::MailboxProvider`]，而不是本目录下。
//!    理由：端口是**跨子系统**的注入缝（存储、出站、日志三者都有多个消费方），
//!    而邮箱通道只有注册机一个消费方。把它抬进 `ports` 只会让
//!    「谁依赖谁」这张图多一条没有第二个端点的边。
//! 5. **适配器自己要有判据。** 端口最大的风险是「替身全绿而真实现坏了」，所以
//!    `storage_file` 要跑完整的端口契约，`fetcher_reqwest` 要用 wiremock 跑真 HTTP。
//!
//! # 为什么是 `Arc<dyn Trait>` 而不是泛型单态化
//!
//! 1. **泛型会传染到每一个 handler**。axum 要求 state 是一个具体类型，
//!    `AppState<S, F, L>` 一出现，`Router<AppState<..>>`、每个 handler 签名、
//!    每个 `from_fn_with_state` 都要带三个类型参数 —— 这与 kiro2api「一个具体
//!    `MessagesState` 被三组路由共用」的形状正面冲突，而那个形状是最值得抄的一条。
//! 2. **零开销在这里换不到东西**：三条端口的每一次调用**本身就是一次 IO**，
//!    一次 vtable 跳转在噪声里。唯一算得上热的 `Storage::get` 恰恰被判据要求在转发
//!    热路径上是**零次** —— 优化一条判据要求不许发生的调用，是本末倒置。
//! 3. 三个类型参数的笛卡尔积会让单态化实例翻几倍，而 CI 有四道阻塞门，
//!    编译时间直接换成迭代速度。
//!
//! 代价老实写：多一层间接；trait 要 `dyn` 兼容，于是依赖 `async-trait`（见 Cargo.toml）。

pub mod fetcher;
pub mod logger;
pub mod storage;
