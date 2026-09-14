//! HTTP 服务：监听、优雅停机、有界排空、停机显式刷盘。
//!
//! 边界：**这一层只做装配与闸，业务一律在各子模块。**

pub mod app;
pub mod client_ip;
pub mod errors;
pub mod state;
pub mod wire;

pub mod routes;

/// 中间件。**挂载顺序本身是规格**，见 [`app::build_router`]。
pub mod middleware;

/// 「持有者」：带 TTL 的配置 / 对外密钥快照。
pub mod holders;

/// 判据用的装配替身。**不进产物。**
#[cfg(test)]
pub mod test_support;
pub mod usage_sink;

use crate::core::config_provenance::EnvMap;
use crate::ports::logger::{LogEntry, LogLevel, Logger};
use anyhow::Context;
use std::future::{Future, IntoFuture};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

/// 停机时给在途请求的**排空窗口上界**。
///
/// 🔴 **`with_graceful_shutdown` 必须封顶。** kiro 那边的事故形态在这里同构：
/// 管理面的实时日志 SSE 是**无限流**，只要有一个管理页开着，停机就会永远卡住，
/// 最后被 SIGKILL 硬掐 —— 在途转发流照样被切断，而**最终刷盘反而彻底跳过**，
/// 和不做优雅停机相比只坏不好。
///
/// **8 秒的取值理由**：必须短于容器宽限期（`docker stop` 默认 10 秒），
/// 给窗口到期后的刷盘留余量；同时长于绝大多数在途一问一答请求。
pub const SHUTDOWN_DRAIN_WINDOW: Duration = Duration::from_secs(8);

/// 起服务。
///
/// 顺序本身是约束：**服务器自身的错误放到刷盘之后再上抛** ——
/// 用 `?` 提前返回会把最终刷盘整个吞掉。
pub async fn serve(
    env: &EnvMap,
    addr: SocketAddr,
    router: axum::Router,
    logger: Arc<dyn Logger>,
) -> anyhow::Result<()> {
    serve_with_signal(env, addr, router, logger, shutdown_signal()).await
}

/// [`serve`] 的本体，**停机信号可注入**。
///
/// 🔴 **它不是「为测试而拆」的样板**：真实的停机信号是**进程级**的
/// （SIGTERM / Ctrl-C），而本模块要钉的性质是「排空窗口从信号那一刻起算」——
/// 那条性质在真信号下唯一的观测方式是给自己的测试进程发一个 SIGTERM，
/// 而那会把同一进程里跑着的其余几百格判据一起带走。
/// ⇒ 可注入的那一格是那条判据**唯一**的前提，体例与
/// [`crate::admin::models::test_with`]（护栏与时钟可注入）逐字相同。
pub async fn serve_with_signal<S>(
    env: &EnvMap,
    addr: SocketAddr,
    router: axum::Router,
    logger: Arc<dyn Logger>,
    signal: S,
) -> anyhow::Result<()>
where
    S: Future<Output = ()> + Send + 'static,
{
    let _ = env; // 端口与数据目录已由调用方解析；这里保留形参是为了将来读运行期开关。

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("监听 {addr} 失败"))?;
    let local = listener.local_addr().unwrap_or(addr);
    logger.log(
        LogEntry::new(LogLevel::Info, "server.listening", "网关已开始监听")
            .with("addr", local.to_string())
            .with("version", crate::VERSION),
    );

    // 🔴🔴 **排空窗口从「信号到达」那一刻起算，不是从 bind 成功那一刻。**
    //
    // 信号那个 future 归 axum 所有（它要靠它开始优雅停机），而**本层也要知道它到了没有**
    // —— 窗口的起点就是它。⇒ 在它后面串一个 oneshot 当「抄送」，
    // 一个 future 两个观察者，结构上不可能分叉成两个时机。
    //
    // ⚠️ **不许在这里 clone 一份信号**（各等各的 SIGTERM）：两份各自装信号处理器，
    //    而进程级信号的投递是**一次性**的，两边谁收到谁收不到不受本层控制。
    let (signal_seen_tx, signal_seen_rx) = tokio::sync::oneshot::channel::<()>();
    let armed = async move {
        signal.await;
        // 收不到是「本层已经不在等了」，不是错误。
        let _ = signal_seen_tx.send(());
    };
    let server = axum::serve(listener, router).with_graceful_shutdown(armed);
    let outcome = drain_after_signal(server, signal_seen_rx, SHUTDOWN_DRAIN_WINDOW).await;

    // 停机显式刷盘。
    //
    // **本轮没有任何东西需要刷**（`Storage::put` 的语义是「返回时已落盘」，
    // 没有去抖批量写）。这不是遗漏：TS 的 `still_exists()`（删除不许被陈旧写回撤销）
    // 与跨副本删除持久性判据**都建立在这条语义上**，改成去抖要先改判据。
    // ⇒ 这里留一句日志把「刷盘这件事发生过」钉出来，
    //    接上池 / 统计 / 对外密钥之后在这里逐项刷，每一项失败**只记 error、
    //    绝不阻断停机**（各项之间无依赖）。
    logger.log(LogEntry::new(
        LogLevel::Info,
        "server.shutdown",
        "已停止接受新请求；本轮没有需要额外刷盘的组件（写都是同步落盘的）",
    ));

    match outcome {
        Some(r) => r.context("HTTP 服务异常退出"),
        None => {
            logger.log(LogEntry::new(
                LogLevel::Warn,
                "server.drain_timeout",
                "排空窗口到期仍有在途请求，按上界强制收尾（无限流的管理端 SSE 会走到这里）",
            ));
            Ok(())
        }
    }
}

/// 等一个停机信号。
///
/// 🔴 **必须处理 SIGTERM**：容器里本进程常是 PID 1，不处理就会一直等到宽限期结束
/// 被 SIGKILL —— 那等于完全没有优雅停机。
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let term = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            // 装不上信号处理器时**不许 panic**：退化成「只认 Ctrl-C」，
            // 比进程起不来好。
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {}
        () = term => {}
    }
}

/// 先等信号，**信号到了才开始计排空窗口**。
///
/// 返回值与 [`drain_with_deadline`] 同义：`Some(结果)` = 服务器自己结束了
/// （无论是在信号之前还是排空窗口之内）；`None` = 窗口到期，调用方按上界收尾。
///
/// # 🔴 为什么不能把服务器与 `sleep(window)` 直接放进一个 `select!`
///
/// 那正是被修掉的那个缺陷：server future **只有在收到信号并排空之后才完成**，
/// 所以正常运行时 `sleep(window)` 必然先赢 ⇒ 网关起来 `window` 秒之后无条件自杀。
/// 由 [`tests::the_gateway_keeps_serving_long_after_the_drain_window_would_have_elapsed`]
/// 与 [`tests::the_drain_window_is_measured_from_the_signal_not_from_bind`] 两格钉住。
pub async fn drain_after_signal<F, S>(server: F, signal: S, window: Duration) -> Option<F::Output>
where
    F: IntoFuture,
    S: IntoFuture,
{
    let fut = server.into_future();
    tokio::pin!(fut);
    let sig = signal.into_future();
    tokio::pin!(sig);

    // 第一段：**不设上界**。这里只有两种结束方式 —— 服务器自己出错退出，
    // 或者信号到达。给这一段设上界就是给「网关能活多久」设上界。
    tokio::select! {
        biased;
        out = &mut fut => return Some(out),
        _ = &mut sig => {}
    }

    // 第二段：信号到手，**从此刻起**给在途请求一个有界的排空窗口。
    drain_with_deadline(&mut fut, window).await
}

/// 带上界地等一个 future 结束。
///
/// 返回 `Some(结果)` = 在窗口内正常结束；`None` = 窗口到期，调用方按上界收尾。
///
/// 抽成泛型函数（取 `IntoFuture` 而不是 `Future`，因为 axum 的
/// `WithGracefulShutdown` 只实现前者）**是为了能用假 future 单测边界行为，
/// 不必真开监听 socket**。
pub async fn drain_with_deadline<F>(server: F, window: Duration) -> Option<F::Output>
where
    F: IntoFuture,
{
    // `biased;` 优先轮询服务器，使「信号与结束同时就绪」时确定性地报干净结束 ——
    // 不加的话这条边界在两次运行之间会摇摆，而摇摆的判据等于没有判据。
    let fut = server.into_future();
    tokio::pin!(fut);
    tokio::select! {
        biased;
        out = &mut fut => Some(out),
        () = tokio::time::sleep(window) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::test_support::RecordingLogger;
    use axum::routing::get;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;
    use tokio::sync::oneshot;
    use tokio::task::JoinHandle;

    /// 一套**真的起在端口上**的服务。
    struct Running {
        addr: SocketAddr,
        handle: JoinHandle<anyhow::Result<()>>,
        recorded: Arc<RecordingLogger>,
        /// 发一下 = 停机信号到达。
        signal: oneshot::Sender<()>,
        /// `/hang` 那条 handler 是否**真的进来过**。
        hit_hang: Arc<AtomicBool>,
    }

    impl Running {
        fn events(&self) -> Vec<&'static str> {
            self.recorded.events()
        }
    }

    /// 让出执行权若干次，让 accept 循环 / 连接任务推进。
    ///
    /// ⚠️ **不许用 `tokio::time::timeout` 当护栏**：`start_paused` 下运行时一空闲
    /// 就会把假时钟推到下一个定时器上，而「等 loopback 上的字节到达」恰恰是一段空闲 ⇒
    /// 那种护栏会随机地提前触发。这里用**有界的 yield 次数** + 明确的 panic 文案代替。
    async fn spin_until(mut done: impl FnMut() -> bool, what: &str) {
        for _ in 0..20_000 {
            if done() {
                return;
            }
            tokio::task::yield_now().await;
        }
        panic!("等不到：{what}");
    }

    /// 起一套真服务，返回它的地址与把手。
    async fn spawn_server() -> Running {
        let recorded = Arc::new(RecordingLogger::default());
        let logger: Arc<dyn Logger> = recorded.clone();
        let (signal, rx) = oneshot::channel::<()>();
        let hit_hang = Arc::new(AtomicBool::new(false));

        let marker = hit_hang.clone();
        let router = axum::Router::new()
            .route("/ping", get(|| async { "pong" }))
            // 一条**永不返回**的 handler：它是「在途请求」唯一可控的形态，
            // 而「窗口到期强制收尾」这条性质没有在途请求就不可观测。
            .route(
                "/hang",
                get(move || {
                    let marker = marker.clone();
                    async move {
                        marker.store(true, Ordering::SeqCst);
                        std::future::pending::<String>().await
                    }
                }),
            );

        let handle = tokio::spawn(async move {
            let env = EnvMap::new();
            serve_with_signal(
                &env,
                "127.0.0.1:0".parse().expect("回环地址"),
                router,
                logger,
                async move {
                    let _ = rx.await;
                },
            )
            .await
        });

        // 真实端口从 `server.listening` 那条事件里取 —— 顺带钉住「那条事件带的是
        // 真正绑上的地址」（传 :0 时它必须是解析后的端口，不是字面量 0）。
        let mut addr = None;
        let peek = recorded.clone();
        spin_until(
            || {
                addr = peek
                    .entries()
                    .iter()
                    .find(|e| e.event == "server.listening")
                    .and_then(|e| e.fields["addr"].as_str().map(str::to_string));
                addr.is_some()
            },
            "server.listening 事件",
        )
        .await;
        let addr: SocketAddr = addr.expect("刚判过").parse().expect("事件里的地址要能解析");
        assert_ne!(addr.port(), 0, "事件里必须是真正绑上的端口");

        Running {
            addr,
            handle,
            recorded,
            signal,
            hit_hang,
        }
    }

    /// 打一条最小的 HTTP/1.1 请求并读到 EOF。
    ///
    /// ⚠️ **刻意不用 reqwest**：那一个自带连接池与空闲定时器，而假时钟下任何
    /// 内部定时器都会与自动推进相互作用 —— 判据会变成随机的。
    async fn http_get(addr: SocketAddr, path: &str) -> std::io::Result<String> {
        let mut s = TcpStream::connect(addr).await?;
        s.write_all(
            format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .await?;
        let mut buf = Vec::new();
        s.read_to_end(&mut buf).await?;
        Ok(String::from_utf8_lossy(&buf).into_owned())
    }

    /// 🔴🔴 **没有停机信号时，网关不许自己停** —— 哪怕过去了几十倍的排空窗口。
    ///
    /// # 回归对象（实测，v0.1.0）
    ///
    /// ⚠️ 下面这段是**照抄的真实日志，只把监听地址换成了占位**：
    /// 凭据扫描那道门禁对 `IP:端口` 这个形态是**零白名单**的（它分不出
    /// 「示例」和「某个部署的真地址」，而那正是它该有的态度）。
    /// 时间戳与事件名照旧原样 —— 它们才是这段话要说明的东西。
    ///
    /// ```text
    /// 11:48:51.58  event=server.listening     msg=网关已开始监听 addr=<监听地址>
    /// 11:48:59.58  event=server.shutdown      msg=已停止接受新请求…
    /// 11:48:59.58  event=server.drain_timeout WARN msg=排空窗口到期仍有在途请求…
    /// ```
    /// **起来整整 8.004 秒后进程退出。** 成因：`drain_with_deadline` 把
    /// **整个 server future** 与 `sleep(8s)` 放进同一个 `select!` ——
    /// server future 只有在收到信号并排空之后才完成 ⇒ 正常运行时那个 sleep
    /// **必然先赢**，于是每一次启动都在 8 秒后打完停机日志并 `Ok(())` 返回。
    ///
    /// ⇒ **排空窗口必须从「信号到达」那一刻起算，不是从 bind 成功那一刻。**
    ///
    /// # 为什么现有那三格拦不住它
    ///
    /// 它们测的是 [`drain_with_deadline`] **这个函数自己**（喂假 future），
    /// 而这个缺陷在于 [`serve`] 喂给它的是什么 —— 函数本身的行为完全正确。
    /// ⇒ 观测量只能是 `serve()` 的**可观测行为**：服务还在不在。
    ///
    /// ⚠️ `/health` 一直 200 正是这个缺陷最毒的地方：任何「起得来吗」的冒烟测试
    /// 都在那 8 秒窗口内跑完，**一次都抓不到它**。
    #[tokio::test(start_paused = true)]
    async fn the_gateway_keeps_serving_long_after_the_drain_window_would_have_elapsed() {
        let run = spawn_server().await;
        // ⚠️ 断言「状态行 + 正文」而不是整串字节：响应里有一格 `date`，
        //    对着整串比是一格会随时间自己变红的假判据。
        let first = http_get(run.addr, "/ping").await.expect("起来之后要能打通");
        assert!(
            first.starts_with("HTTP/1.1 200 OK\r\n") && first.ends_with("\r\n\r\npong"),
            "夹具前提：服务真的在服务，实际：{first:?}"
        );

        // 推进**几十倍**的排空窗口。没有信号 ⇒ 这段时间里什么都不该发生。
        tokio::time::advance(SHUTDOWN_DRAIN_WINDOW * 50).await;
        tokio::task::yield_now().await;

        assert!(
            !run.handle.is_finished(),
            "没收到停机信号，`serve()` 不许返回"
        );
        assert_eq!(
            run.events(),
            vec!["server.listening"],
            "没收到停机信号，停机那几条事件一条都不许打"
        );
        assert!(
            http_get(run.addr, "/ping").await.is_ok(),
            "排空窗口过去几十倍之后，网关必须**照样在服务**"
        );
    }

    /// 🔴🔴 **排空窗口从「信号到达」那一刻起算。**
    ///
    /// 观测量是**假时钟上的两个时刻之差**：信号发出到 `serve()` 返回之间
    /// 必须整整走过一个排空窗口。窗口若是从 bind 起算的，它在第 ① 步那几十倍的
    /// 推进里早就到期了 ⇒ 信号一到就会**立刻**收尾，这里的差值会是 0。
    ///
    /// ⚠️ 必须有一个**真的在途**的请求（`/hang` 那条永不返回的 handler），
    /// 否则优雅停机会当场完成，整条窗口逻辑压根不被执行。
    #[tokio::test(start_paused = true)]
    async fn the_drain_window_is_measured_from_the_signal_not_from_bind() {
        let run = spawn_server().await;

        // ① 挂一条**在途**请求：连上、把请求发出去、**不读响应**。
        let mut hung = TcpStream::connect(run.addr).await.expect("连得上");
        hung.write_all(b"GET /hang HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .expect("发得出去");
        let hit = run.hit_hang.clone();
        spin_until(|| hit.load(Ordering::SeqCst), "/hang 那条 handler 真的进来").await;

        // ② 先空转几十倍的窗口：若窗口从 bind 起算，它此刻已经到期了。
        tokio::time::advance(SHUTDOWN_DRAIN_WINDOW * 50).await;
        tokio::task::yield_now().await;
        assert!(!run.handle.is_finished(), "没信号就不许收尾");

        // ③ 信号到达 —— **计时从这里开始**。
        let fired_at = tokio::time::Instant::now();
        run.signal.send(()).expect("服务还在，信号发得出去");
        let outcome = run.handle.await.expect("serve 这条任务不许 panic");
        let waited = fired_at.elapsed();

        assert!(
            waited >= SHUTDOWN_DRAIN_WINDOW,
            "信号到达之后必须整整给够一个排空窗口，实际只等了 {waited:?}"
        );
        assert!(outcome.is_ok(), "窗口到期是**按上界收尾**，不是一次失败");
        assert_eq!(
            run.recorded.events(),
            vec![
                "server.listening",
                "server.shutdown",
                "server.drain_timeout"
            ],
            "在途请求卡到窗口到期 ⇒ 这三条事件缺一不可，且顺序固定"
        );
        drop(hung);
    }

    /// 在窗口内结束 ⇒ 拿到结果。
    #[tokio::test]
    async fn a_prompt_shutdown_returns_the_servers_outcome() {
        let out = drain_with_deadline(async { 7 }, Duration::from_secs(8)).await;
        assert_eq!(out, Some(7));
    }

    /// 🔴 **卡死的服务器不许把停机卡住。**
    ///
    /// 回归对象：管理端实时日志 SSE 是无限流，只要有一个页面开着，
    /// 不封顶的停机就会一直等到被 SIGKILL 硬掐 —— 在途请求照样断，
    /// 而最终刷盘**反而彻底跳过**。
    #[tokio::test(start_paused = true)]
    async fn a_hung_server_hits_the_deadline_instead_of_blocking_forever() {
        let out = drain_with_deadline(std::future::pending::<()>(), Duration::from_secs(8)).await;
        assert!(out.is_none(), "到点必须放手");
    }

    /// 窗口取值：**必须短于 `docker stop` 的默认宽限期（10 秒）**，
    /// 才有余量做窗口到期后的收尾。
    #[test]
    fn the_drain_window_leaves_room_before_sigkill() {
        assert_eq!(SHUTDOWN_DRAIN_WINDOW, Duration::from_secs(8));
        assert!(
            SHUTDOWN_DRAIN_WINDOW < Duration::from_secs(10),
            "排空窗口不短于容器宽限期的话，收尾那一段永远跑不到"
        );
    }
}
