//! 🔴 **零 IO 岛。**
//!
//! 这一层不许：读时钟（一律收 `now: i64` 参数）、随机、定时、网络、读环境变量、碰存储。
//! 换句话说：**同样的输入永远给同样的输出**。
//!
//! ⚠️ **零 IO 不等于零不确定性**：这里仍然可以有「读不懂就当停用」这种取舍，
//! 零 IO 只保证可复现，不保证没有判断。
//!
//! # 为什么留这一层（kiro2api 没有它）
//!
//! kiro2api 把纯逻辑与 IO 混在同一个文件里，靠 `now_unix` 参数注入 + inline `#[test]`
//! 保证可测。本仓多一条**路径级**硬约束「`src/core` 零 IO」，值得保留，理由三条：
//!
//! 1. 它是单元层那 700 多格判据**能写出来的前提**：那些格子全部是「给定这份记录
//!    + 这个 now，函数该返回什么」，没有零 IO 岛就得给每一格搭一套替身。
//! 2. **它可以由一条可执行的门禁钉住**（路径前缀是最容易表达的判据形态）。
//! 3. kiro 的 `now_unix` 注入纪律与它不冲突，是它的一个子集。
//!
//! ⚠️ 代价明写：多一层目录、多一次「这个函数该放哪」的判断。
//!
//! # 一处对规格模块表的偏离，登记在此
//!
//! 规格 §2.1 的模块表里**没有** `storage_health`，而 TS 版它住在 `src/core/`。
//! 本轮把它**劈成两半**：状态本体（纯数据 + `record(writable, at)`）留在
//! [`storage_health`]；那个包一层 `Storage` 的装饰器与启动探测**下放到 `adapters`**，
//! 因为它们要碰端口与真实 IO，而本模块表明写「`core` 不许依赖 `ports`」。
//! 不劈的话，要么破坏零 IO 岛，要么破坏边界表 —— 两条都比多一次跳转贵。

//! # key 池调度那条主链（本轮落地，P1-a 的 a3）
//!
//! 读的顺序就是下面这个顺序 —— 每一个都只依赖它上面那些：
//!
//! | 文件 | 放什么 | 它最要紧的那一条 |
//! |---|---|---|
//! | [`types`] | `KeyRecord` / `KeyStats` / `PoolPolicy` | **可选字段必须真的可选**；只派生 `Serialize`（裁定 B） |
//! | [`narrow`] | 《存储窄化方向表》在 key 池这条链上的实现 | **方向逐处不同，不许统一成一种** |
//! | [`keypool`] | 三件套判据、两种选 key、五个状态迁移、四格计数 | `counts_toward_target` **与时间无关**；strike 攒满是长冷却**不是**剔除 |
//! | [`errors`] | `classify_status` / `classify_thrown` | **全局唯一那张分档表** |
//! | [`stats`] | Tier-1 记账 | 逐字段补零、取大、取非负差 —— 一句话：**绝不回退** |
//! | [`pool_index`] | `PoolIndex` 与分层窄化 | 结构级坏 ⇒ 重建；元素级坏 ⇒ 剔一个 |
//! | [`refreshable`] | TTL 快照缓存的**状态机** | `gen` 挡住「invalidate 被在途加载吃掉」；在途去重**不在这里**（见它的模块头） |
//! | [`keys::view`] | 上游 key 的面板投影 | **永不含明文**；搜索不许变成预言机 |
//! | [`keys::api_keys`] | 我们签发的 `sk-*` 的纯函数那一半 | 无盐单轮 SHA-256 是**正确选择**；`find_by_digest` 不许短路 |
//!
//! # 本轮仍然不存在的兄弟模块，各自的理由
//!
//! | 将来的文件 | 放什么 | 为什么现在没有 |
//! |---|---|---|
//! | `admin/event_ring.rs`、`admin/event_entry.rs` 等 | 管理面纯逻辑 | P1-c，见 [`admin`] 的模块头 |
//! | `protocol/*` | 四条协议的双向转换与 SSE 原语 | P1-a a6，见 [`protocol`] 的模块头 |
//!
//! 🔴 **不许先建一个空壳把名字占上**：空模块会让「这块做完了吗」这个问题变得
//! 需要读代码才能回答，而这正是本仓最想避免的那类假信号。

pub mod admin;
pub mod config;
pub mod config_errors;
pub mod config_provenance;
pub mod errors;
pub mod keypool;
pub mod keys;
pub mod narrow;
pub mod pool_index;
pub mod protocol;
pub mod refreshable;
pub mod stats;
pub mod storage_health;
pub mod types;
