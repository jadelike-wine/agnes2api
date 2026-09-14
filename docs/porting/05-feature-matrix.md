# agnes2api × kiro2api 功能对比与去留裁定

> 用途：把两个仓的**功能面**列全，按用户定的那条规则逐条裁定，供 P1/P2/P3 排期取用。
>
> 裁定规则（用户原话，不重新解释）：
> > 如果功能是 kiro2api 没有的，或者是比 kiro2api 更强的，就保留；
> > 如果是 kiro2api 有的但是 agnes2api 缺少的或者 agnes2api 不如 kiro2api 的，就对标 kiro2api。
> > **但是面板的样式和 UI 一定要完全对标 kiro2api。**
>
> 证据口径：**逐条来自读源码**（路由注册处、结构体/接口定义、调度内核、i18n 词典键），
> 不采信任何一边 README 的自述。每条都给文件路径；行号对应 2026-09-11（北京时间）的工作副本。
>
> 两边规模基线（`find src -type f | wc -l` + `wc -l`）：
> - agnes2api：TypeScript，96 源文件 / 23,480 行；面板 `admin-ui/` 40 个 html+css+js+mjs / **17,776 行**（原生 ESM，无框架；其中 `js/pure/*.mjs` 15 个纯函数模块是可单测的那一半）
> - kiro2api：Rust，71 源文件 / 53,852 行（含大量 inline `#[test]`）；面板 `admin-ui-v2/` 20 个文件 / **12,501 行**（原生 ESM，无框架，不含其 `tests/`）+ 另有 `user-ui/` React+Vite 用户面门户
>
> ⚠️ **别把「agnes 面板行数更多」读成「agnes 面板更强」。** 行数差主要来自 agnes 的注释密度与
> `js/pure/` 的纯函数拆分；**真正决定观感的 CSS 是 1,077 : 3,465，agnes 只有 kiro 的三分之一**（见 §7 末尾）。

---

## 0. 裁定标记的含义

| 标记 | 含义 | 归属分期 |
|---|---|---|
| **保留** | agnes 独有或明确更强，Rust 版照搬语义 | 随对应模块走 |
| **对标** | kiro 有 / 更强，Rust 版照 kiro 的形状做 | P1（转发面）/ P2（面板）|
| **新增** | 两边都没有，但对标 kiro 的 UI 之后不做会露馅 | P2 |
| **合并** | 两边各强一半，取并集 | 按轴分 |
| **不做** | 判断不必做，**必须写明理由** | — |

### 0.1 一个量化锚点：管理 API 面的规模差

| | agnes2api | kiro2api |
|---|---|---|
| 管理 API 处理器数 | **33**（`src/http/admin/router.ts` 的 `admin.<verb>()` 注册处，不含 `use()` 与静态兜底） | **51** 个方法级路由（`src/admin/mod.rs`，含 4 条向后兼容旧路径 `/admin/api/*`） |
| 用户面 API | **0** | **3**（`src/user/mod.rs`） |
| 路径前缀 | `/admin/api/*` | `/api/admin/*`（新）+ `/admin/api/*`（旧，保留兼容）+ `/api/user/*` |

**⚠️ 路径前缀两边刚好颠倒**（`/admin/api` vs `/api/admin`）。Rust 版**必须选一个并写进契约文档**，
P2 抄 kiro 的面板 JS 时这是最容易出的一类静默错误（前端 baseURL 改错 ⇒ 整片 404）。

---

## 1. 总览裁定表

> 详细理由见后面同编号小节。

| # | 功能面 | agnes2api | kiro2api | 谁强 | 裁定 |
|---|---|---|---|---|---|
| A1 | 前端协议条数 | 4（OpenAI Chat / Anthropic Messages / OpenAI Responses / Gemini 原生） | 4（同上） | 平 | 保留 |
| A2 | 协议端点覆盖（count_tokens / 模型列表 / 别名前缀） | 缺 `count_tokens`、缺 `/claude/*` `/openai/*` `/gemini/*` 别名、无 Anthropic 专属 models 端点 | 全有 | kiro | **对标** |
| A3 | 请求体转换深度（工具调用 / 多模态 / thinking） | 三条非 OpenAI 协议**显式拒绝** `tools`/`tool_choice`，内部规范格式只有纯文本 | 全量双向转换（tool_use/tool_result/thinking/图片/PDF/web_search） | kiro | **对标（有上游前提，见 A3）** |
| A4 | 图片生成 / 视频生成 | 有（`/v1/images/generations`、`/v1/videos` 建任务 + `/v1/videos/:id` 轮询） | 无 | agnes | **保留** |
| A5 | 流式（SSE）与超时分档 | 有，`firstByte` / `sync` 两档预算 | 有，无同类分档 | agnes 略强 | **保留** |
| A6 | CORS | 无 CORS 层 | 有，协议面默认放开、管理面默认关 | kiro | **对标** |
| A7 | `/health` | 有，且反映存储可写性、不可写回 503 | 有，恒 `{"status":"ok"}` | agnes | **保留** |
| A8 | `/v1/ping` | 无 | 有 | kiro | 对标（成本≈0） |
| B1 | 上游凭据的形态 | 一把明文 API key，无刷新概念 | OAuth 刷新令牌 + 三种交互式登录 + ksk | 不可比（上游不同） | **保留 agnes 形态** |
| B2 | 负载均衡策略 | 单一：游标轮询 | 两种：`priority`（粘滞 + 优先级）/ `balanced`（按权重轮询），运行期可切 | kiro | **对标** |
| B3 | 优先级 / 权重字段 | 无 | `priority: i64` + `weight` | kiro | **对标** |
| B4 | 每凭据 RPM 限速 | 无 | `max_rpm_per_credential`，0=不限 | kiro | **对标** |
| B5 | 逐模型「这个号不支持这个模型」记忆 | 无 | 有（`unsupported_models`，跳过后再选） | kiro | **对标** |
| B6 | 跨 key 重试 | 有（一轮最多 `records.length` 次） | 有（自适应上限 + 排除集） | 平（各有取舍） | **合并** |
| B7 | 冷却 / strike / 剔除分档 | 有，429/402/401/403 四类分流 + `MAX_STRIKES` 长冷却 | 有，`FailureKind` + `StatusReason` 更细（含按响应体判 banned/quota） | kiro 略强 | **合并** |
| B8 | 全池判停时的自愈 | 无（靠冷却自然到期） | 有（`pool_self_heal`，仅放回运行期判停的） | kiro | **对标** |
| B9 | 手工停用 vs 系统剔除语义分离 | 有（`disabled` 与 `evicted` 严格分离，且 `countsTowardTarget` 单独一条判据） | 有（内存态 `Entry.disabled` vs 持久 `cred.disabled`） | 平 | **保留** |
| B10 | 批量操作 | 有（`disable` / `clearCooldown` / `delete` 三种，`POST /keys/bulk`） | 有（批量导入；启停是逐条） | agnes 略强 | **保留** |
| B11 | 批量 / 多格式导入 | 有（`POST /keys`，上限 200） | 有（`batch-import`，接受数组 / KAM `{accounts}` / 单对象，逐项韧性） | kiro | **对标（多格式容错）** |
| B12 | 凭据明文回显 | 有（`GET /keys/:id/reveal`） | 有（`server-info` 回主 key 明文供复制） | 平 | **保留** |
| B13 | 单把 key 验活 | 有（`POST /keys/:id/verify`，**零存储写**） | 有（`{id}/refresh` 强制换发令牌） | 平 | **保留** |
| B14 | 每凭据出站代理 | 无 | 有（`proxy_url` 全局 + 凭据级覆盖 + `"direct"` 退回） | kiro | **对标** |
| B15 | TLS 后端可切 | 无 | 有（`tls_backend`: native-tls / rustls，改配置重启生效） | kiro | 对标（Rust 侧近乎白送） |
| C1 | 上游余额 / 额度查询 | **无**（上游有没有这条接口未核实，见 §9 未定项） | 有（`getUsageLimits` + 快照缓存 + 逐账号 `{id}/balance` + `credits/global` 全局聚合 + 订阅档位） | kiro | **对标（阻塞于未定项）** |
| C2 | 配额真实重置时刻 | 无 | 有（`set_quota_reset`，到点自动回池） | kiro | 对标（同上） |
| C3 | 成本估算（USD / credits） | 无 | 有（`stats/pricing.rs` 按模型名分档，逐条记 `estimated_cost`） | kiro | **对标** |
| D1 | 对外 API 密钥（我们签发的那一族） | 有，**7 字段** | 有，**11 字段** | kiro | **对标（补 4 格）** |
| D2 | 密钥过期 | 无 | 有（绝对 `expires_at` + 惰性激活 `duration_days`/`activated_at`） | kiro | **对标** |
| D3 | 密钥消费上限 | 无（TS 侧明写「做不出跨实例正确的上限」而砍掉） | 有（`spending_limit` + `limit_unit` usd/credits + 在途预留 reserve-then-reconcile） | kiro | **对标（Rust 单进程，前提已变）** |
| D4 | 密钥绑定账号白名单 | 无（TS 侧明写「本仓没有对应概念」） | 有（`bound_credential_ids`，鉴权闸解析、选号层 fail-closed 执行） | kiro | **对标** |
| D5 | 密钥明文存储 | 有（`secret?` 可选字段，用户拍板以安全换便利） | 有（`key` 必存明文） | 平 | **保留** |
| D6 | 密钥用量归属 | 有（`byApiKey` 维度，桶聚合） | 有（逐条 `UsageRecord.api_key_id`，可查记录、可重置） | kiro | **对标** |
| E1 | 用量统计形态 | 聚合桶（日键 + 值里 24 小时槽），维度 `byModel`/`byProtocol`/`byApiKey`，**默认关**（`USAGE_STATS_ENABLED`） | 逐条记录（`UsageRecord`：模型、in/out token、cache token、cost、credits、client_ip、latency_ms、api_key_id、credential_id），**默认开** | kiro | **对标（改逐条 + 默认开）** |
| E2 | 延迟统计 | 有（只累计成功请求的 `latencySum/latencyCount`，明确放弃 P95） | 有（逐条 `latency_ms`，汇总算 `avgLatencyMs`） | 平（kiro 可算分位数） | **合并** |
| E3 | 每日汇总 / 按日期查记录 | 有（`GET /usage/:date`） | 有（`usage/daily`、`usage/summary`、`usage/daily/{date}/records`） | kiro | **对标** |
| E4 | 实时 RPM | 无 | 有（`GET /api/admin/rpm`，逐凭据 + 峰值） | kiro | **对标** |
| F1 | 事件 / 日志采集面 | **只采白名单诊断事件**（约 60 个事件名，全是故障/审计），无逐请求日志 | **全量 tracing 捕获**（每请求一条 INFO + 生命周期事件） | kiro | **对标（换成全量捕获）** |
| F2 | 实时流式日志 | 无 | 有（SSE `logs/stream`，含历史回放 + 心跳 + Lagged 处理；`?api_key=` 携带凭据） | kiro | **对标** |
| F3 | 日志持久化 | **持久**（写存储，重启不丢，带 TTL 有界） | **内存环形缓冲**（`log_capacity` 默认 5000，重启即丢） | agnes | **保留（两轴合并：全量 + 持久）** |
| F4 | 日志下载 | 有（`events/download`） | 有（`logs/download`，txt 附件） | 平 | 保留 |
| F5 | 401/403 / 429 专门事件流水 | 无（混在通用事件里） | 有（逐凭据 `failure-logs` / `throttle-logs`，带截断的响应体） | kiro | **对标** |
| G1 | 模型目录 | 12 个（6 chat / 3 image / 3 video），带 modality + 协议矩阵 | 静态 `models_catalog.rs` + 逐凭据动态 `ListAvailableModels` 缓存 | 平（形态不同） | **合并** |
| G2 | 上游模型清单拉取 | 有（`GET /upstream/models`，实拉上游） | 有（`credentials/models/refresh` 全池 + 逐凭据 + 回填缓存） | kiro | **对标（逐凭据回填）** |
| G3 | 逐模型连通性测试 | 有（`POST /models/:id/test`，后端发起） | 无同类（modeltest 是前端自己打转发端点） | agnes | **保留** |
| G4 | 调试台 / Playground | 有（4 协议 × chat/image/video 模式、多轮对话、流式、取消、媒体结果预览、网关口令与管理口令严格隔离；`sec-playground.js` 1,721 行） | 有（`sec-modeltest.js` 911 行：2 端点（OpenAI chat / Anthropic messages）、多轮对话、流式、token 统计、可选主 key 或已建 API Key） | agnes（协议与模态覆盖更宽） | **保留（但 UI 壳对标 kiro）** |
| H | 面板板块 | 9 个 | 8 个 | — | 见 §7 逐个对应表 |
| I | **注册机（自动铸 key）** | **有**（`src/core/registrar/` 10 文件 / 3,664 行 + 2 个邮箱通道适配器 560 行：临时邮箱 → 域名台账 → 注册 → 等验证码 → 登录 → 建 token；定时轮 + 手动轮两族独立预算、跨轮退避、补池锁、域名黑白名单台账、补池历史） | **无任何可参照物** | agnes 独有 | **保留（P3，用户明确要求）** |
| J1 | 配置来源与优先级 | 环境变量 > 存储中的运行期配置 > 默认；有 `FIELD_EXPOSURE` 曝光度分级与 provenance（面板显示「哪些字段被 env 锁死」） | config.json > 环境变量（仅少数几个）> 默认 + CLI `-c/--credentials` | agnes | **保留** |
| J2 | 运行期可改的配置项 | **25 项**（`config-validate.ts` 的 `EDITABLE` 表实数；含全部 11 项注册机参数、超时、冷却、凭据三把），带类型校验 / 清空凭据 / 一键重置 | **2 组**（`load-balancing`、`auth-keys`） | agnes | **保留** |
| J3 | 启动期 fail-closed | 有（缺 `GATEWAY_TOKEN` 拒绝启动；`ADMIN_TOKEN` 未设则 `/admin` 整棵树不注册；<24 位不启用；与网关口令相同则管理接口恒 503） | 无（未设 key 则协议面/管理面**开放访问**，只打 warn） | agnes（安全姿态强得多） | **保留** |
| J4 | CLI 参数 | 无 | 有（clap，`-c` 配置路径、`--credentials`） | kiro | 对标（成本≈0） |
| K1 | 部署形态 | Docker 单一形态（P0 已砍 CF Worker） | Docker + 裸机二进制 | kiro | **对标（Rust 天然）** |
| K2 | 容器内更新 / 重启 | 无 | 有（`check-update` 查 GitHub Releases、`update` 回更新命令、`restart` 带二次确认） | kiro | **对标** |
| K3 | 服务器信息面 | 有（版本 / 运行时 / 内存 / uptime / PID / 存储后端 + 可写性） | 有（版本 / 客户端版本 / host / region / uptime / OS / 内存 / CPU / 运行模式 docker-or-bare / PID / Rust 版本 / 服务器时间） | kiro（面更宽） | **合并** |
| K4 | 反代跳数可信 IP | 有（`TRUST_PROXY`） | 有（`trusted_proxy_hops`，0/1/2 语义写死在注释里） | 平 | 保留 |
| L1 | 面板多语言 | 5 种（zh-CN / zh-TW / en / ja / ko），词典 1,420 行 | 5 种（同上），词典 712 行 | agnes（覆盖更细） | **保留** |
| L2 | 文档语言与篇目 | 5 语言 × 7 篇（README/USAGE/API/DEPLOY/ADMIN/REGISTRAR/SPONSORS） | 5 语言 × 5 篇（README/USAGE/API/DEPLOY/SPONSORS） | agnes | **保留** |
| M1 | **用户面门户**（key 持有者自助） | **无** | **有**（`user-ui/` React+Vite SPA，`/user` 路由；`POST /api/user/login` 以 key 自鉴权 + 用量汇总 + 分页记录 + 消费上限进度条） | kiro | **对标（P2 末尾或 P2.5）** |
| M2 | 上游服务端工具（web_search / MCP） | 无（上游无此概念） | 有（`kiro/mcp.rs` JSON-RPC over HTTP） | 不可比 | **不做**，理由见 §8 |
| M3 | 令牌提前续期（keepalive） | 无（agnes 的 key 不过期） | 有（只续活跃账号，刻意不做全池定时刷新） | 不可比 | **不做**，理由见 §8 |

---

## 2. 协议与端点（A 组）

### A1 协议条数：平手

两边都是四条前端协议。

| 协议 | agnes2api 路由 | kiro2api 路由 |
|---|---|---|
| OpenAI Chat | `POST /v1/chat/completions`、`GET /v1/models` | 同 + `/openai/v1/*` 别名 |
| Anthropic Messages | `POST /v1/messages` | 同 + `POST /v1/messages/count_tokens` + `GET /claude/v1/models` + `/claude/v1/*` 别名 |
| OpenAI Responses | `POST /v1/responses` | 同 + `/openai/v1/responses` |
| Gemini 原生 | `GET /v1beta/models`、`POST /v1beta/models/:rest{.+}` | `GET /v1beta/models`、`POST /v1beta/models/{model_action}` + `/gemini/v1beta/models` |

出处：`agnes2api/src/http/routes/{openai,anthropic,responses,gemini}.ts`；
`kiro2api/src/protocol/{openai,anthropic,responses,gemini}/handler.rs` 各自的 `*_router()`。

**裁定：保留。** 两边条数一致，agnes 侧额外有媒体三条（见 A4）。

### A2 端点覆盖：对标

agnes 缺三样，都有实际后果：

1. **`POST /v1/messages/count_tokens`**：Anthropic 官方 SDK 与 Claude Code 一类客户端在正式请求前会先探这条。
   agnes 侧完全没有，客户端拿到 404。
2. **别名前缀**（`/openai/v1/*`、`/claude/v1/*`、`/gemini/v1beta/*`）：让一个网关地址同时喂多个只认固定基址的客户端。
   kiro2api 每条协议都注册了两份路径。
3. **`GET /claude/v1/models`**：Anthropic 侧的模型列表。

**⚠️ 一条实测的坑（agnes 侧已知）**：agnes 的 Gemini 路由用 `:rest{.+}` 通配，
于是 `:countTokens` / `:embedContent` / 拼错的 `:generatecontent` **全都命中 generateContent**，
带合法 `contents` 打 `:countTokens` 返回 **200 + 一段真回答**（`src/http/routes/gemini.ts:45-54`）。
**Rust 侧照 kiro 用 `{model_action}` 显式路由时必须把 action 解析出来做白名单，别把这个坑一起搬过去。**

### A3 转换深度：对标，但有上游前提

这是两边差距**最大**的一处，也是最要小心别乱承诺的一处。

- agnes 侧：`src/core/protocol/` 共 953 行。三条非 OpenAI 协议的转换器对 `tools` / `tool_choice`
  一律 `throw new UnsupportedParamError(...)`（`anthropic.ts:57-58`、`gemini.ts:60-61`、`responses.ts:54`）。
  内部规范格式**只有纯文本**，`image` / `inlineData` / `input_image` / `tool_use` 这些块整个不进（`request-shape.ts:81`）。
- kiro 侧：`src/protocol/**` 共 16,119 行，`src/kiro/convert.rs` 里 `tool_use`/`tool_result` 出现 138 处，
  `thinking` / `redacted_thinking` / `document` / `search_result` 块全部解析，还有 `websearch.rs`。

`request-shape.ts:106-138` 记着 agnes 为什么这么干，值得逐字带进 Rust 版：

> 实测最难看的一幕：同一份带 `tools` + `tool_choice:"required"` 的请求，走 OpenAI 那条上游返回**真的工具调用**
> （`finish_reason:"tool_calls"`、`arguments {"city":"Paris"}`），走 `/v1/messages` 返回 **200**、
> 正文是模型自己吐的 `<tool_call><tool_call>…` 乱码。

**裁定：对标 —— 但这条**必须**排到 P1 之后，且开工前先核一件事：
agnes 上游是 OpenAI 兼容的 `/chat/completions`，`tools` 在 OpenAI 那条路上**已经能透传并真的工作**。
所以要做的不是「从零发明工具协议」，而是「把 Anthropic/Gemini/Responses 三种工具 schema 双向映射到 OpenAI 那一种」。
这是**有确定形状、可分步做**的活，不是空头支票。多模态（图片输入）同理，取决于上游 `/chat/completions` 认不认
`image_url` 块 —— **这一点我没核实，列入 §9 未定项。**

### A4 媒体生成：保留（agnes 独有）

`POST /v1/images/generations`（同步）、`POST /v1/videos`（建任务）、`GET /v1/videos/:id`（轮询）。
三条路径的「对外那一半」与「上游那一半」都住在 `src/core/admin/protocol-catalog.ts` 的 `MEDIA_ENDPOINTS` 里，
路由文件只是消费者（`src/http/routes/media.ts:14-21`）。**Rust 版照搬这个「一张表两半」的形状**，
Playground 的媒体模式是它的第二个消费者。

kiro2api 完全没有媒体生成 —— 上游 Kiro 不提供。**保留，无争议。**

### A5 超时分档：保留

agnes 的 `TimeoutProfile` 有 `firstByte`（默认 8 秒，流式与轮询用）与 `sync`（图片/视频建任务用，实测图片耗时 11.99 秒）两档；
同步档的预算**跨 key 共享**：`syncDeadline` 决定客户端等待上界，`syncAttemptBudget` 决定单次尝试占多少，
剩下的留给换 key 重试（`src/core/dispatcher.ts:377-384`）。kiro2api 没有同类分档（它没有同步长接口）。

**保留。** 这一套是媒体生成的必要配套，砍了图片生成就会被截断。

### A6 CORS：对标

agnes 全仓没有 CORS 层（`grep -rn 'cors\|Access-Control' src/` 只命中注释；`src/ui/serve.ts:12` 明写
「不加 CORS：面板同源，加了只会扩大攻击面」—— **那句话只覆盖面板，没覆盖四条协议端点**）
⇒ 浏览器里的网页前端打四条协议端点时，预检 `OPTIONS` 落到路由上吃 405，
随后的请求被浏览器掐掉。kiro2api 的 `config.rs` 对这件事写了完整论证：
协议面 + `/health` 默认 `["*"]` 放开（鉴权材料一律走请求头、不用 Cookie，浏览器不会替攻击者自动附凭据），
管理面/用户面**默认关**（未配 key 时管理面是完全开放的，此时再放开跨源读取等于任意恶意网页都能读走凭据）。

**裁定：对标，连这条「协议面开 / 管理面关」的非对称默认一起搬。**

### A7 `/health`：保留

agnes 的 `/health` 反映存储可写性，不可写回 **503**，理由写在 `src/http/routes/health.ts:22-27`：
镜像内置 HEALTHCHECK 按响应判定，恒 200 的话容器一直报 healthy 而每次 API 调用都在失败。
且**刻意不回显底层异常信息**（`/health` 不鉴权）。kiro2api 的 `/health` 是恒定三字段 JSON。

**保留 agnes 这条。**

---

## 3. key 池 / 账号池调度（B 组）

### B1 凭据形态：不可比，保留 agnes 的

kiro2api 的「账号」是 OAuth 刷新令牌，所以它有一整套 agnes 用不上的东西：
`refresh.rs` / `ensure_fresh.rs` / `keepalive.rs` / 三种交互式登录（BuilderID 设备流、IAM SSO、直接贴 SSO token）
/ `machine_id.rs` 身份伪装。agnes 的「key」是一把明文 API key，不过期、不刷新。

**裁定：保留 agnes 形态。** 面板上「添加账号」的交互因此不同 —— kiro 是「三种登录方式 + 批量导入」，
agnes 是「粘贴 key 批量导入 + 注册机自动铸」。**这是 P2 对标 UI 时必须明确取舍的一处**，
不能照抄 kiro 的登录弹窗（那三种流在 agnes 侧没有对应物）。

### B2–B5 调度维度：全面对标

agnes 的 `selectKey`（`src/core/keypool.ts:97-111`）是**十四行**：从游标开始线性扫，
第一个 `isAvailable`（非剔除、非停用、冷却已过）的就用。没有优先级、没有权重、没有粘滞、没有 RPM。

kiro 的 `select_excluding_for_model`（`src/kiro/pool.rs:775-880`）是同一件事的完整版：

- 过滤链：`is_active` ∧ `rpm_ok` ∧ `supports_model` ∧ `!excluded`
- `LbMode::Priority`：**粘滞** —— 当前账号还能用就继续用它不换；换人时取**优先级最小**的可用账号
  （注释明写「而不是 cursor 的下一个 —— 后者会让每次换人都往后漂，长期看又退化成轮询」）。
  上游看到的是「一个账号连续干活、坏了才换人」，不是几百个身份在同一个 IP 上交替刷屏。
- `LbMode::Balanced`：在可用集合上按 `effective_weight()` 展开加权轮询。
- `unsupported_models`：记住「这个号不支持这个模型」，注释里记着实测数字 ——
  不跳的话混着订阅档位时每个 opus 请求要撞 **13 次上游 400** 才轮到支持它的那个。

**裁定：B2/B3/B4/B5 全部对标。** 其中 **`priority` 粘滞是 P1 必须做的**（用户记忆里也钉着
「`loadBalancingMode` 必须 priority」），它对上游风控的影响是实打实的；B4/B5 可以 P1 留字段、P2 接面板。

### B6 跨 key 重试：合并

两边都有，机制不同：

| | agnes | kiro |
|---|---|---|
| 上限 | `attempts = records.length`（整池一遍） | 自适应（`select_and_call_with_retry`，确定性失败另有配额） |
| 已试过的排除 | 靠游标推进 + 本请求内 `records[at]` 视图更新 | 显式 `exclude_ids: HashSet<String>` |
| 同步档预算 | 有（`syncDeadline` 跨尝试共享） | 无 |
| 不重试的致命错 | 转换错误直接 400 | 同（`cross_account_convert_error_no_retry`） |

**裁定：合并。** Rust 版用 kiro 的 `exclude_ids` 显式排除（比靠游标推进更难写错），
但**保留 agnes 的同步档共享预算**（媒体生成需要它）。

### B7 失败分档：合并

agnes：`429`→`cooldownRateLimitMs`、`402`→`cooldownPaymentMs`、`401`/`403`→永久剔除、
瞬时故障累计到 `MAX_STRIKES`→`cooldownStrikeMs` 长冷却。四档，判据只看状态码。

kiro：`classify(status)` + `classify_with_body(status, body)` + `StatusReason`
（含 `Banned` / quota / `body_signals_auth_invalid` / `body_signals_recoverable` / `body_signals_quota`），
**按响应体内容再分一层**。

**裁定：合并 —— 骨架用 agnes 的四档（它与 Agnes 上游的实测行为对齐），
但把「按响应体再判一层」这个机制搬过来**，因为 agnes 侧已经有同型需求的证据：
`registrar` 那边就有 `send_code_unclassified` / `register_rejected` 这类按体判的事件名。

### B8 全池判停自愈：对标

kiro 的 `pool_self_heal`（`pool.rs:800-828`）：全池皆停时，把**仅因运行期判停**的账号放回来重试一轮，
持久禁用 / 上游确证封禁 / 未到恢复时刻的配额**一律不放**。

agnes 没有对应物。它的对应处境是「整池都在冷却」，而 agnes 对此的处置是**刻意不自愈**
（`keypool.ts:83-92` 明写代价：「网关在冷却窗口内持续 503，最长一个 `COOLDOWN_PAYMENT_MS`」，
理由是那时候上游要么在限流要么在故障，铸新号打的是同一个后端）。

**裁定：对标，但不许混淆两件事。** kiro 的自愈针对的是**运行期误判**（上游抖动把好号判停），
agnes 的「不自愈」针对的是**补池**（不许因为冷却去铸新号）。两条可以并存：
「冷却不触发铸币」保留，「全池运行期判停时放回一轮」新增。

### B9 停用/剔除语义分离：保留

agnes 这一处的注释（`keypool.ts:3-96`）记着一条实测缺陷，**必须逐字带进 Rust 版**：

> `isAvailable` 与 `countsTowardTarget` 曾经是同一个函数……`disabled` 落地的那一刻它就出事了：
> `tendOnce` 拿 `targetKeys - available` 当缺口，差多少就真的去注册多少个账号。
> 让 `isAvailable` 顺手回答这个问题，等于**「在面板上停用一把 key」＝「自动注册一个新账号」**。

以及第二条：冷却占不占名额。`countsTowardTarget` 的判据是 `!r.evicted`，**一个字都不多**，
否则「每一次全池风暴永久 `+targetKeys`」，实测 3→6→9→12 线性不封顶。

**这两条只在有注册机的系统里成立，kiro2api 没有可参照物。保留，且 P3 移植注册机时是头号红线。**

### B10/B11 批量操作与导入

- agnes 有 `POST /admin/api/keys/bulk`，三种 op：`disable` / `clearCooldown` / `delete`
  （`src/http/admin/handlers/keys-write.ts:468`）；还有 `PATCH /keys/:id` 的六个字段
  （`disabled` / `note` / `clearCooldown` / `clearStrikes` / `unevict` / `clearStats`）。kiro 的启停是逐条的。→ **保留**
- kiro 的 `batch-import` 接受**数组 / KAM `{accounts}` 包装 / 单对象**三种形状，逐项规整 + 校验 + 入池落盘，
  返回逐项结果与计数（`admin/mod.rs:38-44`）。agnes 的导入只认一种。→ **对标这条容错**

### B14/B15 代理与 TLS 后端：对标

kiro `config.rs` 对 `tls_backend` 写的理由值得照搬：走自签 CA 的代理时往往只有 native-tls 或 rustls
其中一个能握上手，而现象是「刷不出令牌」或「直接连不上」，与 TLS 毫无字面关系 —— 此前是编译期二选一，
换后端得重新出镜像，「排查时最不该卡在这种地方」。

agnes 两样都没有。**对标。** 对一个要在被墙环境下打上游的网关来说，出站代理几乎是必需品。

---

## 4. 余额 / 额度 / 成本（C 组）

### C1 上游余额：对标，但阻塞于一个未核实的前提

kiro2api 的 `src/balance/` 是完整一套：
`client.rs` 打上游 `getUsageLimits`；`model.rs` 定义累加口径（基础额度 + `ACTIVE` 的 bonus + `ACTIVE` 的 free trial，
全取 `*WithPrecision` 精确值）；`cache.rs` 做快照缓存 + 落盘 + 逐 id 取；
暴露 `GET /credentials/{id}/balance` 与 `GET /credits/global`（零上游、只读共享缓存的全局聚合）。
面板仪表盘上「全局剩余积分」那张卡直接吃它，还带「基于 {cached}/{total} 个已缓存账号」「更新于 {ago} 前」的诚实标注。

agnes2api **完全没有余额概念** —— 全仓 `grep -rn '余额\|积分\|credits' src/core/admin/ src/http/admin/handlers/` 零命中。

**裁定：对标 —— 但这是本文档里唯一一条我无法确认可行性的「对标」。**
它成立的前提是「Agnes 上游有一条能查当前 key 剩余额度的接口」，而 **agnes2api 的源码里没有任何证据表明存在这样一条接口**。
agnes 侧今天是靠**上游回 402 才知道欠费**（`cooldownPaymentMs` 那一档）—— 这是「事后」，不是「余额」。
⇒ 列入 §9 未定项，**P2 做仪表盘之前必须先有答案**，否则那张卡只能显示「上游不提供」。
**诚实提醒：如果上游真没有这条接口，那 C1/C2 以及仪表盘上「全局剩余积分」「今日积分」两张卡就是做不出来的，
不许用一个恒为 0 或恒为 null 的字段冒充。**

### C3 成本估算：对标

kiro 的 `stats/pricing.rs` 按模型名前缀分档（opus $5/$25、haiku $1/$5、默认 sonnet $3/$15，每百万 token），
逐条记 `estimated_cost`，喂给仪表盘「今日花费 (美元)」和 API Key 的消费上限。
它自己也如实写明：`output_tokens` = 响应字符数/4，`input_tokens` 也是估算，「先把字段接通，精度随 token 计量改进而提升」。

agnes 侧没有任何定价表。但 agnes 的用量桶里**有 `tokensIn` / `tokensOut`**
（`src/core/admin/usage-stats.ts:114` 附近，注释写明「只有非流式、且协议路由传了 `expectJson` 的那三条才有 token」）。

**裁定：对标。** Agnes 上游是 OpenAI 兼容接口，`usage` 字段大概率是真实 token 数（比 kiro 的字符数/4 更准），
所以这一格 agnes 版做出来会**比 kiro 更准**。定价表本身要按 Agnes 的模型清单自己定（12 个模型 3 种模态），
**不许照抄 Anthropic 的价格**。

---

## 5. 对外 API 密钥（D 组）

`agnes2api/src/core/admin/api-keys.ts:76-82` 这段注释是本次对比里最省事的证据 —— agnes 自己就写着与 kiro 的差：

> kiro2api 那边是十一个字段，砍掉的四个各有各的理由，逐条写在五份 `docs/<lang>/ADMIN.md` 的「API 密钥」一节里
> （`spendingLimit` / `limitUnit` 做不出跨实例正确的上限、`durationDays` / `activatedAt` 的惰性激活要在鉴权热路径上写存储、
> `boundCredentialIds` 在本仓**没有对应概念**）。

逐条重新裁定：

| kiro 字段 | agnes 砍掉的理由 | 重写后这条理由还成立吗 | 裁定 |
|---|---|---|---|
| `spending_limit` + `limit_unit` | 「做不出跨实例正确的上限」 | **不成立了。** 那条理由的前提是 Cloudflare Worker 多实例 + KV 无 CAS。P0 已砍 Worker，Rust 版是单进程 + `parking_lot::RwLock`，kiro 的 reserve-then-reconcile（`server/auth.rs`）正是为这个形态设计的 | **对标** |
| `duration_days` + `activated_at`（惰性激活） | 「要在鉴权热路径上写存储」 | **不成立了。** kiro 的做法是内存改 + 约 5 秒去抖批量刷盘（`apikey/mod.rs:5-7`），热路径零 I/O | **对标** |
| `bound_credential_ids` | 「本仓没有对应概念」 | **半成立。** agnes 的 key 池确实没有「档位」之分（不像 kiro 混着 FREE/付费账号），但「把某把对外密钥限定在某几把上游 key 上」在 agnes 里照样说得通（例如给不同用户分不同批次的 key） | **对标**（优先级低于上两条） |

另外 **D6 用量归属**：agnes 有 `byApiKey` 维度但只有**聚合桶**，查不了「这把 key 具体发过哪些请求」；
kiro 有 `api-keys/{id}/usage/records` 分页查记录 + `DELETE .../usage` 重置用量。→ **对标**（随 E1 一起做）。

**D5 明文存储保留。** agnes 的 `secret?` 是用户 2026-09-10 拍板的取舍，注释里如实记着两条代价
（面板被打穿则明文一次性泄漏、存储介质处置级别要升）。kiro 本来就存明文。两边一致，**保留，并把那段代价说明一起搬过去**。

---

## 6. 用量统计 / 日志与事件（E、F 组）

### E1 统计形态：对标（这是个结构性改动）

| | agnes | kiro |
|---|---|---|
| 粒度 | 聚合桶：`stat:<YYYY-MM-DD>` 一个键，值里装 24 个小时槽 | 逐条 `UsageRecord` |
| 维度 | `hours` / `byModel` / `byProtocol` / `byApiKey`（都有键数与键长上界，溢出并进 `other` 桶） | 逐条带 credential_id / api_key_id / model / client_ip / latency_ms / cost / credits |
| 默认 | **关**（`USAGE_STATS_ENABLED !== "true"` 则一次存储访问都没有、一个内存累加器都不建） | **开** |
| 写预算 | 有（`USAGE_WRITES_PER_DAY = 13`，一天写不过 13 次） | 无（本地文件，约 5 秒去抖批量刷盘） |

agnes 这套「日键 + 小时槽 + 写预算 + TTL 有界」的形状，**整个是为 Cloudflare KV 的写配额设计的**
（`usage-stats.ts:20-30` 逐字算过：按小时分键的话真实写量 192–288/天、最坏 1,040，而 DEPLOY.md 自己管 1,040 叫「已经打穿写配额」）。

**P0 已经把 Worker 砍了 ⇒ 这条设计约束整个消失。** Rust + 本地文件 + 去抖批量刷盘之后，
逐条记录完全负担得起，而且它能算的东西是聚合桶算不出来的：分位数延迟、按 IP 看、按账号看、翻明细。

**裁定：对标 kiro 的逐条记录，默认开。** 保留 agnes 的「键长/键数上界 + 溢出并进 `other`」这条有界性纪律
（`byModel` 的键**完全由客户端在请求体里填**，`usage-stats.ts:346` 明写「这条上界不是防御性代码，是有界性本身」）。
**⚠️ 逐条记录同样需要有界性**，kiro 侧我没确认它有没有记录条数上限 —— 列入 §9。

### F1–F3 日志：合并（两边各强一轴）

| 轴 | agnes | kiro | 取谁 |
|---|---|---|---|
| 采集面 | 只采约 60 个白名单诊断事件，**没有逐请求日志** | 全量 tracing 捕获（每请求一条 INFO + 生命周期事件），`log_capacity` 默认 5000 | **kiro** |
| 实时性 | 无流式，面板轮询拉 | SSE `logs/stream`（先发 history 事件再逐条推，带心跳，处理订阅者 Lagged） | **kiro** |
| 持久性 | **持久**（写存储，带 TTL 有界，重启不丢） | **内存**（重启即丢） | **agnes** |
| 下载 | 有 | 有 | 平 |

**裁定：合并 —— 全量捕获 + SSE 实时流 + 持久化。**

一条必须搬过去的安全约束（agnes `src/adapters/logger-store.ts:96-104`）：
白名单里的 `admin.login_failed` 是**任何未鉴权请求都能触发**的，所以落盘必须受最小间隔闸节流，
否则攻击者可以按批驱动写。Rust 版改成全量捕获之后这条约束**变得更要紧**（面更宽了），
落盘侧必须有节流 + 容量上界。

**SSE 鉴权那条坑也要一起搬**：`EventSource` 无法设请求头，kiro 用 `?api_key=` query 携带
（`admin/mod.rs:106`）。**⚠️ 这意味着管理口令会进 URL ⇒ 进反代访问日志。** kiro 接受了这个代价，
agnes 版要么照做并在文档里写明，要么换成先换一次性短票再连 SSE。**这是一处需要主线裁的取舍**，见 §9。

### F5 失败/限流专门流水：对标

kiro 的 `FailureEvent`（401/403，响应体截断 2000 字符）与 `ThrottleEvent`（429，截断 200 字符）
是逐凭据独立的两条流水（`stats/model.rs`），面板上点进某个账号能直接看它被拒的原文。
agnes 侧这类信息只有 `KeyRecord.lastErrorKind` 一个字符串。**对标。**

---

## 7. 面板板块逐个对应（H 组）

kiro2api `admin-ui-v2/index.html:88-109` 八个 `data-section`；agnes2api `admin-ui/index.html:93-101` 九个。

| kiro 板块 | agnes 对应板块 | 功能差（读代码得出） | 裁定 |
|---|---|---|---|
| `dashboard` 仪表盘 | `overview` 概览 | **kiro 强**：账号总数/活跃、今日请求、今日花费(USD)、今日积分、可用模型、**全局剩余积分**（带缓存基数与新鲜度）、账号状态健康档、**按模型请求(今日)**、**最近活动**、峰值 RPM、检查更新/更新服务、系统信息（OS/内存/CPU/运行模式/PID/Rust 版本/服务器时间）。**agnes 强**：池四格（总数/可用/冷却/剔除/停用）、存储可写性、**新鲜度提示条**（明说「配置保存后最多多久生效」「别的实例判定的冷却最多晚多久」）、**被环境变量锁定的字段数**、降级横幅 | **UI 全面对标 kiro；内容取并集**（agnes 那几条诚实提示是它的长处，放进 kiro 的卡片布局里） |
| `accounts` 账号 | `keys` Key 池 | **kiro 强**：优先级、权重、余额/档位、逐账号用量记录/今日汇总/失败日志/限流日志、强制刷新令牌、逐账号模型清单刷新、三种交互式登录、多格式批量导入。**agnes 强**：批量操作三件套、`PATCH` 六字段（含 `clearStrikes`/`unevict`/`clearStats`）、零写验活、「必须先停用才能删」的安全约束 | **UI 对标 kiro；功能取并集**（交互式登录三件套**不搬**，agnes 无对应概念——换成「注册机」入口） |
| `models` 模型 | `models` 模型 | kiro：静态目录 + 逐凭据动态清单 + 刷新。agnes：12 模型 × modality × 协议矩阵 + 上游清单拉取 + **逐模型连通性测试**（后端发起） | **合并**：kiro 的「逐凭据刷新回填」+ agnes 的「协议矩阵 + 连通性测试」 |
| `apikeys` API 密钥 | `apikeys` API 密钥 | kiro 11 字段 + 过期 + 消费上限 + 绑定账号 + 用量记录 + 重置用量；agnes 7 字段 + 明文回显 + 一键清空 | **对标 kiro**（见 §5） |
| `modeltest` 模型测试 | `playground` 调试台 | agnes 强：4 协议 × chat/image/video、媒体结果预览、取消、口令一致性提示。kiro 强：无 | **保留 agnes 功能，UI 壳对标 kiro** |
| `usage` 用量 | `usage` 用量 | kiro：逐条记录 + 日汇总 + 按 key 过滤。agnes：桶聚合 + 按日期 | **对标 kiro** |
| `logs` 实时日志 | `events` 事件 | 见 §6 F 组 | **合并** |
| `settings` 设置 | `settings` 设置 | **agnes 强得多**：25 个可编辑项 + 类型校验 + 凭据清空 + 一键重置 + provenance（哪些被 env 锁死）。kiro：只有负载均衡模式 + 两把 key | **保留 agnes 功能，UI 壳对标 kiro** |
| — | `registrar` 注册机 | kiro **无对应物** | **保留（第 9 个板块）** |

**⚠️ 板块数结论：Rust 版是 9 个**（kiro 的 8 个逐一对上，外加「注册机」）。
`playground` 顶掉 `modeltest` 的位置，不额外多一个。

**UI 样式对标的具体含义**（用户「现在的太丑了」那句）：
kiro `admin-ui-v2/css/` 是 746 + 1470 + 678 + 560 + 11 = **3,465 行**，按板块族分文件（base / sections-manage / sections-stats / sections-tools，外加一个 11 行的 `sections.css` 聚合入口）；
agnes `admin-ui/css/` 是 226 + 600 + 251 = **1,077 行**，只有 base / shell / sections 三个笼统文件。
**差距在量级上，不是调色板问题。** P2 的正确做法是**把 kiro 的 CSS 整套拿过来当基线**，
再按 agnes 多出来的板块（注册机）与多出来的控件（协议矩阵、媒体预览）补写，
而不是在 agnes 现有 1,077 行上「改配色」。

---

## 8. 明确「不做」的项（逐条给理由）

| 项 | 理由 |
|---|---|
| **M2 MCP / web_search** | kiro 的 `kiro/mcp.rs` 打的是上游 Kiro 自带的 `/mcp` JSON-RPC 端点，用来调它的服务端工具。**Agnes 上游不存在这个端点**，搬过来是一个打不通的客户端。不是「暂缓」，是无对象。 |
| **M3 令牌 keepalive** | 前提是「凭据会过期、刷新要花时间」。agnes 的上游凭据是长期 API key，不过期、无刷新流程 ⇒ 无事可做。 |
| **B1 三种交互式登录（BuilderID / IAM SSO / SSO token）** | 同上，绑死 AWS Identity Center 与 Kiro 的设备授权流。agnes 侧对应的「怎么弄到新凭据」是**注册机**，那是完全不同的一套。 |
| **`machine_id` / UA 伪装** | kiro 要伪装成 Kiro 客户端（`kiro_version` / `system_version` / `node_version` 三段 UA + 每账号派生 machineId）。agnes 上游是公开的 OpenAI 兼容 API，**agnes2api 现有代码里没有任何伪装逻辑** ⇒ 没有证据表明需要。**如果后续实测发现上游按 UA 风控，再回头加**，不预先造。 |
| **`region` 配置** | kiro 的 region 决定上游端点域名（`q.{region}.amazonaws.com`）。agnes 的上游是单一固定基址（`AGNES_BASE_URL` 可改），无区域概念。 |
| **Cloudflare / KV 相关的一切**（写预算 `canWrite`/`consume`、分片键空间、`expiresAt` TTL、`waitUntil`） | P0 已砍 Worker。这些机制的**唯一存在理由**就是 KV 的写配额与平台会取消后台任务。Rust + 本地文件之后全部失去对象。**⚠️ 但它们背后那条纪律要留**：「有界性必须是存储自己的性质，不许绑在落盘节奏上」（`event-ring.ts:20-38` 那段被评审推翻两次的墓碑）。Rust 版换成「按条数/按天数硬性裁剪 + 启动时清理」，**不许换成「靠定时任务压实」**——那正是被推翻的形态。 |

---

## 9. 未定项 / 需要主线裁的（诚实清单）

1. 🔴 **Agnes 上游到底有没有「查余额/额度」的接口？** 这是 C1/C2 以及仪表盘两张积分卡的前提。
   我在 agnes2api 全仓没找到任何证据（零命中）。今天 agnes 只能靠 402 事后知道欠费。
   **没有答案之前，别把「全局剩余积分」画进 P2 的仪表盘设计稿。**
2. 🔴 **Agnes 上游 `/chat/completions` 认不认多模态输入块（`image_url`）？** 这决定 A3 里「图片输入」能不能对标。
   工具调用那一半是有把握的（OpenAI 那条路已实测能透传真工具调用），图片这一半我没核实。
3. 🟡 **SSE 日志流的鉴权方式。** kiro 用 `?api_key=` query（因为 `EventSource` 不能设头），代价是管理口令进 URL、进反代日志。
   **agnes 对这件事有一条成文的相反立场**：`admin-ui/js/gw-api.js` 的文件头写着
   「口令只走请求头，禁止查询参数……口令进 URL 会落进浏览器历史、Referer、各级访问日志」，
   并且有测试钉着「URL 上一个字节都没有」。再加上 fail-closed、`ADMIN_TOKEN` 最少 24 位、不从存储读这几条姿态 ——
   **照抄 kiro 等于当面推翻 agnes 自己钉过的一条纪律。**
   备选：先用一次 `POST` 换一张短时效一次性票，再拿票连 SSE。需要主线裁。
4. 🟡 **逐条用量记录的有界性。** 改成逐条之后，一台跑满的网关一天能落几万条。
   kiro 侧我没读到条数上限 / 滚动删除的代码（只读了 `stats/persist.rs` 的原子写与去抖刷盘接口名）。
   Rust 版必须自己定一条（按天分文件 + 保留 N 天？按条数环形？），**这条没定之前不许开工 E1**。
5. 🟡 **API Key 消费上限的计量单位。** kiro 是 usd / credits 二选一，两个常量都带着「实测多少、取多少倍余量」的论证。
   agnes 的定价表还不存在（见 C3），所以单位与预留值都要重定。
6. 🟡 **P2 的「账号」板块对「添加账号」的交互怎么设计。** kiro 那三种登录方式在 agnes 没有对应物，
   agnes 这一格是「粘贴导入」+「注册机自动铸」。**照抄 kiro 的弹窗会做出三个点不动的按钮。**
7. 🟢 **别名前缀要不要全套。** kiro 给每条协议都注册了 `/openai/` `/claude/` `/gemini/` 前缀。
   成本很低，但会让路由表翻倍。建议全做（对标），但值得确认一次。
8. 🟢 **`priority`/`weight` 这两个字段在 agnes 语境下的运营含义。** kiro 用它区分付费档/免费档账号。
   agnes 的 key 全是注册机铸出来的同一档，**短期内可能没有真实用途**。
   仍建议做（B2 的粘滞选号依赖 priority 排序），但面板上那一列的文案要想清楚说什么。

---

## 10. 建议的分期落点（供主线参考，非结论）

| 分期 | 本文档里对应的条目 |
|---|---|
| **P1（Rust 骨架：HTTP + 四协议 + key 池 + 鉴权）** | A1、A2（别名与 count_tokens）、A5、A6、A7、A8、B1、**B2/B3（priority 粘滞，必做）**、B6、B7、B8、B9、B12、B14、B15、J1、J3、J4、K1 |
| **P1.5（转发面补完）** | A3（工具调用映射）、A4（媒体三条）、B4、B5、B11、C3（定价表）、E1（逐条用量）、F1（全量日志捕获） |
| **P2（面板对标）** | H 全部九个板块、C1/C2（若 §9.1 有答案）、D1–D4、D6、E2–E4、F2、F4、F5、G1–G4、J2、K2、K3、L1 |
| **P2.5** | M1 用户面门户 |
| **P3** | I 注册机（含 B9 那两条红线） |
