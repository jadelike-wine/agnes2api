//! `AppState` —— **唯一的状态载体**。
//!
//! 三条规矩（照抄 kiro2api 的 `MessagesState`）：
//!
//! 1. **全 `Arc`，克隆廉价，`#[derive(Clone)]`**：axum 每个请求克隆一次 state。
//! 2. **协议组与管理组两组路由 `.with_state()` 同一份**，🔴 **不建第二份存储** ——
//!    面板写完 key 之后要让转发路径立刻看见，各拿各的实例等于面板的写入永远不生效。
//! 3. **每个字段上写清楚谁消费它、谁不消费。**
//!
//! # 🔴 「没接线」与「功能关着」在类型上就是两回事
//!
//! `Option<T>` 表示的一律是 **「装配不全 / 这个部署根本没有这样东西」**，
//! 它的 HTTP 面是 `503 not_wired`（或注册机那三条的 `409`）。
//! **「功能关着」不用 `Option` 表达** —— 那是 `200 + tier:"off"`。
//! 两者混用会让面板把「运维没打开统计」渲染成「后端坏了」。
//!
//! 🔴 而且**两者都不许变成「这条路由不存在」**：路由表绝不许随运行时配置变化。

use crate::adapters::logger_store::StoreLogger;
use crate::core::keypool::LbMode;
use crate::core::storage_health::StorageHealth;
use crate::pool::repo::KeyPoolRepo;
use crate::ports::fetcher::Fetcher;
use crate::ports::logger::Logger;
use crate::ports::storage::Storage;
use crate::server::holders::apikey_holder::ApiKeyHolder;
use crate::server::holders::config_holder::ConfigHolder;
use parking_lot::Mutex;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

/// 全应用共享的状态。
#[derive(Clone)]
pub struct AppState {
    /// 存储。
    ///
    /// **消费者**：key 池、配置、对外密钥、事件、用量。
    /// 🔴 **`/health` 不消费它** —— 那条端点零存储 I/O，靠的是下面的 `health`。
    pub storage: Arc<dyn Storage>,

    /// 存储可写性的进程内状态。
    ///
    /// **消费者**：`/health`（只读一个布尔）、将来的 `overview`。
    /// **写入者**：启动探测 + 包了一层的存储装饰器上每一次**真实的写**。
    pub health: Arc<StorageHealth>,

    /// 结构化事件出口（已经是扇出之后的那一个）。
    ///
    /// **消费者**：全体。**不消费者**：`core` 下的纯函数（它们把要打的事件
    /// 当作返回值交上来，由边界上这一层打出去）。
    pub logger: Arc<dyn Logger>,

    /// 事件落库的那个 sink 本体。
    ///
    /// **唯一消费者**：`log_flush` 中间件（它要调 `maybe_flush()`）与
    /// 将来的 `GET /admin/api/events`。
    /// ⚠️ 它**不是**用来打日志的（打日志走 `logger`，那一个已经扇出到它了）——
    /// 拿它打会绕过 console，运维的第一现场就少一半。
    pub event_sink: Arc<StoreLogger>,

    /// 运行期配置**持有者**（不是一份拷贝值）。
    ///
    /// 🔴 **绝不许在装配时把配置值展开成一堆字段**：那正是历史上
    /// 「改完配置网关照旧按旧值跑」的成因。每个请求开头 `current()` 取**一份快照**，
    /// 单个请求内部保持一致。
    pub config: Arc<ConfigHolder>,

    /// 子密钥表持有者。`None` = **没接线**（6 条 apikeys 端点回 `503 not_wired`，
    /// 且鉴权退化成「只认主口令」—— 后者是**正常形态**，不是故障）。
    ///
    /// **消费者**：只有鉴权第②段。🔴 它**刻意不挂在任何全局 layer 上**。
    pub api_keys: Option<Arc<ApiKeyHolder>>,

    /// key 池。**面板写完 key 要让这一个实例失效**，另建一个等于空操作。
    pub repo: Arc<KeyPoolRepo>,

    /// 出站 HTTP。**唯一的持有者是 `pool::dispatcher`** 与将来的探测端点。
    pub fetcher: Arc<dyn Fetcher>,

    /// 选 key 的轮询游标（`LbMode::Balanced`）。
    ///
    /// **每进程一份**，与 TS 的模块级游标逐字同语义。
    /// 🔴 连带约束：**面板不许展示「下一把 key / 轮换顺序」** ——
    /// 多副本部署下各进程各有各的游标，展示了就是给一个与实际不符的值。
    pub key_cursor: Arc<AtomicUsize>,

    /// 粘滞选 key（`LbMode::Priority`）记的「当前这把」。同样是每进程一份。
    pub sticky_key: Arc<Mutex<Option<String>>>,

    /// 负载均衡模式。**默认 `Priority`**。
    ///
    /// ⚠️ 切默认值是一次**行为变更**（TS 版是游标轮询），要写进 CHANGELOG
    /// 与五语言文档，不是「移植的顺手改进」。
    pub lb_mode: LbMode,

    /// 管理口令。`None` = **没配 / 不合规 ⇒ 整棵 `/admin` 树不注册（404）**。
    ///
    /// ⚠️ 这是「路由表不随运行时配置变化」那条铁律**唯一**的例外，
    /// 而它是装配期的一次性判定（不是运行期开关）：不泄漏「这里有个后台」。
    pub admin_token: Option<Arc<String>>,

    /// `TRUST_PROXY`。**唯一消费者**是 `client_ip`，而 `client_ip` 的唯一消费者是
    /// `admin.login_failed` 事件。**不参与鉴权、不参与限速。**
    pub trust_proxy: bool,

    /// Tier-2 用量 sink。
    ///
    /// 🔴 **「关 = sink 根本不存在」，不是「建好再用 `if` 拦住写」。**
    /// 这条形态差别是可观测的：关着时**零存储调用、零累加器**。
    /// ⚠️ 它为 `None` 时用量端点照常 **200 + `tier:"off"`**，🔴 不许回 503。
    pub usage_sink: Option<Arc<crate::server::usage_sink::UsageSink>>,

    /// 注册机的运行期。**没装起来就是 `None`。**
    ///
    /// 🔴 `config-degrade` 那条事故判据（真机 74% 请求 500）钉的是
    /// **注册机装不起来时网关照常活着** —— 它必须是一条断言，不是一句注释。
    /// 所以这里是 `Option` 而不是「一个装着默认值的结构体」：
    /// **没有，就如实地没有**，转发路径一个字都感知不到它。
    pub registrar: Option<Arc<crate::registrar::runtime::RegistrarRuntime>>,
    /// 补池的**进程内**那道闸。
    ///
    /// ⚠️ **它与跨副本那把存储锁不是冗余**：一把挡同进程重入
    ///（面板连点、定时轮撞上按钮），一把挡跨副本重叠。
    /// 少了进程内这道，同一个进程里两条路会各自去抢那把没有 CAS 的存储锁，
    /// 而那正是存储锁挡不住的那一档。
    pub tend_in_flight: Arc<std::sync::atomic::AtomicBool>,
}

impl AppState {
    /// 注册机接线了没有。
    ///
    /// 🔴 **它是从 [`AppState::registrar`] 派生的，不是一个独立字段。**
    /// 从前这里真的有两个字段，而**两个字段各说各话**是这个仓反复裁过的毛病：
    /// 一个说「接线了」、另一个是 `None`，于是端点回 200 而 handler 里 `unwrap` 掉。
    /// 派生之后那种不一致**在结构上不存在**。
    pub fn registrar_wired(&self) -> bool {
        self.registrar.is_some()
    }
}

/// 占住[`AppState::tend_in_flight`]那道闸的租约。
///
/// 🔴 **放开挂在 `Drop` 上，不手写配对。**
/// 手写只覆盖得到「跑完了」「跑错了」两条支，**漏掉第三条：请求被取消**
/// —— 客户端一断开，axum 直接 drop 这个 future，那一行放开根本不执行，
/// 而这道闸**不会自愈**：那颗按钮从此再也点不动，且面板上看不出原因。
pub struct TendInFlight(Arc<std::sync::atomic::AtomicBool>);

impl TendInFlight {
    /// 占住它。已经被占着就回 `None`。
    pub fn enter(flag: &Arc<std::sync::atomic::AtomicBool>) -> Option<Self> {
        use std::sync::atomic::Ordering;
        // `compare_exchange` 而不是「先读一句忙不忙、再去占」——
        // 后者两步之间有窗口，而这道闸存在的全部理由就是关掉那个窗口。
        flag.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| Self(Arc::clone(flag)))
    }
}

impl Drop for TendInFlight {
    fn drop(&mut self) {
        self.0.store(false, std::sync::atomic::Ordering::Release);
    }
}
