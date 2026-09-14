//! 测试临时目录的**统一出口**。`#[cfg(test)]`，不进产物。
//!
//! # 为什么必须有这个模块（照抄 kiro2api 03 §10.3）
//!
//! 各模块测试各自 `std::env::temp_dir().join(format!("xxx_{pid}_{nanos}"))`，
//! 建完从不删。**跑一轮完整测试会在 `/tmp` 顶层留下上千个目录，且永不回收**——
//! kiro 那边实测排查磁盘问题时，`/tmp` 里堆了 9,582 个这样的残留（仅 4 天积累）。
//!
//! 解法：所有临时产物收进 `/tmp/agnes2api-tests/<pid>/`，每个测试进程启动时
//! （`Once`）回收掉**属于已退出进程**的旧根目录（Linux 上看 `/proc/<pid>` 在不在）。
//!
//! ⚠️ 这条「看 `/proc/<pid>`」的判活方式**显式依赖 Linux**。非 Linux 上它退化成
//! 「不回收」，只多占磁盘，不会误删别人的目录 —— 失败方向是对的。

use std::path::PathBuf;
use std::sync::Once;

static CLEANUP: Once = Once::new();

/// 本进程的临时目录根：`/tmp/agnes2api-tests/<pid>/`。
fn root() -> PathBuf {
    CLEANUP.call_once(reap_dead_process_dirs);
    let root = std::env::temp_dir()
        .join("agnes2api-tests")
        .join(std::process::id().to_string());
    let _ = std::fs::create_dir_all(&root);
    root
}

/// 回收上一次（已经退出的）测试进程留下的目录。
///
/// **只删确定已退出的那些**：`/proc/<pid>` 不存在才删。看不懂的名字一律跳过 ——
/// 误删一个正在跑的并发测试进程的目录，会造出最难查的那种「偶发失败」。
fn reap_dead_process_dirs() {
    let base = std::env::temp_dir().join("agnes2api-tests");
    let Ok(entries) = std::fs::read_dir(&base) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Ok(pid) = name.parse::<u32>() else {
            continue;
        };
        if pid == std::process::id() {
            continue;
        }
        if !PathBuf::from(format!("/proc/{pid}")).exists() {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// 一个**唯一的**临时目录。`label` 只是给人看的，不参与唯一性。
///
/// 唯一性靠一个进程内单调计数器，**不靠时间戳**：同一毫秒里建两个目录
/// 在测试里非常常见，而时间戳碰撞造出的是「两个测试互相踩」这种偶发失败。
pub fn dir(label: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let p = root().join(format!("{label}-{n}"));
    let _ = std::fs::create_dir_all(&p);
    p
}

#[cfg(test)]
mod tests {
    /// 两次调用给两个不同的目录（同一个 label 也一样）。
    #[test]
    fn dirs_are_unique_even_with_the_same_label() {
        let a = super::dir("same");
        let b = super::dir("same");
        assert_ne!(a, b);
        assert!(a.is_dir() && b.is_dir());
    }

    /// 目录落在本进程的根下面 —— 这是「按 pid 回收」能成立的前提。
    #[test]
    fn dirs_live_under_this_processes_root() {
        let d = super::dir("scoped");
        assert!(
            d.to_string_lossy()
                .contains(&std::process::id().to_string()),
            "临时目录必须带上 pid，否则回收器分不清谁的"
        );
    }
}
