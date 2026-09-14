//! 管理面的**纯逻辑**（零 IO）：错误码闭集、配置校验、事件环算术、协议目录常量、投影。
//!
//! 边界：handler 薄、逻辑在这里。`admin/handlers/*` 只负责取参数、调这里、拼信封。
//!
//! # 已经落地的
//!
//! - [`view`] —— 面板的**视图判定**（06 §5.3 的 **A 档**）：端点直接吐「已经判好的
//!   视图 DTO」，面板只负责把它画出来。它顶掉了 TS 版 `admin-ui/js/pure/*.mjs` 里
//!   那一半判定 —— 留在 JS 的话，本仓的四道 `cargo` 门禁**一格都跑不到**。
//!
//! # 还没有的，将来放什么
//!
//! | 将来的文件 | 放什么 | 归期 |
//! |---|---|---|
//! | `errors.rs` | `AdminErrorCode` 闭集（25 条）+ 每条码带哪些 `params` | P1-c |
//! | `config_validate.rs` | 逐字段校验（纯函数，**写之前跑，失败一个字节都不写**） | P1-c |
//! | `event_ring.rs` / `event_entry.rs` | 事件环的窗口/槽位算术与逐条窄化 | P1-c |
//! | `protocol_catalog.rs` | `PROTOCOLS` / `MEDIA_ENDPOINTS` / `MODEL_CATALOG`（面板四个消费者的单一真源） | P1-a 的 a6 |
//! | `upstream_models.rs` | `parse_upstream_models` / `diff_against_catalog` | P1-c |
//! | `upstream_facts.rs` | 上游事实登记表，含**「这条未经核实」的可执行标注** | P1-c |
//!
//! ⚠️ `AdminErrorCode` 本轮**已经有一份**，但它住在 `server::errors`：
//! 那里是三种错误信封的家，而信封与码是同一件事的两面。将来这张表要不要搬到这里，
//! 取决于 `config_validate` 落地时谁先需要它 —— **别在两处各留一份**。
//!
//! 🔴 一条写之前就要记住的红线：`slot_of` 必须用 `i32::wrapping_*`，
//! 值域有界且确定性；用普通算术会在时钟异常时 panic，而那条路径在 release 下是回绕，
//! **debug 与 release 行为不一致的判据等于没有判据**。

pub mod view;
