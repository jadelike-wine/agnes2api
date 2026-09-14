//! 面板的**视图判定**层：「这段数据该读成什么」「这个动作现在能不能点」
//! 「这个格子该显示哪一档」。
//!
//! # 这一层为什么在 Rust 里，而不在面板的 JS 里
//!
//! TS 版 agnes2api 把这些判定放在 `admin-ui/js/pure/*.mjs`（18 份 / 6,600 行），
//! 由 `tests/ui/` 的 1,169 格判据守着。**本仓没有 node 工具链**，门禁只有
//! `cargo build` / `cargo test` / `cargo clippy` / `cargo fmt` 四道 ⇒ 把那 18 份
//! 原样搬过来会得到**一套永远绿、永远没人跑的判据**，那比没有判据更坏。
//!
//! ⇒ `docs/porting/06-panel-spec.md` 的 **P-9** 把那一层分三档：
//!
//! | 档 | 是什么 | 去哪 |
//! |---|---|---|
//! | **A：业务判定** | 取值决策、可用性判定、分档 | 🔴 **就是本模块**。端点直接吐已经判好的视图 DTO |
//! | B：本地化格式 | `Intl` 的时刻串 / 时长 / 百分比 / 字节 | 留 JS（`Intl` 只有浏览器有），但**分支那一半由本模块的 `kind` 给** |
//! | C：浏览器交互状态机 | 多选集合、轮询退避、SSE 分帧缓冲 | 留 JS，**如实登记为无判据欠账**（06 §9 R-3） |
//!
//! ⚠️ **这不是新发明**：agnes 的管理端点本来就是这个形状
//! （`/overview` 逐块降级、`/usage` 直接给 `tier`），P-9 只是把已经开始的事做完。
//!
//! # 🔴 本模块的两条硬边界
//!
//! 1. **零 IO、零时钟、零依赖 `crate::admin`**。要「现在几点」就收一个 `now: i64`；
//!    要一条 `note` 就收 `Option<&str>`（**不是 `crate::admin::usage::UsageNote`**）
//!    —— 反向依赖会让 `core` 与 `admin` 互相引用，而 `core` 是被依赖的那一层。
//!    ⚠️ 代价明写：这意味着 `note` 在两侧是「枚举 ↔ 字符串」两种形态，
//!    靠 [`crate::admin::usage::UsageNote::as_str`] 这**一个**转换点接上，
//!    并由那边的判据钉着「每一条 code 都有一档 severity」。
//! 2. **表外一律给「不知道」，不冒充任何一档已知值**。这条纪律在 TS 侧逐字写着
//!    （`usageNoteKey` / `failureReasonKey` / `apiKeyRowLabelKey` 各一份），
//!    它的反面是「面板拿一条读不懂的信号当常态」。
//!
//! # 谁在消费
//!
//! | 本模块 | 端点 | 响应里那一格 |
//! |---|---|---|
//! | [`usage`] | `GET /admin/api/usage`、`GET /admin/api/usage/{date}` | `view` |
//! | [`keys`] | `GET /admin/api/keys` | `view` + 逐项 `items[].view` |
//! | [`apikeys`] | `GET /admin/api/apikeys` | `view` |
//! | [`events`] | `GET /admin/api/events` | `view` |
//! | [`models`] | `GET /admin/api/models` | `view` |
//! | [`reveal`] | `GET /admin/api/keys/{id}/reveal`、`GET /admin/api/apikeys/{id}/reveal` | `kind` |

pub mod apikeys;
pub mod events;
pub mod keys;
pub mod models;
pub mod reveal;
pub mod usage;
