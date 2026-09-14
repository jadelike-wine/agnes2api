//! 单文件 JSON 存储：`<DATA_DIR>/store.json`。
//!
//! # 为什么 P1 不换存储引擎（sled / redb / SQLite）
//!
//! 因为 P1 的定义是**行为等价的 Rust 骨架**。同一轮里既换语言又换存储引擎，
//! 任何一条判据失败时都无法归因。而且写放大那一族的全部常量
//! （写消除、`pending_stats`、各种退避）恰恰建立在「每次 `put` 重写整份文件」
//! 这个性质上 —— 先让它们在 Rust 下逐条成立，再谈换引擎。
//!
//! ⚠️ **代价老实写**：每次 `put` 都重写整个 `store.json`，每次 `get`/`list` 都是
//! 整份文件的读 + 解析。池子几百把 key 时这是实打实的磁盘写放大，
//! 而它与 key 池的状态回写走的是同一个文件、同一条写队列。
//! 这正是「Tier-2 用量统计默认关」的今日理由。
//!
//! # 四条从 kiro2api 抄来的持久化纪律
//!
//! 1. **`tmp → fsync → rename` 原子写**，临时文件名带 **pid + 进程内单调计数器**：
//!    固定的 `.tmp` 会让两个并发写互抢同一个文件，第一个 rename 成功后 tmp 就没了，
//!    第二个必然失败。
//! 2. **孤儿 tmp 只删 mtime 早于 5 分钟的**（见 [`FileStorage::cleanup_orphan_tmp`]）：
//!    5 分钟远大于一次正常落盘（亚秒级），**绝不会误删并发写者正在写的 tmp**。
//! 3. 🔴 **坏文件绝不静默当空**（见 [`FileStorage::preflight`]）。
//! 4. 🔴 **凭据文件一律 `0600`，三条产文件的路径一条都不许漏**（见 [`create_private`]）：
//!    主文件（靠 tmp 的权限经 rename 带过去）、tmp 本身、`.corrupt.*` 备份。
//!
//! # 单进程 **不等于** 串行
//!
//! `put`/`delete` 都是「读全量 → 改一个键 → 写全量」，两个并发调用会交错，
//! 后写的覆盖先写的，造成**静默丢更新** —— 而转发路径每次请求都在返回成功响应
//! **之前**写回 key 状态，丢更新就等于把一个已经成功的上游响应变成 500。
//! ⇒ 这里用一把写锁把 `put`/`delete` 整体串行化。
//! 读（`get`/`list`）是单次整份读，rename 是原子替换，天然一致，**不入队**。

use crate::adapters::now_ms;
use crate::ports::storage::{Storage, StorageError, StorageResult};
use async_trait::async_trait;
use serde_json::{Map, Value};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};
use tokio::sync::Mutex;

/// TTL 表的保留键：一张「key → 过期时刻（epoch 毫秒）」的表，与业务键住在**同一份**
/// JSON 里。
///
/// 单文件、单次读写，**不另开 sidecar 文件**：第二个文件就是第二次跨文件一致性问题。
///
/// # 为什么是**前导空格**而不是 NUL 字节
///
/// 业务键全部带可打印 ASCII 前缀（`key:` / `pool:index` / `config` / `event:` …），
/// 前导空格不可能与它们碰撞。而用字面 NUL（`\x00`）的写法 **TS 版踩过一个很贵的坑**：
/// git 会把整个源文件判成二进制，于是
/// ① 评审包对这个文件只吐一行「Binary files differ」（**最核心的文件没人看过**）；
/// ② `git grep -I` 的凭据扫描会**跳过**它 —— 那道「仓库零内置凭据」的门禁对这个文件
///    完全失明（实测：塞一段假凭据进去，门禁照样放行）。
/// ⇒ 一个字节的选择，换掉的是一整道安全门禁的有效性。
const TTL_TABLE_KEY: &str = " ttl";

/// 孤儿临时文件的回收年龄。见模块头第 2 条。
const ORPHAN_TMP_AGE: Duration = Duration::from_secs(5 * 60);

/// 单文件 JSON 存储。
pub struct FileStorage {
    file: PathBuf,
    /// 写队列。**跨 await 持有，所以必须是 `tokio::sync::Mutex` 而不是 `parking_lot`**
    /// （本仓最硬的一条不成文规矩：不跨 await 才用同步锁）。
    write_lock: Mutex<()>,
    /// 临时文件名里的进程内单调计数器。见模块头第 1 条。
    tmp_seq: AtomicU64,
}

/// `store.json` 的完整性。[`FileStorage::preflight`] 的产物。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreIntegrity {
    /// 文件不存在 —— 全新部署，这是正常的。
    Missing,
    /// 文件在，且解析得动。
    Ok,
    /// 🔴 文件在，但解析不动。**已原样备份到 `backup`，原件保持不动。**
    Corrupt { backup: PathBuf },
}

impl FileStorage {
    /// `data_dir` 是数据目录，真正的文件是它下面的 `store.json`。
    ///
    /// **构造函数不碰磁盘**：启动期的那两件 IO（孤儿清理、完整性体检）是
    /// [`FileStorage::cleanup_orphan_tmp`] 与 [`FileStorage::preflight`] 两个显式调用，
    /// 由装配层按顺序调 —— 藏在构造函数里的 IO 会让每一个测试都被迫进文件系统。
    pub fn new(data_dir: impl AsRef<Path>) -> Self {
        Self {
            file: data_dir.as_ref().join("store.json"),
            write_lock: Mutex::new(()),
            tmp_seq: AtomicU64::new(0),
        }
    }

    /// 落盘文件的路径（面板与诊断用）。
    pub fn path(&self) -> &Path {
        &self.file
    }

    /// 启动期完整性体检。🔴 **坏文件绝不静默当空。**
    ///
    /// # 这条纪律是一条事故链换来的
    ///
    /// 旧写法把读失败吞成「空」：进程带着 0 条记录启动、日志里一个字都没有，
    /// 运维只看到转发全 503、面板显示池子是空的，于是「那就重新导入一批 key」——
    /// 而管理侧的原子落盘会用内存里的**空池整份覆写** `store.json`，
    /// **原来那一池 key 当场蒸发且不可逆。**
    ///
    /// # 本实现做什么、**刻意不做什么**
    ///
    /// 做：**原样备份**成 `store.json.corrupt.<unix秒>.<序号>`（此后任何原子写都覆不掉它），
    /// 并把结论交给调用方去大声报错。
    ///
    /// **刻意不做**「逐条抢救」：kiro 那边的对象是一个 JSON **数组**，坏一条跳一条即可；
    /// 这里的对象是一个**整体解析不动的 JSON 对象**，没有「条」可以逐个抢救。
    /// 更重要的是**原件原地不动**：于是此后每一次读都继续如实失败 ——
    /// 而 [`Storage::put`] 是「先读后写」，读不动就**根本走不到写**，
    /// 这就是落盘自保阀（内存空 + 磁盘坏 ⇒ 跳过写盘）在本实现里的形态。
    /// ⚠️ key 池那一级的「逐条抢救」有对象，它归 `pool::repo`（每把 key 一条记录）。
    pub async fn preflight(&self) -> StoreIntegrity {
        let file = self.file.clone();
        tokio::task::spawn_blocking(move || preflight_sync(&file))
            .await
            .unwrap_or(StoreIntegrity::Ok)
    }

    /// 清掉上次崩溃留下的孤儿临时文件。
    ///
    /// **只删 mtime 早于 5 分钟的**：一次正常落盘是亚秒级的，
    /// 这个阈值保证绝不会误删另一个副本**正在写**的 tmp（共卷部署下真实存在）。
    /// 任何一步失败都只是少清一个文件，**不许让启动失败**，所以它不回 `Result`。
    pub async fn cleanup_orphan_tmp(&self) -> usize {
        let file = self.file.clone();
        tokio::task::spawn_blocking(move || cleanup_orphan_tmp_sync(&file))
            .await
            .unwrap_or(0)
    }

    /// 读整份文件。**文件不存在 ⇒ 空表；解析不动 ⇒ 如实 `Err`。**
    async fn read_all(&self) -> StorageResult<Map<String, Value>> {
        let file = self.file.clone();
        blocking(move || read_all_sync(&file)).await
    }
}

#[async_trait]
impl Storage for FileStorage {
    async fn get(&self, key: &str) -> StorageResult<Option<Value>> {
        let all = self.read_all().await?;
        if is_expired(&all, key, now_ms()) {
            return Ok(None);
        }
        Ok(all.get(key).cloned())
    }

    async fn put(&self, key: &str, value: Value, expires_at: Option<i64>) -> StorageResult<()> {
        let _guard = self.write_lock.lock().await;
        let file = self.file.clone();
        let tmp_id = self.tmp_seq.fetch_add(1, Ordering::Relaxed);
        let key = key.to_string();
        blocking(move || {
            let mut all = read_all_sync(&file)?;
            all.insert(key.clone(), value);
            let mut ttl = read_ttl_table(&all);
            match expires_at {
                Some(at) => {
                    ttl.insert(key, Value::from(at));
                }
                // **没传就把旧的 TTL 摘掉**：同一把键从「会过期」改写成「不过期」时，
                // 留着旧记录会让它在一个谁都没预期的时刻消失。
                None => {
                    ttl.remove(&key);
                }
            }
            prune_expired(&mut all, &mut ttl, now_ms());
            write_ttl_table(&mut all, ttl);
            write_all_sync(&file, &all, tmp_id)
        })
        .await
    }

    async fn delete(&self, key: &str) -> StorageResult<()> {
        let _guard = self.write_lock.lock().await;
        let file = self.file.clone();
        let tmp_id = self.tmp_seq.fetch_add(1, Ordering::Relaxed);
        let key = key.to_string();
        blocking(move || {
            let mut all = read_all_sync(&file)?;
            all.remove(&key);
            let mut ttl = read_ttl_table(&all);
            ttl.remove(&key);
            prune_expired(&mut all, &mut ttl, now_ms());
            write_ttl_table(&mut all, ttl);
            write_all_sync(&file, &all, tmp_id)
        })
        .await
    }

    async fn list(&self, prefix: &str) -> StorageResult<Vec<String>> {
        let all = self.read_all().await?;
        let now = now_ms();
        Ok(all
            .keys()
            .filter(|k| k.as_str() != TTL_TABLE_KEY)
            .filter(|k| k.starts_with(prefix))
            .filter(|k| !is_expired(&all, k, now))
            .cloned()
            .collect())
    }
}

// ── 同步实现（全部在 spawn_blocking 里跑）───────────────────────────────────

/// 把一段同步文件操作丢进阻塞线程池。
///
/// 🔴 **同步写 + fsync 可达数十毫秒，绝不许占住 async 工作线程**：
/// 占住的后果是同一个线程上的其它在途转发请求一起卡住，而现象是「偶发超时」。
async fn blocking<T, F>(f: F) -> StorageResult<T>
where
    F: FnOnce() -> StorageResult<T> + Send + 'static,
    T: Send + 'static,
{
    match tokio::task::spawn_blocking(f).await {
        Ok(r) => r,
        // 阻塞任务 panic 了。这是代码 bug，但**不许让它变成进程崩溃**：
        // 存储失败有既定的处置路径（/health 转 degraded、调用方按窄化方向降级）。
        Err(e) => Err(StorageError::new(format!("存储任务异常终止: {e}"))),
    }
}

/// 建一个**仅属主可读写**（`0600`）的文件；已存在就截断。
///
/// # 为什么不能用 `std::fs::File::create`
///
/// 它取 `0666 & ~umask` —— 默认 umask 022 下是 `0644`，容器里常见的 umask 002 下是
/// `0664`，**两个都是组外可读**。而 `store.json` 里装着三样明文凭据：
/// `key:<id>.key`（上游 Agnes key，设计上必须存明文）、`apikeys.keys[].secret`
/// （已签发的 `sk-` 子密钥）、`config.gatewayToken`（面板设的网关口令）。
///
/// 数据目录被 bind mount 到宿主共享目录是**本仓自己在 `health.rs` 的
/// `UNWRITABLE_DETAIL` 里承认的常见部署形态**：那时同宿主上任意一个非 root 本地用户
/// `cat data/store.json` 就一次性拿到全部上游 key + 全部已签发 `sk-` 明文 + 网关口令，
/// **不需要任何凭据、也不留任何事件**。
///
/// 🔴 **权限必须在建文件时就定死，不能事后 `chmod`**：`rename` 保留 tmp 的权限位，
/// 所以「tmp 是 `0600`」才是「落盘后的 `store.json` 是 `0600`」的**唯一**来源；
/// 反过来，运维手动 `chmod 600` 过的文件会被下一次 `put` 的 rename **静默放宽回去**。
/// 这条规约与 kiro2api 的 `stats/persist.rs::create_private` 逐字同形。
///
/// ⚠️ **非 Unix（Windows）上不施加任何限制**，退回 `File::create`：那里没有 mode 位，
/// 访问控制走 ACL，而本仓没有 Windows 部署形态、也没有任何一格判据跑在那上面。
/// 与其装一个没人验证过的 ACL 收紧，不如如实写「**那个平台上今天没人守这一条**」。
fn create_private(path: &Path) -> std::io::Result<std::fs::File> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
    }
    #[cfg(not(unix))]
    {
        std::fs::File::create(path)
    }
}

/// 把一个**已存在**的文件收紧到 `0600`。**失败一律吞掉**，见调用点。
///
/// 非 Unix 上是空操作，理由同 [`create_private`]。
fn tighten_existing(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

fn read_all_sync(file: &Path) -> StorageResult<Map<String, Value>> {
    let bytes = match std::fs::read(file) {
        Ok(b) => b,
        // 全新部署：文件还不存在。这是正常路径，不是错误。
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Map::new()),
        Err(e) => {
            return Err(StorageError::new(format!(
                "读取 {} 失败: {e}",
                file.display()
            )));
        }
    };
    match serde_json::from_slice::<Value>(&bytes) {
        Ok(Value::Object(map)) => Ok(map),
        // 🔴 顶层不是对象 —— 同样是「解析不动」，**不许当空表**，理由见 `preflight`。
        Ok(_) => Err(StorageError::new(format!(
            "{} 的顶层不是 JSON 对象，拒绝当作空存储处理",
            file.display()
        ))),
        Err(e) => Err(StorageError::new(format!(
            "解析 {} 失败: {e}",
            file.display()
        ))),
    }
}

fn write_all_sync(file: &Path, all: &Map<String, Value>, tmp_id: u64) -> StorageResult<()> {
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| StorageError::new(format!("创建数据目录 {} 失败: {e}", dir.display())))?;
    }
    // 临时文件名 = 目标名 + `.tmp.<pid>.<进程内单调序号>`。见模块头第 1 条。
    let tmp = file.with_extension(format!("json.tmp.{}.{}", std::process::id(), tmp_id));

    let write = || -> std::io::Result<()> {
        // 🔴 `create_private` 而不是 `File::create`：rename 把 tmp 的权限位带到
        // `store.json` 上，所以主文件的权限**完全由这一行决定**。用 `File::create`
        // 的那一版让每一次落盘都把这份凭据文件放成组外可读，且会把运维手动
        // `chmod 600` 的结果静默冲掉。见 [`create_private`]。
        let mut f = create_private(&tmp)?;
        // 缩进 2 空格：`store.json` 是运维会直接打开看的文件。
        serde_json::to_writer_pretty(&mut f, all)
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        f.flush()?;
        // 🔴 fsync 必须在 rename **之前**：没有它，崩溃后可能 rename 成功而内容是空洞。
        f.sync_all()?;
        std::fs::rename(&tmp, file)
    };

    if let Err(e) = write() {
        // 别把半成品留在数据目录里。删不掉也只是多一个孤儿，由 `cleanup_orphan_tmp` 兜。
        let _ = std::fs::remove_file(&tmp);
        return Err(StorageError::new(format!(
            "写入 {} 失败: {e}",
            file.display()
        )));
    }
    Ok(())
}

fn preflight_sync(file: &Path) -> StoreIntegrity {
    if !file.exists() {
        return StoreIntegrity::Missing;
    }
    // 升级路径的收紧点。旧版本用 `File::create` 落下的 `store.json` 是 `0644`/`0664`，
    // 而权限只在 rename 时跟着 tmp 走 —— 一个只读流量的副本可能几天都不写一次，
    // 「升级即修复」不能建立在「恰好有人写过一次」这个前提上。
    // **best-effort**：只读挂载 / 属主不是本进程时改不动，那只是少收紧一个文件，
    // 绝不许让启动失败（所以 `tighten_existing` 不回 `Result`，本函数也不回）。
    tighten_existing(file);
    if read_all_sync(file).is_ok() {
        return StoreIntegrity::Ok;
    }
    let stamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // 序号：同一秒里体检两次也不许覆盖上一次的备份。
    for n in 0..1000u32 {
        let backup = file.with_extension(format!("json.corrupt.{stamp}.{n}"));
        if backup.exists() {
            continue;
        }
        // 🔴 **不用 `std::fs::copy`**：它把源文件的权限位一并复制过去。升级前留下的
        // `0644` 源文件会生出一份同样 `0644` 的副本 —— 内容是一字不差的全部明文凭据，
        // 而它恰恰是**此后任何原子写都覆不掉、也没有任何人会再去 chmod 的**那一份。
        if copy_private(file, &backup).is_ok() {
            return StoreIntegrity::Corrupt { backup };
        }
        break;
    }
    // 连备份都做不出来（目录只读？），**仍然如实报 Corrupt**：
    // 报 Ok 才是最坏的选择。`backup` 指回原件，调用方的文案里会看到它。
    StoreIntegrity::Corrupt {
        backup: file.to_path_buf(),
    }
}

/// 原样复制一个文件，但**目标以 `0600` 新建**（`std::fs::copy` 的权限继承的替代）。
///
/// 整份读进内存再写出去：与 [`read_all_sync`] 一样的量级（那里本来就 `std::fs::read`
/// 整份文件），不引入新的内存形态。
fn copy_private(src: &Path, dst: &Path) -> std::io::Result<()> {
    let bytes = std::fs::read(src)?;
    let mut f = create_private(dst)?;
    f.write_all(&bytes)?;
    f.flush()
}

fn cleanup_orphan_tmp_sync(file: &Path) -> usize {
    let Some(dir) = file.parent() else { return 0 };
    let Some(stem) = file.file_name().and_then(|s| s.to_str()) else {
        return 0;
    };
    let prefix = format!("{stem}.tmp.");
    // ⚠️ 注意 `file.with_extension("json.tmp.…")` 产生的名字是 `store.json.tmp.…`，
    //    与这里的前缀一致（`stem` 取的是含扩展名的文件名 `store.json`）。
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let now = SystemTime::now();
    let mut removed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !name.starts_with(&prefix) {
            continue;
        }
        let old_enough = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|m| now.duration_since(m).ok())
            .is_some_and(|age| age >= ORPHAN_TMP_AGE);
        if old_enough && std::fs::remove_file(entry.path()).is_ok() {
            removed += 1;
        }
    }
    removed
}

// ── TTL 表 ─────────────────────────────────────────────────────────────────

/// 从整份数据里取 TTL 表。**缺失 / 畸形一律当空表**（窄化，不假设形状）。
///
/// 这里是全仓少数几个「读不懂就当没有」是**正确**方向的地方：TTL 表读不懂的后果
/// 是几把键不再过期（多占点磁盘），而当成「全部过期」会**当场删掉真实数据**。
fn read_ttl_table(all: &Map<String, Value>) -> Map<String, Value> {
    let Some(Value::Object(raw)) = all.get(TTL_TABLE_KEY) else {
        return Map::new();
    };
    raw.iter()
        .filter(|(_, v)| v.as_i64().is_some())
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

fn write_ttl_table(all: &mut Map<String, Value>, ttl: Map<String, Value>) {
    // 表空了就把保留键本身也删掉 —— 没用过 TTL 的部署不该在 `store.json` 里
    // 永远背着一个空对象。
    if ttl.is_empty() {
        all.remove(TTL_TABLE_KEY);
    } else {
        all.insert(TTL_TABLE_KEY.to_string(), Value::Object(ttl));
    }
}

fn is_expired(all: &Map<String, Value>, key: &str, now: i64) -> bool {
    read_ttl_table(all)
        .get(key)
        .and_then(Value::as_i64)
        .is_some_and(|exp| exp <= now)
}

/// 顺手清掉 TTL 表里所有已过期的键。
///
/// 🔴 **每次写都扫一遍整张表，不止清这次写的那一把**：
/// 旧方案只 delete 一个按固定偏移算出来的键，稀疏落盘（间隔超过保留期）或多副本
/// 各写各的随机分片时，那个算出来的键很可能压根不是「真正该被清掉的那个」，
/// **清理率归零**。改成这样之后，**只要有任何一次写发生**，所有过期键一起清 ——
/// 键数由这张表本身保证有界，不依赖「落盘节奏恰好规律」这种前提。
fn prune_expired(all: &mut Map<String, Value>, ttl: &mut Map<String, Value>, now: i64) {
    let dead: Vec<String> = ttl
        .iter()
        .filter(|(_, v)| v.as_i64().is_some_and(|exp| exp <= now))
        .map(|(k, _)| k.clone())
        .collect();
    for k in dead {
        all.remove(&k);
        ttl.remove(&k);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_tmp;
    use serde_json::json;

    fn store(dir: &Path) -> FileStorage {
        FileStorage::new(dir)
    }

    /// 最基本的读写删。
    #[tokio::test]
    async fn round_trips_values() {
        let dir = test_tmp::dir("storage-roundtrip");
        let s = store(&dir);
        assert_eq!(s.get("k").await.unwrap(), None, "没写过就该是 None");
        s.put("k", json!({ "a": 1 }), None).await.unwrap();
        assert_eq!(s.get("k").await.unwrap(), Some(json!({ "a": 1 })));
        s.delete("k").await.unwrap();
        assert_eq!(s.get("k").await.unwrap(), None);
    }

    /// 删一把不存在的键不是错误（删除是幂等的）。
    #[tokio::test]
    async fn deleting_a_missing_key_is_not_an_error() {
        let dir = test_tmp::dir("storage-del-missing");
        store(&dir).delete("nope").await.unwrap();
    }

    /// `list` 按前缀过滤，且**不回 TTL 保留键**。
    ///
    /// 回归对象：保留键漏出去会被上层当成一条业务记录
    /// （`list("")` 的调用方会拿它去 `get` 然后解析失败）。
    #[tokio::test]
    async fn list_filters_by_prefix_and_hides_the_reserved_key() {
        let dir = test_tmp::dir("storage-list");
        let s = store(&dir);
        s.put("key:a", json!(1), None).await.unwrap();
        s.put("key:b", json!(2), Some(now_ms() + 600_000))
            .await
            .unwrap();
        s.put("pool:index", json!({}), None).await.unwrap();

        let mut keys = s.list("key:").await.unwrap();
        keys.sort();
        assert_eq!(keys, vec!["key:a".to_string(), "key:b".to_string()]);

        let mut all = s.list("").await.unwrap();
        all.sort();
        assert_eq!(
            all,
            vec![
                "key:a".to_string(),
                "key:b".to_string(),
                "pool:index".to_string()
            ],
            "TTL 保留键不许出现在 list 里"
        );
    }

    /// 🔴 **过期之后 `get` 与 `list` 都看不见**（端口契约里最硬的一条）。
    #[tokio::test]
    async fn expired_keys_are_invisible_to_both_get_and_list() {
        let dir = test_tmp::dir("storage-ttl-invisible");
        let s = store(&dir);
        s.put("event:old", json!(1), Some(now_ms() - 1))
            .await
            .unwrap();
        s.put("event:new", json!(2), Some(now_ms() + 600_000))
            .await
            .unwrap();

        assert_eq!(
            s.get("event:old").await.unwrap(),
            None,
            "过期的 get 必须看不见"
        );
        assert_eq!(s.get("event:new").await.unwrap(), Some(json!(2)));
        assert_eq!(
            s.list("event:").await.unwrap(),
            vec!["event:new".to_string()]
        );
    }

    /// `expires_at` 早于当下：**put 完立刻就读不到了**。
    ///
    /// 这一格看着荒谬，但它是真实存在的形态：两个组件用了不同的时钟基准时，
    /// 会造出「生下来就过期」的键。契约必须对这种情况有确定行为，而不是未定义。
    #[tokio::test]
    async fn already_expired_on_write_is_immediately_invisible() {
        let dir = test_tmp::dir("storage-ttl-born-dead");
        let s = store(&dir);
        s.put("k", json!(1), Some(now_ms() - 10_000)).await.unwrap();
        assert_eq!(s.get("k").await.unwrap(), None);
    }

    /// 🔴 过期键必须从**落盘文件**里**物理删掉**，不是只在读时跳过。
    ///
    /// 只在读时跳过的话，`store.json` 会无界增长 —— 而有界性必须是存储自己的性质。
    #[tokio::test]
    async fn expired_keys_are_physically_removed_from_the_file() {
        let dir = test_tmp::dir("storage-ttl-physical");
        let s = store(&dir);
        s.put("event:old", json!("payload"), Some(now_ms() - 1))
            .await
            .unwrap();
        // 任何一次写都会顺手清一遍整张 TTL 表。
        s.put("other", json!(1), None).await.unwrap();

        let raw = std::fs::read_to_string(s.path()).unwrap();
        assert!(!raw.contains("event:old"), "过期键还留在落盘文件里:\n{raw}");
        assert!(!raw.contains("payload"), "过期键的值还留在落盘文件里");
        assert!(raw.contains("other"));
    }

    /// TTL 表空了之后，保留键本身也要从文件里消失。
    #[tokio::test]
    async fn ttl_table_key_disappears_when_empty() {
        let dir = test_tmp::dir("storage-ttl-empty");
        let s = store(&dir);
        s.put("k", json!(1), Some(now_ms() + 600_000))
            .await
            .unwrap();
        assert!(
            std::fs::read_to_string(s.path())
                .unwrap()
                .contains(TTL_TABLE_KEY)
        );
        // 重写同一把键、这次不带 TTL ⇒ 表该空了。
        s.put("k", json!(1), None).await.unwrap();
        let raw = std::fs::read_to_string(s.path()).unwrap();
        assert!(
            !raw.contains(TTL_TABLE_KEY),
            "空的 TTL 表不该留在文件里:\n{raw}"
        );
    }

    /// 🔴 **坏文件绝不静默当空。**
    ///
    /// 回归对象（事故链）：读失败被吞成「空」⇒ 进程带着空池启动 ⇒ 运维重新导入 ⇒
    /// 原子落盘用空池整份覆写 ⇒ **原来那一池 key 不可逆蒸发**。
    #[tokio::test]
    async fn a_corrupt_file_fails_loudly_instead_of_reading_as_empty() {
        let dir = test_tmp::dir("storage-corrupt");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("store.json"), b"{ this is not json").unwrap();
        let s = store(&dir);
        assert!(s.get("anything").await.is_err(), "坏文件必须如实报错");
        assert!(s.list("").await.is_err());
    }

    /// 顶层不是 JSON 对象（比如是个数组）—— 同样算「解析不动」。
    #[tokio::test]
    async fn a_non_object_top_level_is_also_corrupt() {
        let dir = test_tmp::dir("storage-corrupt-array");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("store.json"), b"[1,2,3]").unwrap();
        assert!(store(&dir).get("k").await.is_err());
    }

    /// 🔴 **落盘自保阀**：磁盘上的文件坏了时，写根本走不到落盘那一步 ——
    /// 于是坏文件不会被一次写覆盖掉，还留着给人工挽救。
    #[tokio::test]
    async fn a_write_never_clobbers_a_corrupt_file() {
        let dir = test_tmp::dir("storage-selfguard");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("store.json");
        std::fs::write(&path, b"{ broken").unwrap();

        let s = store(&dir);
        assert!(s.put("k", json!(1), None).await.is_err());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "{ broken",
            "原件必须原封不动"
        );
    }

    /// `preflight`：坏文件被**原样备份**，且**原件不动**。
    #[tokio::test]
    async fn preflight_backs_up_a_corrupt_file_and_leaves_the_original() {
        let dir = test_tmp::dir("storage-preflight");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("store.json");
        std::fs::write(&path, b"{ broken").unwrap();

        let s = store(&dir);
        let StoreIntegrity::Corrupt { backup } = s.preflight().await else {
            panic!("坏文件必须被判成 Corrupt");
        };
        assert_ne!(backup, path, "备份不能就是原件本身");
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), "{ broken");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "{ broken",
            "原件必须留下"
        );
    }

    /// `preflight`：全新部署（文件不存在）是 `Missing`，不是 `Corrupt`。
    ///
    /// 混成一种的话，每一次全新部署的日志里都会出现一条「数据文件损坏」，
    /// 而那正是运维最不该收到的假警报。
    #[tokio::test]
    async fn preflight_distinguishes_missing_from_corrupt() {
        let dir = test_tmp::dir("storage-preflight-missing");
        assert_eq!(store(&dir).preflight().await, StoreIntegrity::Missing);

        let s = store(&dir);
        s.put("k", json!(1), None).await.unwrap();
        assert_eq!(s.preflight().await, StoreIntegrity::Ok);
    }

    /// 孤儿 tmp 的回收：**只删 mtime 早于 5 分钟的**，新的那个必须留着 ——
    /// 它可能是另一个副本**正在写**的文件。
    #[tokio::test]
    async fn orphan_tmp_cleanup_spares_recent_files() {
        let dir = test_tmp::dir("storage-orphan");
        std::fs::create_dir_all(&dir).unwrap();
        let fresh = dir.join("store.json.tmp.999.0");
        let stale = dir.join("store.json.tmp.999.1");
        std::fs::write(&fresh, b"x").unwrap();
        std::fs::write(&stale, b"x").unwrap();
        // 把 stale 的 mtime 推到 10 分钟前。
        let ten_min_ago = SystemTime::now() - Duration::from_secs(600);
        filetime_set(&stale, ten_min_ago);

        let removed = store(&dir).cleanup_orphan_tmp().await;
        assert_eq!(removed, 1);
        assert!(
            fresh.exists(),
            "刚写的 tmp 可能是别的副本正在用的，绝不许删"
        );
        assert!(!stale.exists());
    }

    /// 并发写不许丢更新：20 个并发 `put` 之后，20 把键必须全在。
    ///
    /// 回归对象：「读全量 → 改一个键 → 写全量」三步跨 await，不串行化就会交错，
    /// 后写的覆盖先写的 —— 而转发路径正是在返回成功响应**之前**写回 key 状态。
    #[tokio::test]
    async fn concurrent_writes_do_not_lose_updates() {
        let dir = test_tmp::dir("storage-concurrent");
        let s = std::sync::Arc::new(store(&dir));
        let mut tasks = Vec::new();
        for i in 0..20 {
            let s = s.clone();
            tasks.push(tokio::spawn(async move {
                s.put(&format!("key:{i}"), json!(i), None).await.unwrap();
            }));
        }
        for t in tasks {
            t.await.unwrap();
        }
        assert_eq!(s.list("key:").await.unwrap().len(), 20);
    }

    // ── 落盘权限 ───────────────────────────────────────────────────────────

    /// 文件当前的权限位（低 9 位）。
    #[cfg(unix)]
    fn mode_of(p: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(p).unwrap().permissions().mode() & 0o777
    }

    #[cfg(unix)]
    fn perms(mode: u32) -> std::fs::Permissions {
        use std::os::unix::fs::PermissionsExt;
        std::fs::Permissions::from_mode(mode)
    }

    /// 🔴 **`store.json` 只许属主读写，且覆盖写之后也不许被放宽。**
    ///
    /// 这份文件里装着三样明文凭据：`key:<id>.key`（上游 Agnes key，设计上必须存明文）、
    /// `apikeys.keys[].secret`（已签发的 `sk-` 子密钥）、`config.gatewayToken`（网关口令）。
    /// 数据目录 bind mount 到宿主共享目录是**本仓自己在 `health.rs` 的
    /// `UNWRITABLE_DETAIL` 里承认的常见部署形态** —— 那时同宿主上任意一个非 root
    /// 本地用户 `cat` 一次就拿走全部凭据，不需要任何凭据、也不留任何事件。
    ///
    /// 回归对象：`write_all_sync` 曾用 `std::fs::File::create` 建 tmp
    /// （`0666 & ~umask`，默认 umask 022 下 = `0644`），而 `rename` 保留 tmp 的权限位。于是
    /// ① 每一次落盘都把这份文件放成组内 + 全局可读；
    /// ② **运维手动 `chmod 600` 过之后，下一次 `put` 会把它静默放宽回去。**
    ///
    /// ⚠️ 第二段断言**同时**是 tmp 文件权限的判据：rename 把 tmp 的权限位带到目标上，
    /// 目标是 `0600` 就证明 rename 那一刻的 tmp 也是 `0600`。
    /// 直接去 `stat` tmp 拿不到稳定结论 —— 它的存活窗口是亚毫秒级的，那会是一格 flaky。
    #[cfg(unix)]
    #[tokio::test]
    async fn the_store_file_is_owner_only_and_stays_that_way_across_overwrites() {
        let dir = test_tmp::dir("storage-perm");
        let s = store(&dir);
        s.put("key:a", json!({ "key": "agnes-plaintext" }), None)
            .await
            .unwrap();
        assert_eq!(
            mode_of(s.path()),
            0o600,
            "首次落盘就必须仅属主可读写（里面是上游 key 明文 + 已签发的 sk- 明文）"
        );

        // 运维手动收紧过（这里显式复现那个动作）—— 下一次写不许把它放宽回去。
        std::fs::set_permissions(s.path(), perms(0o600)).unwrap();
        s.put("key:b", json!({ "key": "another" }), None)
            .await
            .unwrap();
        assert_eq!(
            mode_of(s.path()),
            0o600,
            "覆盖写后权限不得被放宽，实际 {:o}",
            mode_of(s.path())
        );
    }

    /// 🔴 **坏文件的 `.corrupt.*` 备份同样只许属主读写。**
    ///
    /// 回归对象：`preflight_sync` 曾用 `std::fs::copy`，而它**把源文件的权限位一并复制**。
    /// 于是升级前留下的 `0644` 源文件会生出一份同样 `0644` 的副本 —— 内容是一字不差的
    /// 全部明文凭据，而且它是**此后任何原子写都覆不掉、也没有任何人会再去 chmod 的**
    /// 那一份，会一直躺在数据目录里。
    #[cfg(unix)]
    #[tokio::test]
    async fn the_corrupt_backup_is_owner_only_even_when_the_original_was_not() {
        let dir = test_tmp::dir("storage-perm-corrupt");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("store.json");
        std::fs::write(&path, b"{ broken \"gatewayToken\": \"s3cret\"").unwrap();
        // 历史遗留的宽权限源文件 —— `fs::copy` 正是在这里把权限带给副本的。
        std::fs::set_permissions(&path, perms(0o644)).unwrap();

        let StoreIntegrity::Corrupt { backup } = store(&dir).preflight().await else {
            panic!("坏文件必须被判成 Corrupt");
        };
        assert_ne!(backup, path, "备份不能就是原件本身");
        assert_eq!(
            mode_of(&backup),
            0o600,
            "备份装着与原件一模一样的全部明文凭据，实际 {:o}",
            mode_of(&backup)
        );
    }

    /// 升级路径：旧版本落下的 `0644` `store.json` 在**启动体检时**就地收紧。
    ///
    /// 为什么需要这一格：权限只在 `rename` 时跟着 tmp 走，而一个只读流量的副本可能
    /// 几天都不写一次 —— 「升级即修复」不能建立在「恰好有人写过一次」这个前提上。
    ///
    /// ⚠️ 收紧是 **best-effort**：只读挂载 / 文件属主不是本进程时改不动，
    /// 那时只是少收紧一个文件，**绝不许让启动失败**。
    #[cfg(unix)]
    #[tokio::test]
    async fn preflight_tightens_a_legacy_world_readable_store_file() {
        let dir = test_tmp::dir("storage-perm-legacy");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("store.json");
        std::fs::write(&path, br#"{"key:a":{"key":"agnes-plaintext"}}"#).unwrap();
        std::fs::set_permissions(&path, perms(0o644)).unwrap();

        assert_eq!(store(&dir).preflight().await, StoreIntegrity::Ok);
        assert_eq!(
            mode_of(&path),
            0o600,
            "升级之后不许等到「恰好有人写过一次」才收紧，实际 {:o}",
            mode_of(&path)
        );
    }

    /// 把一个文件的 mtime 设到指定时刻。
    ///
    /// 手写而不是引 `filetime` crate：只用一次，且本仓是「能用 std + 手写几行解决的
    /// 就不引 crate」的取向。用 `utimensat(2)` 的 libc 绑定太重，这里退而用
    /// 「重建文件 + 依赖文件系统」不可行，所以走 `touch -d`——
    /// **它只在测试里跑**，生产路径一个字节都不碰。
    fn filetime_set(path: &Path, t: SystemTime) {
        let secs = t.duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs();
        let status = std::process::Command::new("touch")
            .arg("-d")
            .arg(format!("@{secs}"))
            .arg(path)
            .status()
            .expect("测试环境里必须有 touch");
        assert!(status.success());
    }
}
