//! `GET /admin/api/capabilities` —— **面板的形态出口**（02 §9.1）。
//!
//! 面板启动时调一次，所有形态分支读它 —— 🔴 **不许「这个部署有没有 X」散落进
//! 八个板块各写一次**，更不许在前端写死（写死的那一份会在后端改法的那天变成假话）。
//!
//! # 🔴 **零存储读**
//!
//! 全部来自内存（装配时算好的东西 + 进程内状态）。它是面板启动必调的第一个接口，
//! 让它去读一次存储就等于给每次刷新加一次存储读，而 `FileStorage` 的每次 `get`
//! 是整份 `store.json` 的读 + 解析。由 `it_never_touches_storage` 数着精确次数钉住。
//!
//! # ⚠️ 与五语言 API.md 那份响应样例的**两处出入**，以实现为准
//!
//! 1. **样例里有 `runtime.colo`，这里没有**。那是 Workers 给 `Request` 挂的非标准
//!    属性（请求打在哪个边缘机房），v0.4.0 摘掉 Worker 形态之后它恒为 `null` ——
//!    一个恒 null 的字段在面板上只会让人以为「这次没取到」。🔴 **不许加回来。**
//! 2. **样例整块缺 `apiKeys`**，而面板**靠它显隐整个对外密钥板块**
//!    （有没有「复制完整密钥」那颗按钮、「停用之后最多还能用多久」写几分钟、
//!    这个部署接没接那张表）。⇒ 实现必须有，文档那一侧的缺口另行登记。
//!
//! # ⚠️ 四格「只剩一个取值、且没有前端消费者」的，为什么还留着
//!
//! `runtime.name` / `storage.backend` / `quota.model` / `process.metrics` ——
//! 面板的运行时格子读的是 `GET /admin/api/overview` 的那一份，不是这里的。
//! 留着是**范围取舍**（删它们要同步改五语言 API.md 的响应样例），
//! **别把它们读成「形态分支的真源」**。真正还在分叉、真正有人读的是
//! `storage.writable` / `stats.*` / `apiKeys.*`。

use crate::core::config_provenance::EnvMap;
use crate::core::keys::api_keys::{API_KEY_NAME_MAX, APIKEY_MAX};
use crate::server::holders::apikey_holder::APIKEY_CACHE_TTL_MS;
use crate::server::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

// ── 常量（都是「今天只有一个取值」的形态格，写成常量是为了让改它必须先改这里）──

/// 注册用的子树内路径。🔴 `app.rs` 的 `.route()` 从这里取，不许手抄字面量。
pub const CAPABILITIES_SUBPATH: &str = "/capabilities";

/// 运行时名。
///
/// ⚠️ **这里是 `"rust"`，而五语言 API.md 的样例写的是 `"node"`** —— 那份样例描述的是
/// 被移植的 TS 版。面板的运行时格子会把这个串**直接画给运维看**，
/// 在一个 Rust 网关上报 `"node"` 是一句当场可证伪的假话。⇒ 如实报，文档缺口另行登记。
pub const RUNTIME_NAME: &str = "rust";

/// 存储后端。
///
/// 🔴 **按目标区分，不许一律报 `"file"`**：Worker 形态落的是 D1
/// （`adapters::storage_d1`），在面板上写「文件」是一句当场可证伪的假话 ——
/// 运维照着它去磁盘上找 `store.json` 会找不到。
#[cfg(not(target_arch = "wasm32"))]
pub const STORAGE_BACKEND: &str = "file";
#[cfg(target_arch = "wasm32")]
pub const STORAGE_BACKEND: &str = "d1";

/// 写配额模型。文件存储**没有**写配额，但每次写都重写整个 `store.json`。
/// Workers 形态下 D1 有免费层的行写预算（见 `docs/zh/WORKERS.md` 费用一节）。
#[cfg(not(target_arch = "wasm32"))]
pub const QUOTA_MODEL: &str = "file";
#[cfg(target_arch = "wasm32")]
pub const QUOTA_MODEL: &str = "d1_rows";

/// 进程内日志区。**恒 false，这仍是刻意选择。**
///
/// 逐请求日志在 Docker 部署上有一个现成且更好的去处 —— `docker logs`
/// （console logger 一条不落地往 stdout 打）。在面板里再攒一份内存日志，
/// 攒的是同一批行，代价是常驻内存 + 一个只有本进程看得见的第二真源。
pub const PROCESS_LOG: bool = false;

/// Tier-2 落盘间隔的**内置默认值**。
///
/// ⚠️ **它本该住在用量那一组**（`usage_sink` 落地时搬过去），暂住在这里的理由与
/// [`crate::admin::keys`] 文件头那三件「本该住在别处」的东西相同：
/// `stats.flushIntervalMs` 是**这条端点**的对外契约，而面板拿它算「未落盘的尾巴
/// 最长多久」—— 契约与产出它的那个数必须同一轮交付，否则面板会拿一个写死的数去说话。
///
/// 取 `86_400_000 / 12 = 7_200_000`：TS 版这个数由「每实例每天 12 次写预算」定死，
/// 写配额随 Worker 形态没了，**数字保留**（它同时是五语言文档里对用户的承诺）。
pub const USAGE_FLUSH_INTERVAL_MS: i64 = 7_200_000;

/// 落盘间隔的环境变量名。**只从环境变量读，一个字都不从存储读** ⇒
/// 它不进 `GatewayConfig`、不进配置面的四元组（没有 `stored` 那一格可以撒谎）。
pub const USAGE_FLUSH_INTERVAL_ENV: &str = "USAGE_FLUSH_INTERVAL_MS";

// ── 协议 ↔ 「网关看不看得到 token」──────────────────────────────────────────

/// 四条协议，以及**这条路径上网关解不解析响应体**（`expect_json` / `usage_path`）。
///
/// 🔴 [`tokens_coverage`] 由这张表算出来，**前端不许自己写一份**：
/// 它记的是「网关这条路径解不解析响应体」，不是「这条协议有没有 usage」。
///
/// ⚠️ **本该住在 `core::admin::protocol_catalog`**（`GET /admin/api/models` 那一组的
/// 真源，一个端点路径都不在前端硬编码）。那个文件本轮还不存在，而
/// `stats.tokensCoverage` 是**这条端点**的对外契约 ⇒ 暂住在这里。
/// 将来那张目录表落地时，这里整体删掉改成从它派生，**判据不必改**
/// （它们钉的是「哪几条协议在列」，不是谁实现的）。
///
/// ⚠️ **OpenAI 恒 `false` 不是漏**：它是唯一不传 `expect_json` 的一条
/// （见 `server::routes::openai` 文件头），网关从头到尾没有解析过它的响应体。
/// 🔴 别在热路径上补一次 `res.clone().json()` 去「修」它。
const PROTOCOL_USAGE: [(&str, bool); 4] = [
    ("openai", false),
    ("anthropic", true),
    ("responses", true),
    ("gemini", true),
];

/// 哪几条协议的 token 是网关看得到的。
///
/// ⚠️ **只从这一个出口发**：两条用量端点**都不许带它** ——
/// 同一份知识两个出口，前端就要面对「读哪一个」这个不该存在的问题。
#[must_use]
pub fn tokens_coverage() -> Vec<&'static str> {
    PROTOCOL_USAGE
        .iter()
        .filter(|(_, has_usage)| *has_usage)
        .map(|(id, _)| *id)
        .collect()
}

/// 生效的落盘间隔。
///
/// ⚠️ **报的必须是生效值**：运维经 `USAGE_FLUSH_INTERVAL_MS` 调过之后，
/// 这里报常量就等于面板对「尾巴最长多久」说了一句与实际不符的话。
///
/// ⚠️ **空串与「没设」同等对待**：`.env.example` 是给 `cp` + `env_file:` 直接用的，
/// 一个留空的键会以**空串**（不是 unset）进到环境里。
///
/// ⚠️ **非法值回落默认值，不拒绝启动** —— 与 TS 版**刻意不同**，理由登记在此：
/// TS 那一侧在装配时 `throw ConfigRefusal`，而本轮 `usage_sink` 结构性恒为 `None`
/// （Tier-2 整体是后面那一轮的对象），装配路径上**根本没有人读这个变量** ⇒
/// 把「拒绝启动」搬到一条**读端点**里是做不到的（handler 抛不出 `ConfigRefusal`）。
/// 两个可选项里，回落默认值 + 如实报出来，比在读端点上 500 诚实。
/// **Tier-2 接线那一轮必须把这条 fail-fast 补在装配侧**，届时这里退化成纯读。
#[must_use]
pub fn resolve_usage_flush_interval(env: &EnvMap) -> i64 {
    match env.get(USAGE_FLUSH_INTERVAL_ENV) {
        None => USAGE_FLUSH_INTERVAL_MS,
        Some(raw) if raw.trim().is_empty() => USAGE_FLUSH_INTERVAL_MS,
        Some(raw) => match raw.trim().parse::<i64>() {
            Ok(n) if n >= 1 => n,
            _ => USAGE_FLUSH_INTERVAL_MS,
        },
    }
}

// ── 线格式（字段顺序 = 序列化顺序，与五语言 API.md 的样例逐块对齐）────────────

#[derive(Serialize)]
struct RuntimeBlock {
    name: &'static str,
}

#[derive(Serialize)]
struct StorageBlock {
    backend: &'static str,
    writable: bool,
}

#[derive(Serialize)]
struct QuotaBlock {
    model: &'static str,
}

#[derive(Serialize)]
struct ProcessBlock {
    metrics: bool,
}

#[derive(Serialize)]
struct LogsBlock {
    #[serde(rename = "processLog")]
    process_log: bool,
}

#[derive(Serialize)]
struct StatsBlock {
    #[serde(rename = "tier2Enabled")]
    tier2_enabled: bool,
    #[serde(rename = "flushIntervalMs")]
    flush_interval_ms: i64,
    #[serde(rename = "tokensCoverage")]
    tokens_coverage: Vec<&'static str>,
}

/// 对外 API 密钥那一格。
///
/// 🔴 面板**靠它显隐整个板块里那几处形态分支**，一格都不许在前端写死。
#[derive(Serialize)]
pub struct ApiKeysCapability {
    /// 这个 app 接没接对外密钥的存储。为假时面板画一句「这个部署没接」，不画空列表。
    pub wired: bool,
    pub max: usize,
    #[serde(rename = "nameMax")]
    pub name_max: usize,
    /// 🔴 **「这个部署能不能取回明文」，不是「每一把都取得回来」**：
    /// 升级之前签发的记录没有明文，reveal 端点对它们如实回
    /// `reason: "issued_before_plaintext"`，🔴 **不许用掩码或空串冒充明文**。
    #[serde(rename = "plaintextRetrievable")]
    pub plaintext_retrievable: bool,
    /// **生效的**缓存 TTL。面板据它算「停用之后最多还能用多久」。
    #[serde(rename = "cacheTtlMs")]
    pub cache_ttl_ms: i64,
    /// 后端常量的默认值，面板在说明里要写清「运维没调过时是多少」。
    #[serde(rename = "defaultCacheTtlMs")]
    pub default_cache_ttl_ms: i64,
}

/// 出站探测那一格（验活 / 连通性测试 / 拉上游模型三条端点共用的那道闸）。
///
/// 🔴 **面板不许把这个数写死。** TS 版的 `js/pure/keys-write.mjs` 里有一个
/// `VERIFY_MIN_INTERVAL_MS = 3_000` 常量，靠一格 parity 判据与后端对表活着 ——
/// 那是「同一个数在两侧各存一份」的典型形态。06 §5.3 的 A 档把它收成一处：
/// **Rust 是唯一真源，面板读这一格。**
///
/// ⚠️ **相等只在「两边各自量一段时长」这个意义上成立**：面板从**发起**那一刻起算，
/// 后端从**收到**那一刻起算，中间隔着一次网络往返 ⇒ 面板的冷却总是**略早**到期，
/// 于是边界上仍然可能换回一次 `429 probe_cooldown`。那一次由面板如实翻译，
/// **不是**靠这个数字去消灭它。
#[derive(Serialize)]
pub struct ProbesCapability {
    /// 两次**发起**之间至少要隔这么久（毫秒）。
    #[serde(rename = "minIntervalMs")]
    pub min_interval_ms: i64,
}

/// 真正出网的那个形状。**字段顺序 = 序列化顺序**，见 `server::errors` 文件头那段。
#[derive(Serialize)]
struct Wire {
    version: &'static str,
    runtime: RuntimeBlock,
    storage: StorageBlock,
    quota: QuotaBlock,
    process: ProcessBlock,
    logs: LogsBlock,
    stats: StatsBlock,
    #[serde(rename = "apiKeys")]
    api_keys: ApiKeysCapability,
    probes: ProbesCapability,
}

/// 对外密钥那一格。**与真正强制这几个上限的那段代码同源**，不在这里重拼一份。
#[must_use]
pub fn api_keys_capability(wired: bool, cache_ttl_ms: i64) -> ApiKeysCapability {
    ApiKeysCapability {
        wired,
        max: APIKEY_MAX,
        name_max: API_KEY_NAME_MAX,
        plaintext_retrievable: true,
        cache_ttl_ms,
        default_cache_ttl_ms: APIKEY_CACHE_TTL_MS,
    }
}

/// `GET /admin/api/capabilities`。**零存储读。**
pub async fn capabilities(State(st): State<AppState>) -> Response {
    let env = crate::admin::config::admin_env();
    Json(Wire {
        version: crate::VERSION,
        runtime: RuntimeBlock { name: RUNTIME_NAME },
        storage: StorageBlock {
            backend: STORAGE_BACKEND,
            // 进程内状态，零 IO。
            writable: st.health.status().writable,
        },
        quota: QuotaBlock { model: QUOTA_MODEL },
        process: ProcessBlock {
            metrics: crate::admin::overview::process_metrics().is_some(),
        },
        logs: LogsBlock {
            process_log: PROCESS_LOG,
        },
        stats: StatsBlock {
            // 🔴 **来自「这个 app 到底建没建 sink」，不是现读配置里的 `usageStatsEnabled`**：
            //    那个开关是建 app 时读一次的，有人往存储里改成 true 之后现读会说 true
            //    而这个副本根本没有 sink ⇒ 面板画一张空图表并把它当「这段时间没有流量」。
            tier2_enabled: st.usage_sink.is_some(),
            flush_interval_ms: resolve_usage_flush_interval(env),
            tokens_coverage: tokens_coverage(),
        },
        api_keys: api_keys_capability(
            st.api_keys.is_some(),
            st.api_keys
                .as_ref()
                .map_or(APIKEY_CACHE_TTL_MS, |h| h.ttl_ms()),
        ),
        probes: ProbesCapability {
            // 🔴 **直接引用那道闸自己的常量**，不许在这里抄一个数字字面量。
            min_interval_ms: crate::admin::keys::PROBE_MIN_INTERVAL_MS,
        },
    })
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::storage_test_doubles::{CountingStorage, MemoryStorage};
    use crate::ports::storage::Storage;
    use crate::server::test_support::{TestApp, state_with_storage};
    use axum::body::to_bytes;
    use serde_json::Value;
    use std::sync::Arc;

    async fn raw_of(res: Response) -> String {
        let b = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        String::from_utf8(b.to_vec()).unwrap()
    }

    async fn call(st: AppState) -> Value {
        serde_json::from_str(&raw_of(capabilities(State(st)).await).await).unwrap()
    }

    /// 顶层键**恰好是这些、而且就是这个顺序**。
    ///
    /// ⚠️ 顺序必须比**原始字节**：`serde_json::Value` 是 `BTreeMap`，
    /// 反序列化回来就把顺序信息抹掉了 —— 对着它比是一格永远绿的假判据。
    fn assert_keys_in_order(raw: &str, expected: &[&str]) {
        let v: Value = serde_json::from_str(raw).unwrap();
        let mut got: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        let mut want: Vec<&str> = expected.to_vec();
        got.sort_unstable();
        want.sort_unstable();
        assert_eq!(got, want, "顶层键集不对");

        let mut pos = 0;
        for k in expected {
            let pat = format!("\"{k}\":");
            let at = raw[pos..]
                .find(&pat)
                .unwrap_or_else(|| panic!("{k} 没有出现在它该在的位置之后：{raw}"));
            pos += at + pat.len();
        }
    }

    /// 🔴 **`runtime.colo` 不许回来**，且 **`apiKeys` 那一块必须在**。
    ///
    /// 两条都是「以实现为准、文档样例错了」的那一对：前者是文档里多出来的一格
    /// （恒 null，摘 Worker 形态时删掉的），后者是文档里整块漏掉的一格，
    /// 而**面板靠它显隐整个对外密钥板块**。
    #[tokio::test]
    async fn the_documented_sample_is_wrong_on_exactly_two_blocks() {
        let TestApp { state, .. } = state_with_storage(Arc::new(MemoryStorage::new())).await;
        let v = call(state).await;
        assert!(
            v["runtime"].get("colo").is_none(),
            "colo 在 v0.4.0 删掉了，不许加回来：{v}"
        );
        assert!(
            v["apiKeys"].is_object(),
            "面板靠 apiKeys 显隐整个板块，这一块不能缺：{v}"
        );
        assert_eq!(v["apiKeys"]["max"], 200);
        assert_eq!(v["apiKeys"]["nameMax"], 64);
        assert_eq!(v["apiKeys"]["plaintextRetrievable"], true);
        assert_eq!(v["apiKeys"]["defaultCacheTtlMs"], 300_000);
    }

    /// 顶层九块逐块在，且版本号来自唯一真源。
    ///
    /// ⚠️ **`probes` 是 P2b 加的第九块**（06 §5.3 的 A 档）：面板那个
    /// 「验活最小间隔」以前是前端一个写死的常量，靠一格 parity 判据与后端对表活着。
    #[tokio::test]
    async fn the_top_level_blocks_are_exactly_the_contract() {
        let TestApp { state, .. } = state_with_storage(Arc::new(MemoryStorage::new())).await;
        let raw = raw_of(capabilities(State(state)).await).await;
        assert_keys_in_order(
            &raw,
            &[
                "version", "runtime", "storage", "quota", "process", "logs", "stats", "apiKeys",
                "probes",
            ],
        );
        let v: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["version"], crate::VERSION);
        assert_eq!(v["runtime"]["name"], "rust");
        // 原生构建：后端如实报 `"file"`。Worker 构建里这两个常量是 `"d1"` / `"d1_rows"`
        // （见 `STORAGE_BACKEND` 的注释）；本判据跑在原生侧，所以钉的是原生值。
        assert_eq!(v["storage"]["backend"], "file");
        assert_eq!(v["quota"]["model"], "file");
        assert_eq!(v["logs"]["processLog"], false);
    }

    /// 🔴 **探测最小间隔由后端发，面板不许写死。**
    ///
    /// 期望值**手写字面量**（不是 `PROBE_MIN_INTERVAL_MS` 的同义反复）：
    /// 改那道闸的人必须在这里表态一次 —— 因为改完之后面板上那句
    /// 「隔一会儿再试」的时长就跟着变了，而它是运维唯一能读到的解释。
    #[tokio::test]
    async fn the_probe_cooldown_is_published_so_the_panel_need_not_hardcode_it() {
        let TestApp { state, .. } = state_with_storage(Arc::new(MemoryStorage::new())).await;
        let v = call(state).await;
        assert_eq!(v["probes"]["minIntervalMs"], 3_000);
        assert_eq!(
            v["probes"]["minIntervalMs"],
            crate::admin::keys::PROBE_MIN_INTERVAL_MS,
            "发出去的必须就是那道闸自己用的那个数"
        );
    }

    /// 🔴 **`tokensCoverage` 从协议表算出来，OpenAI 刻意不在列。**
    ///
    /// 期望值**手写字面量**（不是 `tokens_coverage()` 的同义反复）：
    /// 改哪条协议的 `expect_json` 都必须有人在这里表态。
    #[test]
    fn tokens_coverage_is_derived_and_openai_is_deliberately_absent() {
        assert_eq!(tokens_coverage(), vec!["anthropic", "responses", "gemini"]);
        assert!(
            !tokens_coverage().contains(&"openai"),
            "OpenAI 是唯一不解析响应体的一条，它不在列不是漏"
        );
        // 四条协议一条都不许从表里掉出去（掉出去 coverage 会静默少一条）。
        assert_eq!(PROTOCOL_USAGE.len(), 4);
    }

    /// 🔴 **`tier2Enabled` 看的是「建没建 sink」，不是配置里那个开关。**
    ///
    /// 回归对象：有人往存储里把 `usageStatsEnabled` 改成 true 之后，现读配置会说
    /// true 而这个副本根本没有 sink ⇒ 面板画一张空图表并把它当成「这段时间没有流量」。
    #[tokio::test]
    async fn tier2_enabled_tracks_the_sink_not_the_stored_switch() {
        let mem = Arc::new(MemoryStorage::new());
        mem.put(
            crate::core::config_provenance::CONFIG_KEY,
            serde_json::json!({ "usageStatsEnabled": true }),
            None,
        )
        .await
        .unwrap();
        let TestApp { state, .. } = state_with_storage(mem).await;
        state.config.invalidate();
        state.config.ensure_fresh(i64::MAX).await;
        assert!(
            state.config.current().usage_stats_enabled,
            "夹具前提：现读配置确实说 true"
        );
        assert!(state.usage_sink.is_none(), "夹具前提：这个副本没有 sink");

        let v = call(state).await;
        assert_eq!(
            v["stats"]["tier2Enabled"], false,
            "没有 sink 就必须报 false，否则面板会画一张空图表当真"
        );
    }

    /// `flushIntervalMs` 报的是**生效值**：env 调过就报调过的那个。
    #[test]
    fn flush_interval_reports_the_effective_value() {
        let mut env = EnvMap::new();
        assert_eq!(resolve_usage_flush_interval(&env), 7_200_000);

        env.insert(USAGE_FLUSH_INTERVAL_ENV.into(), "60000".into());
        assert_eq!(resolve_usage_flush_interval(&env), 60_000);
    }

    /// ⚠️ 空串与「没设」同等对待；非法值回落默认值而**不是 0**。
    ///
    /// 回归对象：`USAGE_FLUSH_INTERVAL_MS=` 是 `.env.example` 直接 `cp` 出来的常态，
    /// 把它解析成 0 会让面板对「未落盘的尾巴最长多久」报 0 —— 那是一句假话。
    #[test]
    fn an_empty_or_illegal_flush_interval_falls_back_to_the_default() {
        for raw in ["", "   ", "abc", "0", "-1", "1.5"] {
            let mut env = EnvMap::new();
            env.insert(USAGE_FLUSH_INTERVAL_ENV.into(), raw.into());
            assert_eq!(
                resolve_usage_flush_interval(&env),
                7_200_000,
                "USAGE_FLUSH_INTERVAL_MS={raw:?} 必须回落默认值"
            );
        }
    }

    /// 没接对外密钥表时 `wired` 为假，而 `cacheTtlMs` 退到后端默认值。
    #[tokio::test]
    async fn an_unwired_api_key_table_is_reported_honestly() {
        let TestApp { mut state, .. } = state_with_storage(Arc::new(MemoryStorage::new())).await;
        assert_eq!(call(state.clone()).await["apiKeys"]["wired"], true);

        state.api_keys = None;
        let v = call(state).await;
        assert_eq!(v["apiKeys"]["wired"], false);
        assert_eq!(v["apiKeys"]["cacheTtlMs"], 300_000);
    }

    /// 🔴 **零存储读。** 数的是**精确次数**：面板每次刷新都会打这一条。
    #[tokio::test]
    async fn it_never_touches_storage() {
        let counting = Arc::new(CountingStorage::new(Arc::new(MemoryStorage::new())));
        let counts = counting.counts.clone();
        let TestApp { state, .. } = state_with_storage(counting).await;
        let before = counts.snapshot();
        for _ in 0..10 {
            let _ = call(state.clone()).await;
        }
        assert_eq!(counts.snapshot(), before, "capabilities 必须零存储 I/O");
    }
}
