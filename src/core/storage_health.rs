//! 存储可写性的**状态本体**（零 IO 的那一半）。
//!
//! # 它为什么存在（真机验证暴露出来的）
//!
//! Docker 部署用 `./data:/app/data` 绑定挂载时，宿主目录的属主会盖过镜像里
//! `chown` 的结果，容器内的运行用户因此**写不进去**。此时 `/health` 仍然返回 200
//! —— 它压根不碰存储 —— 容器被报告为 healthy，**而所有 API 调用都在失败**
//! （池是空的、导入 key 报权限错）。
//!
//! **这是「CI 绿 + 镜像绿 ≠ 能跑」里最坏的一类：静默失败。**
//!
//! # 两条实现上的硬取舍
//!
//! 1. 🔴 **健康检查里刻意不写盘。** 状态由两处更新：① 启动时的一次性探测；
//!    ② 运行期每一次**真实**的写操作（装饰器包一层）。
//!    ⇒ `/health` 只读一个内存里的布尔值，**30 秒一次的 HEALTHCHECK 不产生任何写入**。
//! 2. 🔴 **只观测 `put` / `delete`。** 读操作（`get` / `list`）在数据目录只读时照样成功，
//!    **用它们判断可写性会得出「健康」的错误结论 —— 真机上正是读得到、写不进。**
//!
//! 那两件要碰 IO 的事（装饰器与启动探测）住在 [`crate::adapters::storage_watch`]，
//! 理由见 [`crate::core`] 模块头的偏离登记。

use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};

/// `checked_at` 的「还没测过」哨兵。
///
/// 用哨兵而不是 `Option<i64>` + 锁，是因为这个值在每一次写之后都要更新，
/// 而 `/health` 可能正在读它 —— 两个原子量比一把锁便宜且够用。
/// ⚠️ 代价老实写：`writable` 与 `checked_at` 不是**一起**更新的，
/// 极端交错下 `/health` 可能读到「新的 writable + 旧的 checked_at」。
/// 这可接受：`checked_at` 只用于面板展示「上次观测时刻」，
/// **没有任何判断依赖两者的一致性**。
const NEVER: i64 = i64::MIN;

/// 存储可写性的一份进程内状态。
#[derive(Debug)]
pub struct StorageHealth {
    writable: AtomicBool,
    checked_at: AtomicI64,
}

/// 对外的一份快照。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StorageStatus {
    pub writable: bool,
    /// 上一次观测的时刻（epoch 毫秒）。从没观测过就是 `None`。
    pub checked_at: Option<i64>,
}

impl Default for StorageHealth {
    fn default() -> Self {
        Self::new()
    }
}

impl StorageHealth {
    /// 初始状态取**可写**这一乐观值。
    ///
    /// 它表示的不是「已经确认能写」，而是「**尚未观测到任何写失败**」。
    /// 取悲观初值的后果更坏：进程刚起来、还没有过任何一次写的那几百毫秒里，
    /// `/health` 会报 degraded，而容器编排正好在这个窗口里判活。
    pub fn new() -> Self {
        Self {
            writable: AtomicBool::new(true),
            checked_at: AtomicI64::new(NEVER),
        }
    }

    /// 记一次观测结果。`at` 是观测时刻（epoch 毫秒），**由调用方给**（core 不读时钟）。
    pub fn record(&self, writable: bool, at: i64) {
        self.writable.store(writable, Ordering::Relaxed);
        self.checked_at.store(at, Ordering::Relaxed);
    }

    /// 当前状态。**这是 `/health` 唯一碰的东西**，零 IO。
    pub fn status(&self) -> StorageStatus {
        let checked_at = self.checked_at.load(Ordering::Relaxed);
        StorageStatus {
            writable: self.writable.load(Ordering::Relaxed),
            checked_at: (checked_at != NEVER).then_some(checked_at),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 初值是「尚未观测到写失败」，不是「已确认能写」——两者在行为上都是 `true`，
    /// 但 `checked_at` 必须如实回 `None`，**不许伪造一个观测时刻**。
    #[test]
    fn initial_state_is_optimistic_but_honest_about_never_having_checked() {
        let h = StorageHealth::new();
        assert_eq!(
            h.status(),
            StorageStatus {
                writable: true,
                checked_at: None
            }
        );
    }

    /// 观测到写失败之后状态翻过去，并带上时刻。
    #[test]
    fn recording_a_failure_flips_the_state() {
        let h = StorageHealth::new();
        h.record(false, 1_700_000_000_000);
        assert_eq!(
            h.status(),
            StorageStatus {
                writable: false,
                checked_at: Some(1_700_000_000_000)
            }
        );
    }

    /// 🔴 **能从 degraded 恢复回来。**
    ///
    /// 回归对象：如果状态是单向的（一旦坏就永远坏），运维把目录属主修好之后
    /// 容器仍然报 unhealthy，只能靠重启；而重启恰恰会丢掉一整轮在途请求。
    #[test]
    fn recovery_is_possible_without_a_restart() {
        let h = StorageHealth::new();
        h.record(false, 10);
        h.record(true, 20);
        assert_eq!(
            h.status(),
            StorageStatus {
                writable: true,
                checked_at: Some(20)
            }
        );
    }

    /// `i64::MIN` 作为哨兵：真的有人在这个时刻观测过的话也不会被吃掉。
    ///
    /// （epoch 毫秒不可能取到 `i64::MIN`，这一格钉的是「哨兵没有泄漏成一个合法值」。）
    #[test]
    fn sentinel_does_not_leak_as_a_real_timestamp() {
        let h = StorageHealth::new();
        assert!(h.status().checked_at.is_none());
        h.record(true, 0);
        assert_eq!(
            h.status().checked_at,
            Some(0),
            "0 是合法时刻，不许被当成没测过"
        );
    }
}
