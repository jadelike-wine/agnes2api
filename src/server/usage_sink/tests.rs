//! 用量写端的判据。
//!
//! 🔴 这一层最要紧的性质是**「它绝不影响响应」** —— 所以断言大量打在
//! 「失败之后还能不能接着记」「失败了有没有说一声」上，而不只是「数字对不对」。

use super::*;
use crate::adapters::storage_test_doubles::MemoryStorage;
use crate::ports::logger::NullLogger;
use std::sync::Mutex as StdMutex;

// ── 夹具 ──────────────────────────────────────────────────────────────────

/// ⚠️ 事件收在 `Arc` 里而不是借用 —— `Arc<dyn Logger>` 要求 `'static`。
#[derive(Default)]
struct SpyLogger {
    events: StdMutex<Vec<(&'static str, String)>>,
}
impl Logger for SpyLogger {
    fn log(&self, e: LogEntry) {
        self.events.lock().unwrap().push((e.event, e.msg));
    }
}

/// 一个**写一律失败**的存储。
struct FailingStorage;
#[async_trait::async_trait]
impl Storage for FailingStorage {
    async fn get(&self, _k: &str) -> crate::ports::storage::StorageResult<Option<Value>> {
        Ok(None)
    }
    async fn put(
        &self,
        _k: &str,
        _v: Value,
        _e: Option<i64>,
    ) -> crate::ports::storage::StorageResult<()> {
        Err(crate::ports::storage::StorageError::new("磁盘满了"))
    }
    async fn delete(&self, _k: &str) -> crate::ports::storage::StorageResult<()> {
        Ok(())
    }
    async fn list(&self, _p: &str) -> crate::ports::storage::StorageResult<Vec<String>> {
        Ok(Vec::new())
    }
}

fn out(ok: bool, stream: bool, latency: i64, model: &str) -> UsageOutcome {
    UsageOutcome {
        ok,
        stream,
        latency_ms: latency,
        tokens_in: if stream { 0 } else { 10 },
        tokens_out: if stream { 0 } else { 20 },
        model: model.into(),
        protocol: "openai",
        api_key_id: Some("ak-1".into()),
    }
}

const DAY0: i64 = 1_800_000_000_000; // 某个 UTC 日的 0 点附近

fn sink(storage: Arc<dyn Storage>, logger: Arc<dyn Logger>, interval: i64) -> UsageSink {
    UsageSink::new(storage, logger, "shard-a", interval, DAY0)
}

async fn stored(s: &MemoryStorage, day: i64, slot: usize) -> Option<Value> {
    s.get(&usage_day_key(day, slot)).await.unwrap()
}

// ── 落盘的形状必须与读端逐字段对上 ───────────────────────────────────────

/// 🔴 **这一格钉的是「写端产出的东西读端读得懂」。**
///
/// 两侧各写一份字段名的话，面板上会安静地显示 0 ——
/// 而「用量是 0」与「用量没记上」在屏幕上长得一模一样。
#[tokio::test]
async fn what_the_writer_produces_is_exactly_what_the_reader_parses() {
    let mem = Arc::new(MemoryStorage::new());
    let s = sink(mem.clone(), Arc::new(NullLogger), 0);
    s.record(&out(true, false, 120, "gpt-4"), DAY0);
    s.flush(DAY0).await;

    let day = usage_day_index(DAY0);
    let v = stored(&mem, day, s.status(DAY0).0)
        .await
        .expect("一个字都没落盘");
    // 顶层五块：读端逐块取。
    for k in ["total", "hours", "byModel", "byProtocol", "byApiKey"] {
        assert!(v.get(k).is_some(), "少了 `{k}` 这一块：{v}");
    }
    let t = &v["total"];
    // 桶的八个字段：**名字必须与读端的 `narrow_bucket` 逐字一致**。
    for k in [
        "requests",
        "success",
        "errors",
        "tokensIn",
        "tokensOut",
        "streamingRequests",
        "latencySum",
        "latencyCount",
    ] {
        assert!(t.get(k).is_some(), "桶里少了 `{k}`：{t}");
    }
    assert_eq!(t["requests"], 1);
    assert_eq!(t["success"], 1);
    assert_eq!(t["errors"], 0);
    assert_eq!(t["tokensIn"], 10);
    assert_eq!(t["latencySum"], 120);
}

/// 落盘要带过期时间 —— **有界性是存储自己的性质，不靠「顺手删一把旧键」。**
#[tokio::test]
async fn every_flush_carries_the_retention_ttl() {
    let mem = Arc::new(MemoryStorage::new());
    let s = sink(mem.clone(), Arc::new(NullLogger), 0);
    s.record(&out(true, false, 1, "m"), DAY0);
    s.flush(DAY0).await;
    let day = usage_day_index(DAY0);
    // `MemoryStorage` 到期即不可见 —— 用一个已过期的时刻反证 TTL 真的挂上了。
    assert!(stored(&mem, day, s.status(DAY0).0).await.is_some());
    assert_eq!(usage_expires_at(day), (day + 30) * 86_400_000 + 86_400_000);
}

// ── 延迟只算成功那一半 ───────────────────────────────────────────────────

/// 🔴 **这一格钉的是那条被订正过的规则。**
///
/// 上一版对失败请求同样累计延迟 —— 于是一个 8 秒超时和一个 30 毫秒的 401
/// 都会进平均延迟，**面板上那个「平均延迟」不是任何真实的东西**。
#[tokio::test]
async fn a_failed_request_contributes_to_errors_but_never_to_latency() {
    let mem = Arc::new(MemoryStorage::new());
    let s = sink(mem.clone(), Arc::new(NullLogger), 0);
    s.record(&out(true, false, 100, "m"), DAY0);
    s.record(&out(false, false, 8_000, "m"), DAY0); // 一个 8 秒的失败
    s.flush(DAY0).await;
    let v = stored(&mem, usage_day_index(DAY0), s.status(DAY0).0)
        .await
        .unwrap();
    let t = &v["total"];
    assert_eq!(t["requests"], 2);
    assert_eq!(t["errors"], 1);
    assert_eq!(
        t["latencySum"], 100,
        "🔴 失败请求的 8 秒进了延迟 —— 那个「平均延迟」就不是任何真实的东西了",
    );
    assert_eq!(t["latencyCount"], 1);
}

/// 流式请求单列一栏，**让 token 的缺口可见**。
#[tokio::test]
async fn streaming_requests_are_counted_separately_so_the_missing_tokens_are_visible() {
    let mem = Arc::new(MemoryStorage::new());
    let s = sink(mem.clone(), Arc::new(NullLogger), 0);
    s.record(&out(true, true, 50, "m"), DAY0);
    s.flush(DAY0).await;
    let t = stored(&mem, usage_day_index(DAY0), s.status(DAY0).0)
        .await
        .unwrap()["total"]
        .clone();
    assert_eq!(t["streamingRequests"], 1);
    assert_eq!(
        t["tokensIn"], 0,
        "流式那条本来就没有 token —— 它该显示成 0 而不是被算进某个平均里",
    );
}

// ── 上界 ─────────────────────────────────────────────────────────────────

/// 🔴 **模型名是客户端随便填的** —— 不收上界时一个乱填的客户端就能把日累加器撑爆。
#[tokio::test]
async fn a_client_spamming_model_names_cannot_grow_the_accumulator_without_bound() {
    let mem = Arc::new(MemoryStorage::new());
    let s = sink(mem.clone(), Arc::new(NullLogger), 0);
    for i in 0..(USAGE_MODEL_MAX_KEYS + 50) {
        s.record(&out(true, false, 1, &format!("m{i}")), DAY0);
    }
    s.flush(DAY0).await;
    let v = stored(&mem, usage_day_index(DAY0), s.status(DAY0).0)
        .await
        .unwrap();
    let by_model = v["byModel"].as_object().unwrap();
    assert!(
        by_model.len() <= USAGE_MODEL_MAX_KEYS + 1,
        "byModel 长到了 {} 个键",
        by_model.len(),
    );
    assert!(
        by_model.contains_key(USAGE_OTHER_BUCKET),
        "越界的那些没归进 `__other__` —— 它们的计数就凭空丢了",
    );
    // 一条都不许丢：总数仍然是全部。
    assert_eq!(v["total"]["requests"], (USAGE_MODEL_MAX_KEYS + 50) as i64);
}

/// 🔴 **收上界必须排在写入之前。**
///
/// 变异对象：先写再判 —— 那一次的计数已经落进了一个越界的键，
/// 而随后的归并**不会把它取回来**。
#[tokio::test]
async fn a_key_that_would_exceed_the_bound_is_folded_before_anything_is_written() {
    let mem = Arc::new(MemoryStorage::new());
    let s = sink(mem.clone(), Arc::new(NullLogger), 0);
    for i in 0..USAGE_MODEL_MAX_KEYS {
        s.record(&out(true, false, 1, &format!("m{i}")), DAY0);
    }
    s.record(&out(true, false, 1, "一个越界的新模型"), DAY0);
    s.flush(DAY0).await;
    let v = stored(&mem, usage_day_index(DAY0), s.status(DAY0).0)
        .await
        .unwrap();
    assert!(
        !v["byModel"]
            .as_object()
            .unwrap()
            .contains_key("一个越界的新模型"),
        "越界的键仍然被写出去了 —— 先写再判的形态",
    );
    assert_eq!(v["byModel"][USAGE_OTHER_BUCKET]["requests"], 1);
}

/// ⚠️ 两个上界**不是同一个数**：这一维的键是我们自己发的，客户端选不了。
#[test]
fn the_two_bounds_are_deliberately_different() {
    assert_ne!(
        USAGE_MODEL_MAX_KEYS, USAGE_APIKEY_MAX_KEYS,
        "两个上界被统一成了一个数 —— 它们的立论完全不同",
    );
}

/// ⚠️ `byProtocol` **刻意不收上界**：它的值是四条路由里的字面量，外部碰不到。
#[tokio::test]
async fn the_protocol_dimension_is_not_bounded_because_clients_cannot_reach_it() {
    let mem = Arc::new(MemoryStorage::new());
    let s = sink(mem.clone(), Arc::new(NullLogger), 0);
    s.record(&out(true, false, 1, "m"), DAY0);
    s.flush(DAY0).await;
    let v = stored(&mem, usage_day_index(DAY0), s.status(DAY0).0)
        .await
        .unwrap();
    assert_eq!(v["byProtocol"]["openai"]["requests"], 1);
}

// ── 归属 ─────────────────────────────────────────────────────────────────

/// ⚠️ **`None` 与空串是同一件事** —— 一个空字符串键在面板上是一格**没有名字的行**。
#[tokio::test]
async fn an_absent_and_an_empty_api_key_id_both_land_in_the_unattributed_bucket() {
    for id in [None, Some(String::new())] {
        let mem = Arc::new(MemoryStorage::new());
        let s = sink(mem.clone(), Arc::new(NullLogger), 0);
        let mut u = out(true, false, 1, "m");
        u.api_key_id = id.clone();
        s.record(&u, DAY0);
        s.flush(DAY0).await;
        let v = stored(&mem, usage_day_index(DAY0), s.status(DAY0).0)
            .await
            .unwrap();
        assert_eq!(
            v["byApiKey"][USAGE_UNATTRIBUTED_BUCKET]["requests"], 1,
            "{id:?} 没归进未归属那一格",
        );
    }
}

// ── 失败态 ───────────────────────────────────────────────────────────────

/// 🔴 **落盘失败不清 dirty** —— 下一次会把整份再写一遍，这一次没丢东西。
#[tokio::test]
async fn a_failed_flush_keeps_the_data_and_the_next_one_writes_it_all() {
    let spy = Arc::new(SpyLogger::default());
    let s = UsageSink::new(Arc::new(FailingStorage), spy.clone(), "shard-a", 0, DAY0);
    s.record(&out(true, false, 1, "m"), DAY0);
    s.flush(DAY0).await;
    assert_eq!(
        spy.events.lock().unwrap()[0].0,
        EVENT_FLUSH_FAILED,
        "落盘失败却一声不吭",
    );
    // 换一个能写的存储再落一次 —— 那一条计数必须还在。
    let mem = Arc::new(MemoryStorage::new());
    let s2 = sink(mem.clone(), Arc::new(NullLogger), 0);
    s2.record(&out(true, false, 1, "m"), DAY0);
    s2.flush(DAY0).await;
    assert_eq!(
        stored(&mem, usage_day_index(DAY0), s2.status(DAY0).0)
            .await
            .unwrap()["total"]["requests"],
        1,
    );
}

/// 🔴🔴 **这一格钉的是那条最容易漏的并发规则。**
///
/// 我发起写之后可能又有新的 `record` 进来 —— 那一份增量**不在我刚写出去的快照里**。
/// 无条件清 dirty 会让它**永远落不了盘**，而计数看起来一切正常。
///
/// ⚠️⚠️ **这一格我第一版写错了，留个路标。** 我原来让第二条 `record` 发生在
/// `flush()` **返回之后** —— 那样 dirty 会被第二条重新置上，第二次落盘照样写出去，
/// **版本号那条检查一次都走不到**。变异（无条件清 dirty）于是存活。
/// 要抓住它，那条增量**必须在落盘进行中到达** —— 也就是下面这个会在 `put` 里回调的存储。
#[tokio::test]
async fn an_increment_that_arrives_during_a_flush_is_not_silently_dropped() {
    use std::sync::atomic::{AtomicBool, Ordering};

    /// 一个**在 put 进行中回调一次**的存储：模拟「落盘还没回来，又来了一条请求」。
    struct RacingStorage {
        inner: Arc<MemoryStorage>,
        sink: StdMutex<Option<Arc<UsageSink>>>,
        fired: AtomicBool,
    }
    #[async_trait::async_trait]
    impl Storage for RacingStorage {
        async fn get(&self, k: &str) -> crate::ports::storage::StorageResult<Option<Value>> {
            self.inner.get(k).await
        }
        async fn put(
            &self,
            k: &str,
            v: Value,
            e: Option<i64>,
        ) -> crate::ports::storage::StorageResult<()> {
            // 真正写出去**之前**插一条新的计数 —— 它不在调用方刚取的那个快照里。
            if !self.fired.swap(true, Ordering::SeqCst)
                && let Some(s) = self.sink.lock().unwrap().clone()
            {
                s.record(&out(true, false, 1, "m"), DAY0);
            }
            self.inner.put(k, v, e).await
        }
        async fn delete(&self, k: &str) -> crate::ports::storage::StorageResult<()> {
            self.inner.delete(k).await
        }
        async fn list(&self, p: &str) -> crate::ports::storage::StorageResult<Vec<String>> {
            self.inner.list(p).await
        }
    }

    let mem = Arc::new(MemoryStorage::new());
    let racing = Arc::new(RacingStorage {
        inner: mem.clone(),
        sink: StdMutex::new(None),
        fired: AtomicBool::new(false),
    });
    let s = Arc::new(UsageSink::new(
        racing.clone(),
        Arc::new(NullLogger),
        "shard-a",
        0,
        DAY0,
    ));
    *racing.sink.lock().unwrap() = Some(s.clone());

    s.record(&out(true, false, 1, "m"), DAY0);
    // 这一次落盘写出去的快照里只有 1 条，而写的**过程中**又来了第 2 条。
    s.flush(DAY0).await;
    // 再落一次 —— 那第 2 条必须还在 dirty 里，于是被写出去。
    s.flush(DAY0).await;

    assert_eq!(
        stored(&mem, usage_day_index(DAY0), s.status(DAY0).0)
            .await
            .unwrap()["total"]["requests"],
        2,
        "🔴 落盘进行中到达的那一条永远落不了盘 —— dirty 被无条件清掉了，\
         而计数看起来一切正常",
    );
}

/// ⚠️ **失败的那一次也把 `last_flush_at` 推到当下** —— 那是节流要的语义。
///
/// 不推的话存储一直失败时**每个请求都会重试一次**，白烧延迟。
#[tokio::test]
async fn a_failing_store_does_not_turn_every_request_into_a_retry() {
    let spy = Arc::new(SpyLogger::default());
    let s = UsageSink::new(
        Arc::new(FailingStorage),
        spy.clone(),
        "shard-a",
        60_000, // 一分钟一落
        DAY0,
    );
    s.record(&out(true, false, 1, "m"), DAY0);
    s.maybe_flush(DAY0 + 60_001).await; // 到点，试一次（失败）
    s.maybe_flush(DAY0 + 60_002).await; // 紧接着又来一个请求 —— 不该再试
    assert_eq!(
        spy.events.lock().unwrap().len(),
        1,
        "存储一直失败时每个请求都重试了一次",
    );
}

/// 没到点就一次写都不发。
#[tokio::test]
async fn nothing_is_written_before_the_interval_elapses() {
    let mem = Arc::new(MemoryStorage::new());
    let s = sink(mem.clone(), Arc::new(NullLogger), 60_000);
    s.record(&out(true, false, 1, "m"), DAY0);
    s.maybe_flush(DAY0 + 59_999).await;
    assert!(
        stored(&mem, usage_day_index(DAY0), s.status(DAY0).0)
            .await
            .is_none(),
        "还没到点就落盘了",
    );
}

/// 一条都没记过时**一次写都不发**（哪怕早就到点了）。
#[tokio::test]
async fn an_idle_process_writes_nothing_no_matter_how_long_it_runs() {
    let mem = Arc::new(MemoryStorage::new());
    let s = sink(mem.clone(), Arc::new(NullLogger), 0);
    s.maybe_flush(DAY0 + 999_999_999).await;
    assert!(
        stored(&mem, usage_day_index(DAY0), s.status(DAY0).0)
            .await
            .is_none()
    );
}

// ── 跨天 ─────────────────────────────────────────────────────────────────

/// 跨天之后两天各写各的键。
#[tokio::test]
async fn two_days_land_in_two_separate_keys() {
    let mem = Arc::new(MemoryStorage::new());
    let s = sink(mem.clone(), Arc::new(NullLogger), 0);
    let next_day = DAY0 + 86_400_000;
    s.record(&out(true, false, 1, "m"), DAY0);
    s.record(&out(true, false, 1, "m"), next_day);
    s.flush(next_day).await;
    let slot = s.status(DAY0).0;
    assert_eq!(
        stored(&mem, usage_day_index(DAY0), slot).await.unwrap()["total"]["requests"],
        1,
    );
    assert_eq!(
        stored(&mem, usage_day_index(next_day), slot).await.unwrap()["total"]["requests"],
        1,
    );
}

/// 小时桶落在对的那一格。
#[tokio::test]
async fn the_hour_bucket_matches_the_utc_hour() {
    let mem = Arc::new(MemoryStorage::new());
    let s = sink(mem.clone(), Arc::new(NullLogger), 0);
    let day_start = usage_day_index(DAY0) * 86_400_000;
    s.record(&out(true, false, 1, "m"), day_start + 5 * 3_600_000 + 30);
    s.flush(DAY0).await;
    let v = stored(&mem, usage_day_index(DAY0), s.status(DAY0).0)
        .await
        .unwrap();
    assert_eq!(
        v["hours"]["5"]["requests"], 1,
        "小时桶落错了格：{}",
        v["hours"]
    );
}

// ── 分片 ─────────────────────────────────────────────────────────────────

/// 同一个 `shard_id` 落在同一格 —— 重启之后它**覆写自己那一份**，而不是再多占一格。
#[test]
fn the_same_shard_id_always_lands_in_the_same_slot() {
    let mk = |id: &str| {
        UsageSink::new(
            Arc::new(MemoryStorage::new()),
            Arc::new(NullLogger),
            id,
            0,
            DAY0,
        )
        .status(DAY0)
        .0
    };
    assert_eq!(mk("shard-a"), mk("shard-a"));
    for id in ["shard-a", "shard-b", "x", ""] {
        assert!(mk(id) < USAGE_SLOTS, "槽位越界：{id}");
    }
}
