//! 判据用的装配替身。**`#[cfg(test)]`，不进产物。**
//!
//! 它存在的理由与 `ports/` 那一层同源：本仓有一整族判据要观察的是
//! 「这次请求产生了几次存储 `put`」「那条事件到底打出去没有」这类**进程内可观测量**，
//! 打 HTTP 端口根本看不见。
//!
//! ⚠️ **它刻意不复用 `wire::build_app`**：那一条是真实启动路径，
//! 而判据要能逐件替换（存储、时钟、上游）。两者都存在时要提防的是
//! 「测试装配与真实装配漂移」——所以**真实装配自己也有一组判据**（见 `wire.rs`）。

use crate::adapters::logger_multi::MultiLogger;
use crate::adapters::logger_store::StoreLogger;
use crate::adapters::now_ms;
use crate::core::config::resolve_config;
use crate::core::config_provenance::EnvMap;
use crate::core::keypool::LbMode;
use crate::core::storage_health::StorageHealth;
use crate::pool::repo::KeyPoolRepo;
use crate::ports::fetcher::{FetchError, Fetcher, OutboundRequest, OutboundResponse};
use crate::ports::logger::{LogEntry, Logger};
use crate::ports::storage::Storage;
use crate::server::holders::apikey_holder::{APIKEY_CACHE_TTL_MS, ApiKeyHolder};
use crate::server::holders::config_holder::ConfigHolder;
use crate::server::state::AppState;
use bytes::Bytes;
use parking_lot::Mutex;
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};

/// 判据里用的网关口令。
pub const TEST_GATEWAY_TOKEN: &str = "t-abcdefghijklmnop";

/// 把打出去的事件原样记下来。
#[derive(Default)]
pub struct RecordingLogger(Mutex<Vec<LogEntry>>);

impl Logger for RecordingLogger {
    fn log(&self, entry: LogEntry) {
        self.0.lock().push(entry);
    }
}

impl RecordingLogger {
    pub fn entries(&self) -> Vec<LogEntry> {
        self.0.lock().clone()
    }

    pub fn events(&self) -> Vec<&'static str> {
        self.0.lock().iter().map(|e| e.event).collect()
    }
}

/// 永远连不上的上游。
///
/// **默认就该是这一个**：哪条判据不小心真发了一次上游调用，它会以一个
/// 可观察的失败收场，而不是悄悄成功。
pub struct UnreachableFetcher;

#[async_trait::async_trait]
impl Fetcher for UnreachableFetcher {
    async fn send(&self, _req: OutboundRequest) -> Result<OutboundResponse, FetchError> {
        Err(FetchError::Connect("判据里没有上游".into()))
    }
}

/// 脚本里的一条上游响应。
///
/// 🔴 **响应体按块给**：把一整条响应搓成一块的替身会让
/// 「网关不整体缓冲」「多字节字符 / SSE 帧被切在块边界上」这两族性质**整体不可观测**
/// —— 那正是已登记的假阳性形态第 ⑤ 条（测试搭的环境让被测的选择测不出来）。
pub struct UpstreamReply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub chunks: Vec<Bytes>,
}

impl UpstreamReply {
    /// 一条 200，`content-type` 由调用方给，响应体逐块给。
    pub fn raw(content_type: &str, chunks: Vec<Bytes>) -> Self {
        Self {
            status: 200,
            headers: vec![("content-type".into(), content_type.into())],
            chunks,
        }
    }

    /// 一条 `200 application/json`。
    pub fn json(body: serde_json::Value) -> Self {
        Self::raw(
            "application/json",
            vec![Bytes::from(body.to_string().into_bytes())],
        )
    }

    /// 一条 `200 text/event-stream`，**每个元素一块**。
    pub fn sse(chunks: &[&str]) -> Self {
        Self::raw(
            "text/event-stream",
            chunks
                .iter()
                .map(|c| Bytes::from((*c).to_string().into_bytes()))
                .collect(),
        )
    }

    #[must_use]
    pub fn with_header(mut self, k: &str, v: &str) -> Self {
        self.headers.push((k.into(), v.into()));
        self
    }
}

/// 这一次**真的**发给上游的是什么。
#[derive(Clone, Debug)]
pub struct SeenRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Bytes>,
}

impl SeenRequest {
    /// 上游收到的请求体，解析成 JSON。**判据靠它区分「转换过」与「原样透传」。**
    pub fn body_json(&self) -> serde_json::Value {
        self.body
            .as_ref()
            .and_then(|b| serde_json::from_slice(b).ok())
            .unwrap_or(serde_json::Value::Null)
    }
}

/// 脚本化上游：按顺序回放预置的响应。
///
/// ⚠️ **脚本用完 = 判据写错了**（发生了预期之外的上游调用）⇒ 以一个**可观察的失败**
/// 收场，而不是悄悄成功。这条纪律与 `pool::dispatcher` 里那个同名替身逐字一致。
pub struct ScriptedFetcher {
    script: Mutex<VecDeque<UpstreamReply>>,
    seen: Mutex<Vec<SeenRequest>>,
}

impl ScriptedFetcher {
    pub fn new(script: Vec<UpstreamReply>) -> Arc<Self> {
        Arc::new(Self {
            script: Mutex::new(script.into()),
            seen: Mutex::new(Vec::new()),
        })
    }

    /// 这一轮发出去的每一次请求。
    pub fn seen(&self) -> Vec<SeenRequest> {
        self.seen.lock().clone()
    }

    /// 恰好发了一次上游调用，取那一次。**次数本身就是判据的一部分。**
    pub fn only(&self) -> SeenRequest {
        let seen = self.seen();
        assert_eq!(seen.len(), 1, "应当恰好发一次上游调用");
        seen.into_iter().next().expect("刚判过长度")
    }
}

#[async_trait::async_trait]
impl Fetcher for ScriptedFetcher {
    async fn send(&self, req: OutboundRequest) -> Result<OutboundResponse, FetchError> {
        self.seen.lock().push(SeenRequest {
            method: req.method,
            url: req.url,
            headers: req.headers,
            body: req.body,
        });
        let Some(reply) = self.script.lock().pop_front() else {
            return Err(FetchError::Connect(
                "脚本用完了：发生了预期之外的上游调用".into(),
            ));
        };
        Ok(OutboundResponse {
            status: reply.status,
            headers: reply.headers,
            body: Box::pin(futures_util::stream::iter(
                reply.chunks.into_iter().map(Ok).collect::<Vec<_>>(),
            )),
        })
    }
}

pub struct TestApp {
    pub state: AppState,
    /// console 那一路的替身：事件断言看这里。
    pub recorded: Arc<RecordingLogger>,
    /// 事件 sink 用的可注入时钟（epoch 毫秒）。
    ///
    /// **判据要它**：`StoreLogger` 的最小间隔闸把 `last_flush_at` 钉在构造那一刻，
    /// 不推时钟的话任何「刷盘真的发生了」判据都测不出来 ——
    /// 而那正是已登记的假阳性形态第 ④ 条（冻结时钟下的结果是挂起而非失败）。
    pub clock: Arc<AtomicI64>,
}

impl TestApp {
    /// 把事件 sink 的时钟往前推。
    pub fn advance(&self, ms: i64) {
        self.clock.fetch_add(ms, Ordering::Relaxed);
    }
}

/// 用给定存储装一套。
pub async fn state_with_storage(storage: Arc<dyn Storage>) -> TestApp {
    state_with(storage, Arc::new(UnreachableFetcher)).await
}

/// 用**脚本化上游**装一套，并**往池子里放一把 key**。
///
/// 🔴 这两件事必须一起做：池子空着的时候 `dispatch` 在选 key 之前就以 503 收场，
/// 于是任何「成功路径」判据都够不到它要看的那一段 ——
/// 那正是已登记的假阳性形态第 ① 条（判据对着一条没有消费者的代码路径写）。
pub async fn state_with_upstream(script: Vec<UpstreamReply>) -> (TestApp, Arc<ScriptedFetcher>) {
    let fetcher = ScriptedFetcher::new(script);
    let app = state_with(
        Arc::new(crate::adapters::storage_test_doubles::MemoryStorage::new()),
        fetcher.clone(),
    )
    .await;
    app.state
        .repo
        .add("sk-judge-0123456789abcdef", now_ms())
        .await
        .expect("判据用的 key 必须放得进去");
    (app, fetcher)
}

/// 用给定存储 + 给定上游装一套。
pub async fn state_with(storage: Arc<dyn Storage>, fetcher: Arc<dyn Fetcher>) -> TestApp {
    let mut env = EnvMap::new();
    env.insert("GATEWAY_TOKEN".into(), TEST_GATEWAY_TOKEN.into());
    let (config, _) = resolve_config(&env, None, false).expect("判据用的配置必须装得起来");

    let recorded = Arc::new(RecordingLogger::default());
    let clock = Arc::new(AtomicI64::new(now_ms()));
    let sink_clock = clock.clone();
    let event_sink = Arc::new(StoreLogger::with_clock(
        storage.clone(),
        "testshrd",
        // ⚠️ sink 自己坏了的时候**不能**往自己里写：`onError` 走的是裸 sink 之外的那一路。
        recorded.clone(),
        Arc::new(move || sink_clock.load(Ordering::Relaxed)),
    ));
    let logger: Arc<dyn Logger> =
        Arc::new(MultiLogger::new(vec![recorded.clone(), event_sink.clone()]));

    let health = Arc::new(StorageHealth::new());
    let repo = Arc::new(KeyPoolRepo::new(
        storage.clone(),
        logger.clone(),
        config.pool_cache_ttl_ms,
        config.pool_touch_interval_ms,
    ));
    let api_keys = Some(Arc::new(ApiKeyHolder::new(
        storage.clone(),
        logger.clone(),
        APIKEY_CACHE_TTL_MS,
    )));
    let config = Arc::new(ConfigHolder::prime(
        &env,
        storage.clone(),
        logger.clone(),
        config,
        now_ms(),
    ));

    let state = AppState {
        storage,
        health,
        logger,
        event_sink,
        config,
        api_keys,
        repo,
        fetcher,
        key_cursor: Arc::new(AtomicUsize::new(0)),
        sticky_key: Arc::new(Mutex::new(None)),
        lb_mode: LbMode::Priority,
        admin_token: None,
        trust_proxy: false,
        usage_sink: None,
        registrar: None,
        tend_in_flight: Arc::new(std::sync::atomic::AtomicBool::new(false)),
    };
    TestApp {
        state,
        recorded,
        clock,
    }
}
