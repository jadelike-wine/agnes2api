//! 「TTL + 失效 + 加载失败保留上一份合法快照」的最小原语（`refreshable.ts`，01 §4.6）。**零 IO。**
//!
//! 它同时是 `ConfigHolder`（配置）与 `pool/repo.rs` 池快照的底座 ——
//! 两者回答的是**同一个问题**「刚改完，什么时候能看见？」，
//! 而对用户承诺的生效时间只有一份。**各写一份实现必然漂移出两套语义**，
//! 那时面板上写的数字和实际行为就对不上了。
//!
//! # 🔴 一处对 TS 形状的刻意偏离：**在途去重（`inFlight`）不在这里**
//!
//! TS 版的 `Refreshable` 自己持有一个 `Promise`，于是「并发调用共享同一次加载」
//! 也在这个类里。Rust 侧做不到**且不该做**：那要求本类型持有一个 future，
//! 而 `core` 是零 IO 岛（不许依赖 `tokio`，也不许知道加载是异步的）。
//!
//! ⇒ **本类型只做状态机**：什么时候该重载、这次结果还算不算数、失败之后计时推不推进。
//! 真正的加载与在途去重由持有者（P1-b 的 `server/holders/config_holder.rs` 与
//! `pool/repo.rs`）用 `tokio::sync::Mutex` / `OnceCell` 做。
//!
//! 🔴 **这不是可以省掉的一步，欠账在此明记**：不做在途去重的话，
//! 突发流量下 N 个并发请求就是 **N 次存储读取**，
//! 而「读取次数与请求数解耦」正是整个快照缓存存在的理由。
//! 持有者那一侧必须有一格判据钉住「M 个并发 `ensure_fresh` 只产生 1 次 `load`」。
//!
//! # 调用方的标准用法（三步，顺序不许换）
//!
//! ```text
//! if r.needs_reload(now, ttl) {
//!     let token = r.begin_reload();          // ① 先取样代数
//!     match load().await {                   // ② 再去做 IO（此间可能有人 invalidate）
//!         Ok(v)  => r.complete_ok(token, v, now2),
//!         Err(e) => { log(e); r.complete_err(token, now2); }
//!     }
//! }
//! let snapshot = r.current();
//! ```
//! ⚠️ **`begin_reload` 必须在 IO 之前调**。放到 IO 之后取样，代数就永远等于自己，
//! [`invalidate`](Refreshable::invalidate) 会被在途加载静默吃掉 —— 那正是 `gen` 要挡的那件事。

/// 一次重载的「取样代数」。
///
/// 做成 newtype 而不是裸 `u64`，是为了让「把 `begin_reload` 的返回值原样交回来」
/// 成为唯一写得出来的用法 —— 随手传一个 `0` 进去会让 `gen` 这道闸整个失效，
/// 而那种失效**没有任何现象**（直到某次删除被在途加载盖回来）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReloadToken(u64);

/// TTL 快照缓存的状态机。**零 IO**：时间从参数进来，加载由调用方做。
#[derive(Debug)]
pub struct Refreshable<T> {
    value: Option<T>,
    /// 曾经成功装载过（或被 [`set`](Refreshable::set) 过）。
    /// 与 `loaded_at` **分开**，因为 [`invalidate`](Refreshable::invalidate) 只重置后者。
    ever_loaded: bool,
    /// 上一次**真加载**的时刻。
    ///
    /// `None` 就是 TS 那边的 `-Infinity`：`now - (-∞) = +∞` ⇒
    /// 「回拨判据（`age >= 0`）成立、TTL 判据（`age < ttl`）不成立」
    /// ⇒ **冷启动照常触发真加载**，且不会被误判成时钟回拨。
    loaded_at: Option<i64>,
    generation: u64,
}

impl<T> Default for Refreshable<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Refreshable<T> {
    pub const fn new() -> Self {
        Self {
            value: None,
            ever_loaded: false,
            loaded_at: None,
            generation: 0,
        }
    }

    /// 同步读当前快照。**永不失败**；从未装载过时是 `None`。
    pub fn current(&self) -> Option<&T> {
        self.value.as_ref()
    }

    /// 从来没有过任何一份快照（连失败兜底都没有）。
    pub fn is_empty(&self) -> bool {
        !self.ever_loaded
    }

    /// 当前失效代数。**只给判据与排障用**，业务不许据此分支。
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// 该不该真的去加载一次。**这是 `ensureFresh` 的判据那一半。**
    ///
    /// ⚠️ `age < 0` = **时钟回拨（NTP）**。不挡的话 `age < ttl` 恒成立，
    /// 这个 Refreshable 会**冻结「回拨量 + 一个 TTL」那么久**。
    /// 本仓有五处「`age < 0` 立刻恢复」的同一套语义
    /// （`Refreshable` / `list_on_read_path` / `write_index_best_effort` /
    /// `pending_is_stale` / 写预算的 `in_window`）——
    /// **同一份代码库里不许有两套时钟语义。**
    ///
    /// `ttl_ms <= 0` ⇒ 恒为真（= 关闭缓存，每次都真读），与 TS 的 `age < 0ms` 逐字同义。
    pub fn needs_reload(&self, now: i64, ttl_ms: i64) -> bool {
        let Some(at) = self.loaded_at else {
            return true;
        };
        let age = now.saturating_sub(at);
        !(self.ever_loaded && age >= 0 && age < ttl_ms)
    }

    /// 取样代数。**必须在真正发起加载之前调**（见模块头那三步）。
    pub fn begin_reload(&self) -> ReloadToken {
        ReloadToken(self.generation)
    }

    /// 加载成功。
    ///
    /// **快照无条件更新**（它是手上最新的一次取样，比更早那份兜底强），
    /// 但**代数变了就不推进计时** ⇒ 下一次 [`needs_reload`](Refreshable::needs_reload)
    /// 一定为真。于是「装载途中被 `invalidate`」的陈旧窗口被压到
    /// **「已经在途的那一个请求」**，而不是一整个 TTL。
    pub fn complete_ok(&mut self, token: ReloadToken, value: T, now: i64) {
        self.value = Some(value);
        self.ever_loaded = true;
        if token.0 == self.generation {
            self.loaded_at = Some(now);
        }
    }

    /// 加载失败。**上一份合法快照原样保留。**
    ///
    /// **失败也推进计时，但只在已有兜底快照时**：
    /// - **有兜底**：故障期每个请求都重试等于把存储打爆，而这正是「读不出来」时最不该做的事；
    ///   代价是失败后最多再等一个 TTL 才重试。
    /// - **无兜底（冷启动就失败）**：**不推进**，让下一次调用继续尝试 ——
    ///   否则冷启动撞一次抖动就要空等满一个 TTL，期间 `current()` 一直是 `None`。
    /// - **装载途中被 `invalidate` 过同样不推进**：面板刚改完就撞上一次读失败，
    ///   不该因此再等满一个 TTL 才看见新值。
    ///
    /// ⚠️ 失败回调（记事件）由调用方做 —— 它是 IO。
    /// TS 那边把 `onError` 包在 `try/catch` 里（sink 自身故障不许影响主流程），
    /// **Rust 侧那条纪律搬到调用方：日志 sink 抛错不许冒泡到转发路径。**
    pub fn complete_err(&mut self, token: ReloadToken, now: i64) {
        if self.ever_loaded && token.0 == self.generation {
            self.loaded_at = Some(now);
        }
    }

    /// 强制装载一次的结果（`prime`）。供启动时的 fail-closed 路径用。
    ///
    /// 与 [`complete_ok`](Refreshable::complete_ok) 的区别：它**无条件**推进计时
    /// （启动路径上没有「在途的另一次加载」这回事）。
    /// 🔴 「失败就拒绝启动」那一半在调用方：`prime` 只在成功时被调到。
    pub fn prime(&mut self, value: T, now: i64) {
        self.value = Some(value);
        self.ever_loaded = true;
        self.loaded_at = Some(now);
    }

    /// 下一次一定真的重载。**面板写操作成功后调用。**
    ///
    /// 🔴 **`gen` 不递增的话 `invalidate()` 会被在途 reload 静默吃掉**：
    /// 一次装载是一个 IO 往返（先取样、后交付），期间跑完的 `delete()` 已经把记录删掉、
    /// `invalidate()` 也真调了，可紧接着落地的那次 reload 会把**删除之前**那份快照
    /// 原样盖回来并推进计时 ⇒ **已删的 key 在快照里活满一个 TTL**。
    /// 对配置就是「**已撤销的口令再活一个 TTL**」。
    pub fn invalidate(&mut self) {
        self.loaded_at = None;
        self.generation = self.generation.wrapping_add(1);
    }

    /// 就地替换快照而**不产生任何读取**（写穿透）。
    ///
    /// - **刻意不推进计时**：推进了就意味着一个持续写入的实例**永远不会再去读存储**，
    ///   于是它永远看不到别的实例（或面板）的改动。写穿透只保证「我自己写的我立刻看得见」，
    ///   不该顺带延长「别人写的我多久能看见」的上界。
    /// - 🔴 **刻意不递增 `gen`（与 [`invalidate`](Refreshable::invalidate) 不同，这不是省略）**：
    ///   写穿透最密集的时刻恰恰是上游出事的时刻 —— 429 突发 ⇒ 一批 key 同时进冷却
    ///   ⇒ 一批 `save() + set()`。递增代数就意味着每次写穿透都作废掉正在途中的那次装载
    ///   并立刻重来，于是**在最需要缓存的时候引发 reload 风暴**。
    ///
    ///   代价：在途 reload 落地时会盖掉刚写穿透的那份，该状态在**本实例的快照**里
    ///   最多晚一个 TTL 才可见（**存储里始终是对的**）。
    ///   根据是两者承诺强度不同：`invalidate()` 的「下一次一定重载」是**硬保证**，
    ///   写穿透只是「我自己写的我立刻看得见」的尽力优化 —— 只有硬保证值得付那个代价。
    pub fn set(&mut self, value: T) {
        self.value = Some(value);
        self.ever_loaded = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TTL: i64 = 60_000;
    const T0: i64 = 1_700_000_000_000;

    fn loaded(now: i64) -> Refreshable<&'static str> {
        let mut r = Refreshable::new();
        let tok = r.begin_reload();
        r.complete_ok(tok, "v1", now);
        r
    }

    /// 冷启动：没有快照、必须重载。
    #[test]
    fn a_cold_instance_always_wants_to_load() {
        let r: Refreshable<&str> = Refreshable::new();
        assert!(r.is_empty());
        assert_eq!(r.current(), None);
        assert!(r.needs_reload(T0, TTL));
    }

    /// TTL 之内命中缓存；**恰好等于 TTL 的那一刻要重载**（`age < ttl`，不是 `<=`）。
    #[test]
    fn ttl_boundary_is_exclusive() {
        let r = loaded(T0);
        assert!(!r.needs_reload(T0, TTL));
        assert!(!r.needs_reload(T0 + TTL - 1, TTL));
        assert!(r.needs_reload(T0 + TTL, TTL), "age == ttl 就该重载");
        assert!(r.needs_reload(T0 + TTL + 1, TTL));
    }

    /// `ttl <= 0` = 关闭缓存，每次都真读。
    #[test]
    fn a_non_positive_ttl_disables_caching() {
        let r = loaded(T0);
        assert!(r.needs_reload(T0, 0));
        assert!(r.needs_reload(T0, -1));
    }

    /// 🔴 **时钟回拨立刻恢复刷新，不许冻结「回拨量 + 一个 TTL」那么久。**
    #[test]
    fn a_backwards_clock_does_not_freeze_the_cache() {
        let r = loaded(T0);
        assert!(
            r.needs_reload(T0 - 3_600_000, TTL),
            "age < 0 必须当场判定要重载"
        );
    }

    /// 加载成功后快照可读、计时推进。
    #[test]
    fn a_successful_load_publishes_the_snapshot_and_starts_the_clock() {
        let mut r: Refreshable<&str> = Refreshable::new();
        let tok = r.begin_reload();
        r.complete_ok(tok, "v1", T0);
        assert_eq!(r.current(), Some(&"v1"));
        assert!(!r.is_empty());
        assert!(!r.needs_reload(T0 + 1, TTL));
    }

    /// 🔴 **失败保留上一份合法快照**，绝不退化成 `None`。
    #[test]
    fn a_failed_reload_keeps_the_previous_good_snapshot() {
        let mut r = loaded(T0);
        let tok = r.begin_reload();
        r.complete_err(tok, T0 + TTL);
        assert_eq!(r.current(), Some(&"v1"), "读不出来 ≠ 没有值");
    }

    /// **有兜底时失败也推进计时**：故障期每个请求都重试等于把存储打爆。
    #[test]
    fn failures_back_off_when_there_is_a_fallback() {
        let mut r = loaded(T0);
        let tok = r.begin_reload();
        r.complete_err(tok, T0 + TTL);
        assert!(!r.needs_reload(T0 + TTL + 1, TTL), "失败之后要退避一个 TTL");
    }

    /// **冷启动就失败时不推进**：否则撞一次抖动就要空等满一个 TTL，
    /// 而期间 `current()` 一直是 `None`（什么都服务不了）。
    #[test]
    fn a_cold_failure_retries_immediately() {
        let mut r: Refreshable<&str> = Refreshable::new();
        let tok = r.begin_reload();
        r.complete_err(tok, T0);
        assert!(r.needs_reload(T0, TTL), "没有兜底时必须立刻再试");
        assert!(r.is_empty());
    }

    /// `invalidate` 之后下一次一定重载。
    #[test]
    fn invalidate_forces_the_next_load() {
        let mut r = loaded(T0);
        assert!(!r.needs_reload(T0, TTL));
        r.invalidate();
        assert!(r.needs_reload(T0, TTL));
        assert_eq!(r.current(), Some(&"v1"), "失效不等于丢掉手上那份");
    }

    /// 🔴🔴 **`invalidate` 不许被在途 reload 静默吃掉**（`gen` 那道闸）。
    ///
    /// 时序：取样 → 期间面板删了东西并 `invalidate` → 在途那次落地。
    /// 落地那份**可以**发布（它是手上最新的一次取样），
    /// 但**绝不许推进计时** —— 推进了，已删的东西就在快照里活满一个 TTL。
    #[test]
    fn an_in_flight_load_cannot_swallow_an_invalidate() {
        let mut r = loaded(T0);
        let tok = r.begin_reload(); // ① 取样
        r.invalidate(); // ② 期间有人改了存储
        r.complete_ok(tok, "stale", T0 + 1); // ③ 在途那次落地
        assert!(
            r.needs_reload(T0 + 1, TTL),
            "🔴 陈旧窗口必须被压到「已经在途的那一个请求」，不是一整个 TTL"
        );
    }

    /// 同一条闸在失败路径上也成立。
    #[test]
    fn an_in_flight_failure_cannot_swallow_an_invalidate_either() {
        let mut r = loaded(T0);
        let tok = r.begin_reload();
        r.invalidate();
        r.complete_err(tok, T0 + 1);
        assert!(r.needs_reload(T0 + 1, TTL));
    }

    /// 写穿透：立刻看得见，**但不推进计时**（否则持续写入的实例永远看不到别人的改动）。
    #[test]
    fn write_through_is_visible_immediately_but_does_not_extend_the_ttl() {
        let mut r = loaded(T0);
        r.set("v2");
        assert_eq!(r.current(), Some(&"v2"));
        assert!(
            r.needs_reload(T0 + TTL, TTL),
            "写穿透不许顺带延长「别人写的我多久能看见」的上界"
        );
    }

    /// 🔴 **写穿透刻意不递增 `gen`**（与 `invalidate` 不同，这不是省略）。
    ///
    /// 回归对象：顺手补一行 `gen += 1` 会让每次写穿透都作废掉在途装载并立刻重来，
    /// 于是**在最需要缓存的时候（429 突发、一批 key 同时进冷却）引发 reload 风暴**。
    #[test]
    fn write_through_deliberately_does_not_bump_the_generation() {
        let mut r = loaded(T0);
        let before = r.generation();
        r.set("v2");
        assert_eq!(r.generation(), before, "写穿透不递增代数，这是刻意的");

        // 直接的后果：在途 reload 落地时**会**盖掉刚写穿透的那份，
        // 且照常推进计时。代价已在 `set` 的文档里明写。
        let tok = r.begin_reload();
        r.set("v3");
        r.complete_ok(tok, "from-storage", T0 + 1);
        assert_eq!(r.current(), Some(&"from-storage"));
        assert!(!r.needs_reload(T0 + 1, TTL));
    }

    /// 从未装载过时的 `set`：它也算「有了一份快照」。
    #[test]
    fn set_on_a_cold_instance_counts_as_having_a_snapshot() {
        let mut r: Refreshable<&str> = Refreshable::new();
        r.set("v1");
        assert!(!r.is_empty());
        assert_eq!(r.current(), Some(&"v1"));
        assert!(
            r.needs_reload(T0, TTL),
            "但计时没有开始过，所以照样要读一次"
        );
    }

    /// `prime` 无条件推进计时（启动路径上没有在途的第二次装载）。
    #[test]
    fn prime_starts_the_clock_unconditionally() {
        let mut r: Refreshable<&str> = Refreshable::new();
        r.prime("v1", T0);
        assert_eq!(r.current(), Some(&"v1"));
        assert!(!r.needs_reload(T0 + TTL - 1, TTL));
    }

    /// 代数递增用 `wrapping`：**绕回去只是多重载一次，而 panic 会打死整条路径。**
    #[test]
    fn generation_wraps_instead_of_panicking() {
        let mut r = loaded(T0);
        for _ in 0..3 {
            r.invalidate();
        }
        assert_eq!(r.generation(), 3);
    }
}
