# 06 · P2b 管理面板对标规格（agnes2api-rs `/admin`）

> **用途**：P2b（面板）开工前的唯一设计依据。回答四件事——
> ① kiro2api 的面板**长什么样**（细到能照着从零重建）；
> ② agnes2api 的面板**有什么是 kiro 没有的**、这些东西照 kiro 的设计语言**怎么长出来**；
> ③ agnes 那层 `js/pure/*.mjs` 与它背后 1,169 格判据**怎么落到 Rust 仓里**；
> ④ 面板**怎么进 Rust 二进制**、验收判据逐条是什么。
>
> **本文只写规格，不写代码。** 与 00–05 的关系：
> - `00-P1-spec.md` §2.1 给了 `src/webui/mod.rs` 的占位（「P2：本轮不存在」）——本文填它；
> - `02-http-contract.md` §8.2 给了 TS 版 `/admin` 静态树的契约——本文**逐条复核并登记两处偏离**；
> - `05-feature-matrix.md` §7 给了板块对应表的**骨架**——本文把它展开成可施工的粒度，并**修正其中一条**（见 §3.1 注）。
>
> **证据口径**：逐条来自读源码，不采信任何一边 README 的自述。
> 文件路径一律给全；行号对应 2026-09-11（北京时间）的工作副本，**引用时优先给选择器名/函数名**，
> 因为行号会漂。文中所有对比度读数是**我自己按 WCAG 2.x 相对亮度公式现算的**，
> 不是抄 agnes 注释里的数（算出来与它逐个吻合，这件事本身就是一次交叉验证——见 §2.4）。
>
> **标记**：🔴 = 改了会造成安全/正确性问题；⚠️ = 改了会静默坏掉或曾经真的坏过；
> **裁定 P-n** = 本文做出的、可被主线推翻的决定，逐条带理由与代价。

---

## 0. 一页速查：本文所有裁定

| # | 题目 | 裁定 | 一句理由 |
|---|---|---|---|
| **P-1** | CSS 基线 | **整套搬 kiro 的 4 份 CSS（3,465 行）当基线**，agnes 现有 1,077 行**全部丢弃** | 用户要「完全对标」；差距在量级不在配色，在 1,077 行上改配色改不出 kiro 的样子 |
| **P-2** | token 命名 | **用 kiro 的名字**（`--primary` / `--bg-primary` / `--text-secondary` / `--border-color` …），放弃 agnes 的 `--bg` / `--panel` / `--panel-2` / `--text` / `--muted` | 保留旧名就要写一层别名映射，而别名层是本仓反复吃亏的「第二份会漂的真源」 |
| **P-3** | 主色取值 | `--primary` = **`#059669`**（照 kiro）；**新增 `--primary-strong: #047857`**，只用在「主色当正文字色 / 实心按钮底」那几处 | `#059669` 配白字实算 **3.77:1**，不过 WCAG 1.4.3 AA；`#047857` 是 **5.48:1**，并排看几乎分不出。**这是一次刻意偏离「完全对标」，要主线裁**（§9 R-1） |
| **P-3b/c/d** | 语义三色的**正文字色**那一档 | 亮色 `--success-text` `#059669` → **`#047857`**、`--danger-text` `#dc2626` → **`#b91c1c`**；**新增 `--warning-text`**（亮 **`#92400e`** / 暗 `#fbbf24`，kiro 根本没有这一档） | P-3 当初**只按名字覆盖了 `--primary` 一个 token**：`--success-text` 与它**逐字节同值**，同一个病换个名字就绕过去了。warning 那一档一直直接借装饰色 `--warning`（白底 **2.15:1**），而它承载的正是 `.usage-unknown`——「这个数不可知」那个标记。逐条现算见 §2.4.1，偏离同 P-3，要主线裁（§9 R-1） |
| **P-3e** | 正文文字色**整面收口**（规则层） | 约 **110 处**正文字色从装饰色改指 `*-text` / `--primary-strong`；深色 `--text-tertiary` `#64748b` → **`#94a3b8`**；`.badge-off` 与 `.stat-icon.*`/`.status-dot` 两处**显式豁免并写明理由** | 前面几条补的都是 **token 本身**，而**「`--primary` 只做装饰」这半句从来没有人验过** —— 实况不成立（`.model-tag` / `.current-pill` / `.dash-rotation-badge` … 都拿它当正文字色），判据全绿而裁定在约 110 处被违反。逐簇现算见 §2.4.2 |
| **P-4** | 深色次要文字 | `[data-theme=dark] --text-secondary` 从 `#94a3b8` 改成 **`#aab8c9`** | kiro 那个值画在它自己的 `--bg-tertiary` 上实算 **4.04:1**（`.data-table th` 正是这一对），**kiro 自己不达标**。偏离同上，要主线裁（§9 R-1） |
| **P-5** | 板块数 | **9 个**：`dashboard/accounts/models/apikeys/playground/usage/events/settings` + **`registrar`** | kiro 8 个逐一对上，playground 顶掉 modeltest 的位置，注册机是第 9 个（05 §7 已定，本文沿用） |
| **P-6** | 板块 id 与导航文案 | **用 kiro 的 8 个 `data-section` 名**（`dashboard`/`accounts`/`models`/`apikeys`/`usage`/`logs`/`settings`）+ 新增 `registrar`；`modeltest` 改名 `playground` | 「样式和 UI 对标」包含导航条目的**名字与顺序**；agnes 的 `overview`/`keys`/`events` 三个名字退场。⚠️ 这会同时改 `localStorage` 的板块记忆键值域，见 §3.1 |
| **P-7** | HTML 骨架 | 照抄 kiro 的 `index.html` 结构（header/sidebar/content/login-overlay/toast/modalRoot），但 **🔴 删掉 `<base href="/admin/">`**，资源引用改成 `/admin/…` 绝对路径 | agnes 的 CSP 含 `base-uri 'none'`，`<base>` 会被**静默忽略** ⇒ 相对路径解析到 `/js/…` ⇒ 整份 CSS/JS 404。见 §7.4 |
| **P-8** | 模块体系 | **保留 agnes 的 ESM + `js/ui.js` 共用层**，不回退到 kiro 的 `window.K` 经典脚本 | 用户那条规则管的是「样式和 UI」；kiro 因为没有共用层，`openModal`/`confirmModal`/`svgIcon` 各有 3 份**不同的**实现（`agnes2api/admin-ui/js/ui.js` 文件头点名） |
| **P-9** | `js/pure/*.mjs` | **分三档处置**：A 档（业务判定）**上移 Rust**、B 档（本地化格式）留 JS 且去分支、C 档（浏览器交互状态机）留 JS 并**如实登记为无判据欠账** | Rust 仓的门禁只有 `cargo` 四道，留在 JS 的判据**没有执行器** ⇒ 等于引入一套无人跑、会静默腐烂的判据。详见 §5 |
| **P-10** | i18n 字典 | 五语言 key-major 不变；**字典文件体改写成「一整块合法 JSON」**（`export const I18N = { …纯 JSON… };`） | 浏览器仍可直接 `import`，而 Rust 侧能 `include_str!` + `serde_json` 严格解析 ⇒ 齐全性/占位符一致性判据**进 606 格体系** |
| **P-11** | 面板投递 | **`rust-embed`**，照 kiro 的 `src/webui/mod.rs`（内容 sha256 强 ETag + `no-cache` + 304），**但不做 SPA 回退**，未命中一律 404 纯文本 | agnes 面板无 URL 路由，回退会让「curl 核对某个资源上没上线」变成骗人（kiro 自己的注释记过这个坑） |
| **P-12** | `/admin/` 尾斜杠 | 照 kiro：`/admin`、`/admin/`、`/admin/{*file}` 三条路由**都直接发内容**，**不发 301** | 02 §8.2 记的 TS 行为是 301；而 301 会被浏览器近乎永久缓存（`agnes2api/src/ui/serve.ts` 自己的注释就写着这个风险）。**登记为偏离 02 §8.2** |
| **P-13** | 二进制资源 | **允许独立的 `logo.png`**（对标 kiro 的品牌标记）；**🔴 仍然禁止独立 `.svg`** | 「零二进制」那条规则的理由是 `build-ui.mjs` 的扩展名白名单，`rust-embed` 之后这个理由消失；而禁 `.svg` 的理由（`image/svg+xml` 挂在 `/admin/` 下 = 可执行脚本的**同源文档**）与生成器无关，仍然成立 |
| **P-14** | 安全头 | **全部保留 agnes 的六条**（CSP / X-Frame-Options / nosniff / Referrer-Policy / Cache-Control），kiro **一条都没有** | 这是 agnes 明确更强的一面，用户规则第一句直接命中：kiro 没有的就保留 |
| **P-15** | 实时日志的凭据 | **不照抄 kiro 的 `?api_key=`**；推荐「`POST` 换一次性短票 → 票走 URL」 | agnes 有一条成文的相反纪律并有判据钉着（05 §9.3 已登记）。**要主线裁**（§9 R-2） |

---

## 1. 两边面板的形状（先把事实摆清）

### 1.1 文件清单与规模

**kiro2api `admin-ui-v2/`** —— 24 个文件 / 12,501 行（不含 `tests/`）：

| 文件 | 行 | 是什么 |
|---|---:|---|
| `index.html` | 179 | 全站唯一 HTML。头 + 侧栏 + 8 个空 `<section>` + 登录浮层 + toast/modal 挂载点 + 12 条 `<script>` |
| `css/base.css` | 746 | token（亮/暗）、reset、布局骨架、**全部组件原语** |
| `css/sections.css` | 11 | **一条规则都没有**的聚合入口（只有一段说明谁在哪） |
| `css/sections-manage.css` | 1,470 | accounts / apikeys / settings / models 四个板块的专用样式 |
| `css/sections-stats.css` | 678 | dashboard / usage 两个板块 |
| `css/sections-tools.css` | 560 | modeltest / logs 两个板块 |
| `js/boot.js` | 41 | `<head>` 同步经典脚本：落 `data-theme` + `data-lang`，防闪白 |
| `js/i18n-dict.js` | 712 | `window.I18N`，**649 个 key** × 5 语言 |
| `js/i18n.js` | 94 | 三钩子（`data-i18n` / `-ph` / `-title`）+ `t()` + `setLang()` |
| `js/theme.js` | 40 | `data-theme` 读写 + `meta[theme-color]` + `themechange` 事件 |
| `js/api.js` | 179 | `/api/admin` 出口（`x-api-key` 头）+ SSE（`?api_key=`）+ `/v1` 转发出口（`Bearer`）+ toast + `ApiError` |
| `js/app.js` | 375 | 壳层：登录闸、板块路由、主题/语言/退出/重启/状态徽章接线、移动端侧栏、切语言整页重绘 |
| `js/sec-*.js` × 8 | 7,416 | 八个板块（accounts 2,412 最大） |
| `logo.png` | — | 品牌标记（独立二进制） |
| `tests/*.test.mjs` × 3 | 1,177 | 纯 Node `node --test`，**35 格**，全部用 `new Function` + 迷你 DOM 替身注入 IIFE |

**agnes2api `admin-ui/`** —— 41 个文件 / 17,776 行：

| 文件族 | 行 | 是什么 |
|---|---:|---|
| `index.html` | 120 | 顶栏 + 侧栏 + 9 个空 `<section>` + 登录闸（`.gate`）+ toast 挂载点 + **1 条** `<script type="module">` |
| `css/base.css` / `shell.css` / `sections.css` | 226 / 251 / 600 | token+原语 / 外壳+登录闸+toast+modal+断点 / 业务原语 |
| `js/boot.js` | 23 | 同 kiro（全站唯一经典脚本） |
| `js/i18n-dict.js` | 1,420 | `export const I18N`，**743 个 key** × 5 语言 |
| `js/i18n.js` / `theme.js` / `ui.js` | 66 / 34 / 299 | 同上 + **`ui.js` 是 kiro 没有的共用 DOM 原语层** |
| `js/api.js` / `gw-api.js` / `health.js` | 170 / 239 / 69 | 管理出口 / **Playground 专用网关出口（另一把钥匙）** / **不鉴权健康探针** |
| `js/app.js` | 292 | 壳层（同 kiro，少了重启，多了会话 12 小时上限） |
| `js/sec-*.js` × 9 | 7,681 | 九个板块（playground 1,721 最大） |
| **`js/pure/*.mjs` × 18** | **6,600** | **agnes 独有的可测纯函数层**（顶层只许 `export`，禁 `import`、禁 `document`/`window`） |
| `README.md` | 144 | 三条硬规则 + 板块↔文件对照表（有 `docs-parity` 判据双向钉着） |

外加 `agnes2api/tests/ui/` —— **46 个文件 / 33,600 行 / 1,169 格**（`^\s*(it|test)\(` 口径），
其中 `tests/ui/dom/` 那 17 份跑在自研 DOM 垫片 `tests/helpers/fake-dom.ts` 上。

### 1.2 加载链与运行时形态

| | kiro2api | agnes2api |
|---|---|---|
| 脚本形态 | **经典脚本 × 12**，共享 `window.K` 全局命名空间 | **ESM 单入口**（`app.js`）+ `boot.js` 一个经典脚本 |
| `<base>` | `<base href="/admin/">` + 全部相对路径 | 无 base，全部 `/admin/…` 绝对路径 |
| 站点图标 | 独立 `logo.png` | 内联 `data:image/png;base64`（32×32 favicon + 64×64 品牌标记，由 `scripts/check-png.mjs` 逐像素对账 `docs/logo.png`） |
| 板块契约 | `K.sections.<name> = { init?, onShow?, onHide? }` | **逐字相同**（`agnes2api/admin-ui/js/app.js` 的 `SECTIONS`） |
| 切语言 | `apply(document)` + 重跑当前板块 `onShow()` | **逐字相同**，且明令「板块内不许各自监听 `langchange`」（kiro 有 4 处冗余监听，agnes 文件头点名） |
| 板块记忆 | `localStorage['kiro2api_section']` | `localStorage['agnes2api_section']`（键名集中在 `pure/storage-keys.mjs`） |
| 401 处置 | 清 key + 弹浮层；**403 明确不当掉线** | 逐字相同（agnes 的注释直接写「照抄 kiro 用中文注释警告过的那个坑」） |
| 会话上限 | 无 | **12 小时绝对上限**，每请求复查（`pure/session.mjs`） |
| 安全头 | **一条都没有** | CSP + XFO + nosniff + Referrer-Policy + Cache-Control |

⚠️ **两边的壳层骨架已经是同一套**（板块契约、切语言重绘、403 不当掉线三条逐字同源）。
P2b 的工作量**不在壳层**，在 CSS 与九个板块的内容。

### 1.3 一条必须先说的结论

CSS 行数 **3,465 : 1,077**。
agnes 的 1,077 行里，`base.css` 只有 226 行、而且**没有一个板块专用样式文件**——
九个板块的全部视觉都挤在 `sections.css` 那 600 行的通用原语里。
kiro 的 1,470 行 `sections-manage.css` 光「账号行」一个组件就用掉 230 行
（行卡 + 开关 + 图标按钮 + 统计行 + 分页 + 分组下拉菜单）。

⇒ **「太丑了」不是调色板问题，是「组件根本没做」。**
P-1 的裁定（整套搬）由这条支撑：在 1,077 行上补配色，得到的仍然是一个没有 `.account-row`、
没有 `.key-card`、没有 `.acct-switch`、没有 `.mt-chat` 的面板。

---

## 2. 样式体系全解剖

> 本节的写法：**细到能照着从零重建**。逐条给选择器、给取值、给它管什么。
> 来源一律是 `kiro2api/admin-ui-v2/css/` 四份文件。

### 2.1 设计语言的三句话

1. **Emerald + 玻璃拟态**。主色是 emerald，顶栏是唯一一条真正的毛玻璃（`backdrop-filter: blur(10px)` + 半透明底）。
2. **双层阴影 + rem 圆角分级**。`--shadow-md/lg/xl` 都是「一条紧贴元素的硬边 + 一条铺开的软边」两条；
   圆角五档 `sm .375rem / md .5rem / lg .75rem / xl 1rem / full 9999px`。
3. **悬停即抬起**。卡片族（`.stat-card` / `.account-row` / `.key-card` / `.dash-*-card` / `.usage-*-panel`）
   一律 `transform: translateY(-2px | -4px)` + 提一档阴影 + 边框换成 `--primary-20`。
   这条是整套 UI 的「手感」来源，**别省**。

### 2.2 token 全表 —— 亮色 `:root`（`base.css`）

> 🔴🔴 **这张表记的是 kiro 的基线值**（§2 开头的取数口径：来源一律是
> `kiro2api/admin-ui-v2/css/`），**不是本仓 `admin-ui/css/base.css` 的当前值**。
> 两者只在 §2.4 逐条登记的那几格不同：亮色 `--success-text` `#059669` → `#047857`（P-3b）、
> `--danger-text` `#dc2626` → `#b91c1c`（P-3c）。
> ⚠️ **别把本仓的值改写进这张表**：那会一次抹掉两件事 —— kiro 原本长什么样，
> 以及我们究竟偏离了哪几格；而「偏离了哪几格」正是 §9 R-1 要主线裁的那个东西。
> （同理，表里没有 `--primary-strong` 与 `--warning-text` **不是漏**：kiro 没有这两个 token，
> 它们是 P-3 / P-3d 新增的，见 §2.4。）
> ⚠️ 另有一格是**补定义**而不是改值：`--warning-10` —— kiro 用了它
>（`.status-badge.is-restarting`）却**从没定义过**，靠 `var(--warning-10, rgba(245,158,11,.12))`
> 那个行内兜底活着；本仓把所有 `var()` 兜底都摘掉了（兜底正是「引了一个不存在的 token」
> 这类静默缺陷的藏身处），所以必须真的定义出来：`rgba(245,158,11,.1)`。

| 组 | token | 值 | 落点 |
|---|---|---|---|
| 主色 | `--primary` / `--primary-color` | `#059669` | 链接、选中态、实心按钮、焦点环 |
| | `--primary-hover` | `#047857` | 实心按钮 hover |
| | `--primary-light` | `#34d399` | （亮色下少用，深色的主色就是它） |
| | `--primary-soft` | `#d1fae5` | 淡底 |
| 辅助 | `--secondary` / `--success` | `#10b981` | 成功徽章、状态点 |
| | `--danger` | `#ef4444` / `--danger-soft` `#fee2e2` | 危险按钮、错误 |
| | `--warning` | `#f59e0b` | 花费、积分、警告 |
| | `--info` | `#3b82f6` | RPM、信息条 |
| 背景 | `--bg-primary` | `#ffffff` | **卡片底** |
| | `--bg-secondary` | `#f8fafc` | **页面底**（`body`） |
| | `--bg-tertiary` | `#f1f5f9` | 表头、图标按钮底、开关轨道 |
| | `--glass` / `--bg-glass` | `rgba(255,255,255,.8)` | 顶栏 |
| | `--glass-strong` | `rgba(255,255,255,.95)` | （浮层加强档） |
| 文本 | `--text-primary` | `#0f172a` | 正文 |
| | `--text-secondary` | `#475569` | 次要说明、表头 |
| | `--text-tertiary` | `#64748b` | 三级说明、时间戳、单位 |
| 边框 | `--border-color` | `#e2e8f0` | 一切边框与分隔线 |
| | `--border-hover` | `#cbd5e1` | 悬停加深一档 |
| 圆角 | `--radius-sm/md/lg/xl/full` | `.375/.5/.75/1rem` / `9999px` | sm=日志行；md=分段/chip/图标按钮；lg=按钮/输入/表格容器；xl=卡片/modal/侧栏；full=徽章/状态点/开关 |
| 阴影 | `--shadow-sm` | `0 1px 2px 0 rgba(0,0,0,.05)` | 卡片常驻 |
| | `--shadow-md` | `0 4px 6px -1px rgba(0,0,0,.05), 0 2px 4px -1px rgba(0,0,0,.03)` | hover 抬起 |
| | `--shadow-lg` | `0 10px 15px -3px rgba(0,0,0,.05), 0 4px 6px -2px rgba(0,0,0,.025)` | 浮层 / 卡片重 hover |
| | `--shadow-xl` | `0 20px 25px -5px rgba(0,0,0,.05), 0 10px 10px -5px rgba(0,0,0,.02)` | （modal 备用） |
| | `--shadow-glass` | `0 8px 32px 0 rgba(31,38,135,.07)` | 顶栏 |
| 透明档 | `--primary-10/20/30` | `rgba(5,150,105,.1/.2/.3)` | 选中底、焦点光晕、hover 边框 |
| | `--success-10` `--warning-15` `--danger-10` `--info-10` | 对应色 10%/15% | 徽章底、图标 chip 底 |
| 语义档 | `--info-bg` `.08` / `--info-bg-light` `.04` / `--info-border` `.2` / `--info-text` `#1d4ed8` | | 中性提示条 |
| | `--success-bg` `.1` / `--success-text` `#059669` | | |
| | `--danger-bg` `.1` / `--danger-text` `#dc2626` | | |
| 遮罩 | `--overlay-bg` | `rgba(0,0,0,.6)` | modal / 侧栏遮罩 |
| 代码 | `--code-bg` / `--code-text` | `#1e1e1e` / `#d4d4d4` | **日志控制台（亮色下也是深色面）** |
| 动画 | `--transition` | `all .2s cubic-bezier(.4,0,.2,1)` | 一处定义、各处引用 |
| 布局 | `--sidebar-w` / `--header-h` | `240px` / `64px` | |

### 2.3 token 全表 —— 深色 `[data-theme="dark"]`

> 🔴 同 §2.2：**这里记的也是 kiro 的深色基线**。本仓与它不同的是两格 ——
> `--text-secondary` `#94a3b8` → `#aab8c9`（P-4）、`--text-tertiary` `#64748b` → `#94a3b8`（P-3e 簇 ①），
> 外加 kiro 没有的 `--primary-strong` `#34d399`（P-3）与 `--warning-text` `#fbbf24`（P-3d）。逐条见 §2.4。

只列**与亮色不同**的：

`--primary` `#34d399`、`--primary-hover` `#10b981`、`--primary-light` `#6ee7b7`、`--primary-soft` `rgba(52,211,153,.15)`；
`--success` `#34d399`、`--danger` `#f87171`、`--warning` `#fbbf24`、`--info` `#60a5fa`；
🔴 **明暗层级刻意反过来**：`--bg-primary` `#1e293b`（卡片，**浅**）、`--bg-secondary` `#0f172a`（页面，**深**）、`--bg-tertiary` `#334155`；
`--glass` `rgba(30,41,59,.85)`、`--glass-strong` `rgba(15,23,42,.95)`；
`--text-primary` `#f1f5f9`、`--text-secondary` `#94a3b8`、`--text-tertiary` `#64748b`；
`--border-color` `#334155`、`--border-hover` `#475569`；
四档阴影黑度提到 `.3/.4/.5/.6`（亮色那套 5% 黑压在 `#0f172a` 上等于没有）；
`--overlay-bg` `rgba(0,0,0,.8)`；`--code-bg` `#0d1117`、`--code-text` `#e6edf3`；
`--info-text` `#93c5fd`、`--success-text` `#6ee7b7`、`--danger-text` `#fca5a5`。

末尾一条 a11y 补丁（**必抄**）：
```css
[data-theme="dark"] .btn-primary,
[data-theme="dark"] .btn-success { color: #0b1f17; }
```
理由 kiro 自己写着：深色主色 `#34d399` 偏亮，白字对比度不足，改深色字达 AA。

**亮暗怎么切**（三处，缺一处就闪）：
1. `js/boot.js` 在 `<head>` 里**同步**（无 `defer`、非 module）读 `localStorage` 落 `data-theme` —— 模块脚本天然 defer，会在 body 绘制后才跑，每次刷新闪一次白；
2. `js/theme.js` 运行期读写 + 改 `meta[name=theme-color]` + 广播 `themechange`；
3. `light` 时**移除属性**而不是设成 `"light"` —— `:root` 就是亮色全量 token，多一个值只会多一条永远匹配不上的规则。

### 2.4 token 冲突的裁定（🔴 本节是本文最硬的一段）

用户要「完全对标」。但 kiro 那套配色里有**两处实算不过 WCAG 1.4.3 AA（正文 4.5:1）**，
而 agnes 为这两处各有一格会红的判据（`agnes2api/tests/unit/source-guards.test.ts`）。

⚠️ **这一节后来长出了 2.4.1 / 2.4.2 两段**：P-3 / P-4 是**开工前**按 kiro 的 token 表现算出来的两处；
施工之后又查出**同一个病换了名字**（语义三色的 `*-text` 那一档，P-3b/c/d）与
**「只做装饰」那半句根本没人验过**（约 110 处正文字色，P-3e）。
🔴 **三段是同一次偏离的延长线，一并登记在 §9 R-1 下**，别当成三笔独立的事。

**我自己按 WCAG 相对亮度公式现算了四个读数**（不是抄 agnes 的注释；算完与它逐个吻合）：

| 前景 | 背景 | 比值 | 用在哪 | 过 AA？ |
|---|---|---|---|---|
| `#ffffff` | `#059669`（kiro 亮色主色） | **3.77 : 1** | `.btn-primary` 的标签（0.875rem / 600，**不是大文本**） | ❌ |
| `#ffffff` | `#047857`（agnes 亮色主色） | **5.48 : 1** | 同上 | ✅ |
| `#94a3b8` | `#334155`（kiro 深色 `--text-secondary` on `--bg-tertiary`） | **4.04 : 1** | 🔴 `.data-table th` 就是这一对；`.info-label`、`.stat-info p` 同族 | ❌ |
| `#aab8c9` | `#334155`（agnes 深色 `--muted`） | **5.13 : 1** | 同上 | ✅ |

**裁定 P-3 / P-4**：
- `--primary` 保持 `#059669`（一切**装饰性**用法：图标 chip 底、淡底、边框、状态点、焦点环 —— 这些受 1.4.11 的 3:1 约束，3.77 过线）；
- **新增 `--primary-strong: #047857`**（深色下 `--primary-strong: #34d399`，白字那一半在深色下由上面那条 `color:#0b1f17` 补丁解决），
  **只用在两处**：`.btn-primary`/`.btn-success`/`.sidebar-toggle` 的**底色**，以及 `a` / `.nav-item.active` / `.login-tab.active` 的**字色**；
- 深色 `--text-secondary` 从 `#94a3b8` 改 `#aab8c9`。

**代价如实写**：这是对「完全对标」的**两处刻意偏离**。并排看几乎分不出（同一条 emerald 暗一档、同一条 slate 亮半档），
但**确实不是逐字节相同**。⇒ 登记为 §9 R-1，主线可以推翻成「逐字照抄、接受不达标」。
⚠️ **别用「把按钮字号调大到 18px 就算大文本」绕过去**——那会连坐整套按钮的尺寸标度。

#### 2.4.1 P-3b / P-3c / P-3d —— 语义三色的「正文字色」那一档（token 层）

P-3 当初**只按名字覆盖了 `--primary` 一个 token**，而 `--success-text` 与 `--primary`
**逐字节同值**（都是 `#059669`）—— 同一个病换个 token 名就绕过去了，且绕过去的那几个
恰恰是横幅、徽章、错误提示这些**最需要被读懂**的东西的字色。

| 语义 | kiro（§2.2 表里的值） | 本仓 | 白底 | 它自己那层 tint 面上 | 这一对画在哪 |
|---|---|---|---|---|---|
| success | `#059669` | **`#047857`**（emerald-700） | 3.77 → **5.48** | 3.43 → **4.99**（`--success-bg` 合成出的 `#e7f8f2`） | `.reg-test-result.is-ok` 一族 |
| danger | `#dc2626` | **`#b91c1c`**（red-700） | 4.83 → **6.47** | 4.24 → **5.68**（`.banner-danger` 底下的 `#fdecec`） | 降级横幅、错误正文 |
| warning | **kiro 没有这个 token** | **新增 `#92400e`**（amber-800；深色 `#fbbf24`） | 借 `--warning` 时 2.15 → **7.09** | 1.92 → **6.32**（`--warning-15` 合成出的 `#fef0da`） | 🔴 `.usage-unknown`、`.status-badge.is-degraded`、`.badge-warning` |

⚠️ **warning 那一档降的是两档（amber-500 → amber-800），不是一档**：amber 比 emerald 亮得多，
只降一档的 `#d97706` 白底仍然只有 **3.19**；P-3d 当初落的 amber-700 `#b45309` 白底 5.02 够，
但压在 `--warning-15` 的面上只有 **4.48** —— 差 AA 最后那一点，而那张面正是
`.status-badge.is-degraded` / `.badge-warning` 天天在用的。**色系不变，只是第三档。**
🔴 深色那一半**沿用 `--warning` 自己的值 `#fbbf24`**（与深色 `--primary-strong` 同法）：
卡片面 **8.76**、`--bg-tertiary` 面 **6.20**，本来就够，再压暗只会离 amber 越来越远。
🔴 装饰用的 `--success` / `--danger` / `--warning` **原值一律不动**（只受 1.4.11 的 3:1 约束）。

#### 2.4.2 P-3e —— 「只做装饰」那半句从来没人验过（规则层，约 110 处）

P-3 / P-3b/c/d 补的全是 **token 本身**；而判据只钉了「`--primary-strong` 配白字 ≥ 4.5」
与「`--primary` 做装饰 ≥ 3.0」，🔴 **「`--primary` 只做装饰」这半句没有任何东西在验，
而它不成立**：`.mt-meta-val` / `.bb-count` / `.us-n.us-credits` / `.model-tag` /
`.current-pill` / `.dash-rotation-badge` 全都拿它当**正文字色** —— 判据从头到尾是绿的，
裁定在约 110 处被违反。同一条病在另外三个语义上更重。四个簇，逐簇改前 → 改后（现算）：

| 簇 | 处数 | 改前 | 改后 |
|---|---|---|---|
| ① 深色 `--text-tertiary` 当正文（`.form-hint` / `.modal-desc` / `.mng-inline-note` / `.cell-sub` 与全部表格 meta） | ~50 | `#64748b` 是**给白底挑的值原样抄进深色块**：卡片面 **3.07**、页面底 3.75 | `#94a3b8`（slate-400，上一档）：**5.71** / **6.96**。⚠️ 它仍比 `--text-secondary: #aab8c9` 暗一档 ⇒ 三级文字层次原样保住 |
| ② 亮色 `--warning` 当正文（每个费用/额度数字、`.status-badge.is-degraded`、`.mt-hint-warn`） | ~18 | 白底 **2.15**、`--warning-15` 面 **1.92** | 改指 `--warning-text`：**7.09** / **6.32** |
| ③ 亮色 `--success` 当正文（含顶栏那颗「运行中」`.status-badge`） | ~16 | 白底 **2.54**、`--success-10` 面 **2.31** | `--success-text`：**5.48** / **4.99** |
| ④ 亮色 `--danger` / `--info` / `--primary` 当正文 | ~26 | 3.76 / 3.30、3.68 / 3.28、3.77 / 3.34（白底 / 自己那层 10% tint 面） | `--danger-text` **6.47** / **5.68**；`--info-text` **6.70** / **5.98**；`--primary-strong` **5.48** / **4.86** |

🔴 **两处显式豁免 —— 登记在案，不是漏的**：
- `.badge-off`（停用态文字）：WCAG **1.4.3 自己就豁免禁用控件**，而且它**已经有删除线这条
  非颜色线索**（U-16 那条纪律在这一格是满足的）。实算亮色 4.34、深色 4.04，都够不到 4.5。
- `.stat-icon.*`（五格 `aria-hidden` 的图标 chip）与 `.status-dot`（8px 圆点，走 `background`
  不走 `color`）：**非文字元素**，不受 1.4.3 的 4.5:1 约束；一起抬到 4.5 会让图标 chip 比
  它旁边的标题还重。
  ⚠️ **其中几格连 1.4.11 的 3:1 都不到**（亮色 `.stat-icon.warning` 1.92、`.stat-icon.success` 2.31；
  `.status-dot.warning` 2.15、`.status-dot.healthy` 2.54）。**它们靠的是「纯装饰 + `aria-hidden`
  + 语义另有承担」这条排除，不是达标** —— 这一笔如实记着，不当成已经解决。

🔴 **色系不许走样**：这一轮每一处改动都只在**同一条色阶**上下挪档，没换过一个色系。

### 2.5 reset / 字体 / 滚动条 / 关键帧

```css
* { margin:0; padding:0; box-sizing:border-box; }
html, body { height:100%; }
body {
  font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto,
               'Helvetica Neue', Arial, 'PingFang SC', 'Microsoft YaHei', sans-serif;
  background: var(--bg-secondary); color: var(--text-primary); line-height: 1.6;
  transition: background-color .3s ease, color .3s ease;
}
a { color: inherit; text-decoration: none; }   /* ⚠️ kiro 的链接不带主色，agnes 的 a 带 */
button { font-family: inherit; }
::-webkit-scrollbar { width:8px; height:8px; }
::-webkit-scrollbar-thumb { background: var(--border-color); border-radius:4px; }
::-webkit-scrollbar-thumb:hover { background: var(--text-tertiary); }
:focus-visible { outline: 2px solid var(--primary); outline-offset: 2px; }   /* WCAG 2.4.7 */
```
**零 CDN、零 web font**，只用系统字体栈。

关键帧共 5 条：`fadeIn`（`opacity 0→1` + `translateY(6px→0)`，0.15–0.3s，用于板块切换/浮层/语言菜单）、
`spin`、`modalIn`（`translateY(-10px→0)`）、`kb-spin`（重启按钮图标）、`kb-pulse`（状态徽章的呼吸圈）、
`logsPulse`（日志连接点）、`mtFadeIn`（聊天气泡）、`mtBlink`（打字光标）。

**字号标度**（整套只用这几档，别另发明）：
`1.75rem` 大数字 → `1.6rem` 仪表盘数字 → `1.5rem` 板块标题/账号统计数字 → `1.25rem` modal 标题 →
`1.125rem` 卡片标题 → `1rem` 面板小标题/表单值 → `0.95rem` 账号名/气泡 → `0.9rem` 导航/表单控件 →
`0.875rem` 按钮/表格/表单标签 → `0.85rem`→`0.8rem`→`0.78rem`→`0.75rem`→`0.72rem`→`0.7rem` 各级说明。

**间距标度**：`0.15 / 0.25 / 0.35 / 0.5 / 0.75 / 1 / 1.25 / 1.5 / 2rem`。
卡片内边距恒 `1.5rem`（紧凑卡 `0.85–1.1rem`），栅格 gap 恒 `1–1.5rem`。

### 2.6 布局骨架

```
.container            flex column, min-height:100vh
├── .app-header       position:sticky top:0 z:50  height:64px
│                     display:flex align-items:center gap:1rem padding:0 1.5rem
│                     background:var(--glass) backdrop-filter:blur(10px)
│                     border-bottom:1px solid var(--border-color)
│   ├── .brand        （logo 30×30 + 名字 1.15rem/700，letter-spacing -.01em）
│   ├── .header-spacer  flex:1
│   └── .header-actions display:flex gap:.5rem
└── .main-content     flex, flex:1, max-width:1600px, margin:0 auto,
                      padding:1.5rem, gap:1.5rem, position:relative
    ├── aside.sidebar width:240px, flex-shrink:0, align-self:flex-start,
    │                 position:sticky top:calc(64px + 1.5rem),
    │                 background:var(--bg-primary), border:1px solid,
    │                 border-radius:var(--radius-xl), padding:.75rem,
    │                 box-shadow:var(--shadow-sm)
    └── main.content  flex:1, min-width:0        ← 🔴 min-width:0 是宽表不顶穿整页的前提
        └── section.section { display:none }  .section.active { display:block; animation:fadeIn .3s }
```
外加三个挂载在 `.container` 之外的元素：`#sidebarBackdrop`、`#sidebarToggle`（移动端汉堡）、
`#loginOverlay`、`#toastContainer`、`#modalRoot`。

**板块标题两种写法**：
`.section-title`（1.5rem/700，margin-bottom 1.5rem）；
`.section-header`（flex + space-between + wrap，右侧 `.section-actions` 放按钮组）。

### 2.7 顶栏与导航（逐元素）

**顶栏从左到右六样**（kiro `index.html:22-82`）：
1. `.brand` —— `<img class="brand-img">` 30×30 + 文字；
2. `.header-spacer`；
3. `#serverStatus.status-badge` —— 40px 高胶囊，`--success-10` 底 + `--success` 字，
   `.status-dot` 8px 圆点带 `kb-pulse` 呼吸；三态 `is-offline`（danger，停呼吸）/ `is-restarting`（warning）；
4. `#githubLink.icon-btn`（`<a>`，不是 button —— 中键开新标签只有真链接才有）；
5. `#restartBtn.icon-btn`（图标 + 文字，`is-restarting` 时图标 `kb-spin`）；
6. `.lang-wrap` —— `#langBtn.icon-btn` + `.lang-menu`（绝对定位，`top:calc(100%+6px)`，min-width 160px，
   `--shadow-lg`，`.open` 时 `fadeIn .15s`；项 hover `--bg-tertiary`，active `--primary-10` + 主色 + 600）；
7. `#themeToggleBtn.icon-btn.theme-toggle`（40×40 正方，内含 `.icon-sun` / `.icon-moon` 两枚，
   **切换只改 `style.display`，不重建节点**）；
8. `#logoutBtn.icon-btn`（图标 + 文字）。

`.icon-btn` 的规格（**整套顶栏的手感都在这一条**）：
```css
height:40px; min-width:40px; padding:0 .75rem; gap:.4rem;
background:var(--bg-tertiary); color:var(--text-secondary);
border:1px solid var(--border-color); border-radius:var(--radius-full);
font-size:.85rem; font-weight:600; transition:var(--transition);
:hover { background:var(--bg-secondary); border-color:var(--primary);
         color:var(--primary); transform:translateY(-2px); box-shadow:var(--shadow-md); }
svg { width:18px; height:18px; }
```

**侧栏导航项**：
```css
.nav-item { display:flex; gap:.75rem; padding:.7rem .9rem;
            border-radius:var(--radius-lg); color:var(--text-secondary);
            font-size:.9rem; font-weight:600; cursor:pointer;
            transition:var(--transition); user-select:none; }
.nav-item .nav-icon { font-size:1.05rem; width:22px; text-align:center; flex-shrink:0; }
.nav-item:hover  { background:var(--bg-tertiary); color:var(--text-primary); }
.nav-item.active { background:var(--primary-10); color:var(--primary); }
```
图标是**emoji**（📊 👤 🤖 🔑 🧪 📈 📜 ⚙️），不是 SVG。文字必须自己占一个 `<span data-i18n>`
（`apply()` 对 `[data-i18n]` 写的是 `textContent`，标在容器上会把 emoji 一起抹掉 —— 两边的注释都记了这条）。

⚠️ **`.nav-item.active` 只有颜色差**，agnes 为同一个组件补过 `font-weight:600`（WCAG 1.4.1：状态不许只由颜色表达）。
**建议照 agnes 补上这一条**（属于「agnes 更强 ⇒ 保留」，且视觉代价只有半档字重），登记在 §4 表里。

### 2.8 组件原语逐个

#### 2.8.1 统计卡

```css
.stats-grid { display:grid; grid-template-columns:repeat(auto-fit, minmax(250px,1fr));
              gap:1.5rem; margin-bottom:2rem; }
.stat-card  { background:var(--bg-primary); border:1px solid var(--border-color);
              border-radius:var(--radius-xl); padding:1.5rem;
              display:flex; align-items:center; gap:1rem; box-shadow:var(--shadow-sm); }
.stat-card:hover { box-shadow:var(--shadow-md); transform:translateY(-2px); }
.stat-icon  { 48×48; border-radius:var(--radius-lg); font-size:1.5rem; }
.stat-icon.primary/.success/.warning/.danger/.info  → 对应 --*-10/15 底 + 对应字色
.stat-info h3 { font-size:1.75rem; font-weight:700; line-height:1.2; }
.stat-info p  { font-size:.875rem; color:var(--text-secondary); }
```
仪表盘另有一层覆写：`#sec-dashboard .stats-grid { grid-template-columns:repeat(5,1fr); gap:1rem }`
（五张卡强制一行），卡 padding 收到 `1.1rem`，`.dash-stat-icon` 48×48/`1.35rem`，
`.dash-stat-value` `1.6rem/700`，`.dash-stat-label` `.8rem`；**≤1200px 回落 auto-fit(200px)**。
用量页同理强制六列，`.stat-icon` 收到 40×40、数字收到 `1.35rem`，≤1200px 回落 auto-fit(190px)。

#### 2.8.2 卡片

```css
.card { background:var(--bg-primary); border:1px solid var(--border-color);
        border-radius:var(--radius-xl); padding:1.5rem;
        box-shadow:var(--shadow-sm); margin-bottom:1.5rem; }
.card-header { flex + space-between; margin-bottom:1rem; padding-bottom:1rem;
               border-bottom:1px solid var(--border-color); }
.card-header h3 { font-size:1.125rem; font-weight:600; }
.info-grid { grid auto-fit minmax(200px,1fr); gap:1rem; }
.info-item { flex column gap:.5rem }  .info-label{.875rem,secondary}  .info-value{1rem/600}
```

#### 2.8.3 按钮族

```
.btn      inline-flex, gap:.5rem, padding:.625rem 1.25rem,
          border:1px solid transparent, radius:lg, .875rem/600
          :disabled{opacity:.55}  :active{scale(.98)}  svg{16px}
.btn-primary   实心主色 + 白字；hover → --primary-hover + 0 4px 6px -1px var(--primary-20)
.btn-danger    白底 + danger 字 + danger 边；hover → 实心 danger + 白字
.btn-success   实心 success + 白字；hover opacity .9
.btn-outline   透明底 + secondary 字 + border-color 边；hover → bg-tertiary + 主色边/字
.btn-secondary bg-tertiary 底 + primary 字；hover → bg-secondary + border-hover 边
.btn-sm  .375rem .75rem / .8125rem      .btn-lg  .875rem 1.5rem / 1rem
.btn-block width:100%                   .btn-xs  .1rem .5rem / .7rem（卡内联刷新）
```

#### 2.8.4 表单族

```
.form-group   margin-bottom:1.25rem
.form-label   block, margin-bottom:.5rem, .875rem/600
.form-control width:100%, padding:.75rem 1rem, border:1px solid border-color,
              radius:lg, .9rem, background:var(--bg-secondary)
  :focus      border:primary + background:bg-primary + box-shadow:0 0 0 4px var(--primary-10)
.form-hint    margin-top:.375rem, .8125rem, tertiary（sections-manage 里另有一版 flex + .hint-ico 12px）
select.form-control   appearance:none; option 显式给底色/字色（深色下不给会瞎）
textarea.form-control min-height:120px; resize:vertical
.form-row     flex + wrap, gap:1rem；子 .form-group{flex:1;min-width:160px}
.form-check   flex gap:.5rem；input 16×16 + accent-color:var(--primary)
.form-error   （sections-manage）表单级错误行
.req-star     必填星号，danger + 700
```

#### 2.8.5 徽章与 tag

```
.badge          inline-flex, gap:.375rem, padding:.25rem .75rem, radius:full, .8125rem/600
.badge-success/-danger/-warning/-info   → --*-10/15 底 + 对应字色
.badge-secondary / .badge-muted         → bg-tertiary 底 + secondary 字（**两个名字都要留**）
.tag            padding:.15rem .55rem, radius:sm, bg-tertiary, .75rem/600
.current-pill   primary-10 底 + 主色字 + .7rem/700（「当前使用中」）
.key-bound-chip color-mix 紫 #7c3aed 12% 底 + 25% 边 + 紫字（绑定账号）
.status-dot     8px 圆点，四档 healthy/warning/unhealthy/muted
```

#### 2.8.6 表格与分页

```
.table-container { overflow-x:auto; border-radius:lg; border:1px solid; background:bg-primary }
.data-table      { width:100%; border-collapse:collapse }
.data-table th   { position:sticky top:0; background:bg-tertiary; padding:.875rem 1rem;
                   text-align:left; .875rem/600; color:text-secondary;
                   border-bottom:1px solid; white-space:nowrap }
.data-table td   { padding:.875rem 1rem; border-bottom:1px solid; .875rem }
.data-table tr:last-child td { border-bottom:none }
.data-table tbody tr:hover   { background:var(--bg-secondary) }
.table-actions   { flex gap:.35rem wrap }
.pagination      { flex, justify-end, gap:.5rem, margin-top:1rem }  .page-info{.85rem,secondary}
```
数值列一律 `text-align:right` + `font-variant-numeric: tabular-nums`（`.num` / `.tabular` / `.ta-right`）。

#### 2.8.7 toast

```
.toast-container { position:fixed; top:1rem; right:1rem; z:1001; flex column gap:.5rem;
                   pointer-events:none }
.toast { bg-primary + 1px 边 + radius:lg + padding:1rem 1.5rem + shadow-lg;
         min-width:300px; max-width:380px;
         opacity:0; transform:translateX(110%); transition:all .3s ease;
         pointer-events:auto; flex + gap:.6rem + .875rem }
.toast.show { opacity:1; transform:translateX(0) }
.toast.success/.error/.info/.warning  → border-left:4px solid 对应色（**两种类名拼写都支持**）
```
⚠️ kiro 的 toast **恒 3.2 秒自动消失，没有 sticky 档**。
agnes 有 `sticky` + 关闭叉 + **自动聚焦到关闭按钮 + Escape** —— 那是给「批量操作里一部分被拒绝」
这类藏在 HTTP 200 响应体里的信息用的。**保留 agnes 的 sticky**（§4 表里 U-7）。

#### 2.8.8 modal

```
.modal-overlay { fixed inset:0; background:var(--overlay-bg); z:999; display:none;
                 flex center; padding:1rem; opacity:0; transition:opacity .25s }
.modal-overlay.active { display:flex; opacity:1 }
.modal { bg-primary + 1px 边 + radius:xl + shadow-lg;
         width:100%; max-width:560px; max-height:90vh; flex column;
         animation:modalIn .25s ease }
.modal.modal-lg { max-width:820px }
.modal-header { flex space-between; padding:1.5rem; border-bottom:1px solid }
               （sections-manage 覆写 align-items:flex-start + .modal-head-text/.modal-desc 副标题）
.modal-header h3 { 1.25rem/700 }
.modal-close { 36×36 圆; bg-tertiary; hover → danger 底 + 白字 }
.modal-body  { padding:1.5rem; overflow-y:auto }
.modal-footer{ flex justify-end gap:.75rem; padding:1.5rem; border-top:1px solid }
```
⚠️ kiro 的 modal **没有焦点管理**（无初始焦点、无焦点陷阱、无 Escape、`<h3>` 无 `aria-labelledby`）。
agnes 的 `ui.js:openModal` 四条都有。**保留 agnes 的实现，只换皮**（§4 表里 U-6）。

#### 2.8.9 登录闸

```
.login-overlay { fixed inset:0; background:var(--bg-secondary); z:2000; display:none; flex center }
.login-overlay.active { display:flex }
.login-card { max-width:400px; bg-primary + 1px 边 + radius:xl + shadow-lg;
              padding:2rem; animation:fadeIn .3s }
.login-brand { flex center gap:.6rem; 1.35rem/700; img 40×40 }
.login-title { center, 1.05rem/600, secondary, margin-bottom:1.5rem }
.login-error { display:none; .85rem; danger }   .login-error.show { display:block }
.login-hint  { margin-top:1rem; center; .8rem; tertiary }
```
⚠️ kiro 的登录卡上**没有主题切换按钮**；agnes 的 `.gate-head` 上有一颗
（理由：深色用户看到的登录闸本来就是深色的，而顶栏那颗要登录之后才够得着）。
**保留 agnes 那颗**，样式套 `.icon-btn`，放在 `.login-brand` 同行右端（§4 表里 U-5）。

#### 2.8.10 工具类

`.empty-state`（center，padding 3rem 1rem，tertiary，.95rem）、`.loading-spinner`（20px 环，`spin .8s`）、
`.text-muted/.text-success/.text-danger`、`.text-mono`（`'SFMono-Regular', ui-monospace, 'Menlo', 'Consolas', monospace`）、
`.hidden`（`display:none !important`）、`.flex/.items-center/.justify-between/.gap-sm`。

#### 2.8.11 二级原语（住在 `sections-manage.css`，但被多个板块用）

| 类 | 规格 | 谁在用 |
|---|---|---|
| `.mng-toolbar` + `.mng-spacer` + `.mng-search`(180–280px) | flex + wrap + gap .5rem | 全部管理板块的工具条 |
| `.acct-switch` / `.acct-switch-thumb` | **44×24 轨道 + 20px 白拇指 + 平移 20px**，`.is-on` 换主色底，`transition .25s cubic-bezier(.4,0,.2,1)`，`:focus-visible` 双层环 | 启停开关（本文推荐它同时服务 Key 池的「停用/启用」） |
| `.acct-icon-btn` / `.is-mini`(22×22) | 方形图标按钮，`.is-danger` 变体 | 行内动作簇 |
| `.seg-group` / `.seg-btn` / `.is-active` | `padding:.35rem .7rem`，radius:md，active = 实心主色 + 白字，`.seg-ico` 14px | 模式切换（限制方式、集成示例的协议×语言、Playground 的协议/模态） |
| `.chip-row` / `.chip-btn` / `.is-active` | `padding:.3rem .7rem`，同上 | 快捷档（时长、额度、到期档） |
| `.login-tabs` / `.login-tab` / `.login-pane` | 底边一条线，选中项用 `border-bottom:2px solid var(--primary)` **顶开**那条线 + 主色字 | 🔴 **注册机的 TAB 直接用它，不用新造**（见 §2.12） |
| `.meter` / `.meter > span` / `.hot` / `.warm` | 10px 高胶囊进度条，三色档 | 额度/配额进度（推荐给视频轮询进度复用） |
| `.cred-ms-*`（12 个类） | 多选下拉：触发器 + 标签簇 + 搜索框 + 选项列表 + 勾选标 | 绑定账号多选（推荐给「协议多选」复用） |
| `.kv-mono` / `.cell-sub` / `.mng-section-title` / `.mng-inline-note` | 表内小字 | 全局 |
| `.model-opus/-sonnet/-haiku/-muted` | `#9333ea` / `#2563eb` / `#16a34a` / tertiary | 模型名色标 |
| `.int-card` / `.int-codewrap` / `.int-copy` / `.int-code` | 集成示例代码块 + 右上角浮动复制按钮 | 设置页集成示例 |
| `.readonly-value` / `.rv-text` / `.icon-mini` | 只读值 + 显示/复制迷你按钮 | 设置页主 Key |
| `.settings-grid` | `repeat(2, minmax(0,1fr))`，≤768px 单列 | 设置页两列 |

### 2.9 响应式断点

kiro 全仓共用五档，**agnes 已经按同一组数对齐过**（`agnes2api/admin-ui/css/shell.css` 末尾那段
写着「三个数是对齐来的：kiro2api 的面板用的就是 1024 / 900 / 640」）：

| 断点 | 做什么 | 在哪份 CSS |
|---|---|---|
| **≤1200px** | 仪表盘五列 → `auto-fit(200px)`；用量六列 → `auto-fit(190px)` | sections-stats |
| **≤1024px** | 系统信息 4 列 → 2 列；`.mt-layout` 双栏 → 单栏；`.logs-detail` 360→320px | stats / tools |
| **≤900px** | 🔴 **侧栏变 off-canvas 抽屉**：`position:fixed`、260px、`translateX(-100%)`、`.open` 推出；`.sidebar-toggle` 显形（48px 圆钮，fixed 左下，主色底）；`.sidebar-backdrop.show` 显形 | base |
| **≤768px** | 设置页两列 → 单列；仪表盘 uptime 行竖排；`.logs-layout` 竖排、console 560→440px；`.mt-chat` 620→440px；`.mt-bubble` 78%→85% | manage / stats / tools |
| **≤640px** | 顶栏 padding 1.5→1rem；品牌 1.15→1rem；隐藏语言标签、状态文字、重启文字（只留图标/圆点） | base / stats |

⚠️ **agnes 的 ≤900 档做的是另一种结构**（侧栏横过来铺成一条可横向滚的导航带，
理由写在 `shell.css` 里：九个导航项这个量级下抽屉不值）。
**裁定：改用 kiro 的 off-canvas 抽屉**（P-1 的直接后果 —— 抽屉的全部 CSS 在 `base.css` 里现成，
`app.js` 的 `wireNav()`/`closeSidebar()` 也现成）。代价：多一条 `nav.menu` 文案（kiro 字典里已有）。

### 2.10 逐份 CSS 定义了什么

**`base.css`（746 行）** —— 全局唯一的 token 与原语所在地：
`:root` 亮色 token（60 条）→ `[data-theme=dark]` 暗色覆写（38 条）→ reset →
3 条 `@keyframes` → 布局（`.container` `.app-header` `.brand` `.header-actions` `.icon-btn`
`.status-badge` `.lang-wrap/.lang-menu` `.main-content` `.sidebar` `.nav-item` `.content`
`.section` `.section-title` `.section-header` `.sidebar-toggle` `.sidebar-backdrop`）→
`.stats-grid/.stat-card/.stat-icon/.stat-info` → `.card/.card-header/.info-*` →
按钮族 8 变体 → 表单族 10 类 → 徽章族 6 类 + `.tag` → 表格族 5 类 + `.pagination` →
toast 3 类 + 4 变体 → modal 7 类 → 登录闸 6 类 → 工具类 9 条 → 两档 `@media` → a11y 两条。

**`sections.css`（11 行）** —— 🔴 **一条规则都没有**，只是一段说明「实际规则在下面三份里」。
（本文沿用这个形状：加载序 `base → sections → sections-manage → sections-stats → sections-tools`。）

**`sections-manage.css`（1,470 行）** —— accounts / apikeys / settings / models：
共用工具条与小件（`.mng-*` `.kv-mono` `.cell-sub` `.status-dot`）→
账号汇总卡（`.accounts-stat-grid` 固定 4 列 + `.acct-stat-*` 7 类 + `.acct-stat-meter`）→
批量条（`.accounts-batchbar` `.account-row-select` `.is-selected`）→
**账号行卡**（`.accounts-list` `.account-row`（-4px 抬起）`.account-row-info/-idline/-id/-name/-meta/-stats`
`.account-stat` 五色档 + `.is-link` `.current-pill`）→
**开关**（`.acct-switch` 44×24）→ **图标动作按钮**（`.acct-icon-btn` + `.is-danger` + `.is-mini`）→
`.account-info-row`（余额弹窗）→ 分页（`.accounts-pager` `.accounts-page-indicator`）→
添加账号分组下拉（`.add-menu-wrap/.add-menu/.add-menu-label`）→
**交互式登录弹窗**（`.login-tabs/.login-tab/.login-pane` `.device-code-box/.device-code` `.waiting-line`
`.refresh-errors/.rex-*`）→ `.meter` 三档 →
**API 密钥全族**（`.svc-card/.svc-row` 服务连接卡；`.key-stat-grid/.key-stat` 5 档；
`.key-sort` `.key-search-wrap`；`.key-list/.key-group/.key-group-head`（绑定组用紫 `#7c3aed`）；
**`.key-card`**（-4px 抬起、`.is-bound` 紫色系 `color-mix`、`.is-dim`）+ `.key-line1/2/3`
+ `.key-serial/.key-name/.key-bound-chip` + `.key-usg-*` 四色；`.key-model-grid`；`.purge-list`）→
**表单弹窗全族**（`.modal-head-text/.modal-desc` `.key-form` `.form-error` `.seg-group/.seg-btn`
`.chip-row/.chip-btn` `.dur-custom/.dur-num/.quota-prefix` `.form-hint`
`.cred-ms-*` 多选下拉 12 类 `.created-key-box` `.bound-list` `.multiselect`）→
用量条与明细（`.usage-strip` + 三色数值档；`.usage-bymodel-grid/-card`；`.model-opus/-sonnet/-haiku`；
`.usage-log-head/-count`；`.usage-records-table` 十余个单元格类；`.usage-pager`；
`.key-log-head`；`.rec-*` 八个类）→
**设置页**（`.settings-grid` 两列 + `.settings-card-desc` `.readonly-value/.rv-text`
`.model-chip-grid/.model-chip`）→ **模型页**（`.models-table-scroll/.models-table` + 单元格类 + `.model-tag`）→
`@media ≤768` → `.icon-mini` → `.req-star` `.acct-inline-input` `.upload-row` →
**集成示例**（`.int-card/.int-codewrap/.int-copy/.int-code`）→ `#accountsFreshness.is-stale`。

**`sections-stats.css`（678 行）** —— dashboard / usage：
`.stats-head/.stats-toolbar/.stats-placeholder/.stats-spinner` →
仪表盘五列强制 + `.dash-stat-icon`（4 个 tint）`.dash-stat-value/-label` `.btn-xs` `.dash-gc-btn/-meta` →
系统信息面板（`.dash-sysinfo` shadow-md 常驻 / hover lg；`.dash-sysinfo-ver` `.dash-rotation-badge`
`.dash-info-grid` 固定 4 列 × 2 行；`.dash-info-item/-label/-icon/-value`）→
uptime + 二维码行（`.dash-uptime-row/-card`（min-width 260、padding 2rem 2.5rem）
`.dash-uptime-icon/-value/-label`；`.dash-qr-cards` `.dash-qr-row` `.dash-qr-card/-box/-title/-desc`
`.dash-qr-img`（`cursor:zoom-in` + hover `scale(1.05)`））→
用量六列强制 + 汇总卡覆写 → **图表面板**（`.usage-chart-panel`（hover 抬起）`.chart-controls/.control-group`
`.usage-fallback-note`（带 `::before` 图钉）`.chart-container`（手搓 inline SVG）`.chart-loading`
`.empty-chart/-icon`；SVG 原语 `.usage-chart-bar/-gridline/-ylabel`；`.usage-tooltip/.tt-date/.tt-row`）→
**日表 + 下钻**（`.usage-table-panel` `.usage-panel-title` `.usage-table-wrap`
`.usage-daily-table`（整行 `cursor:pointer`）`.usage-drill/-head` `.usage-records-table` `.usage-pager`）→
`@media 1024/768/640`。

**`sections-tools.css`（560 行）** —— modeltest / logs：
`.logs-head` `.mt-check`（16px 复选 + accent-color）→
**Playground**（`.mt-layout` `grid minmax(320px,1fr) minmax(340px,1.2fr)` gap 1.25rem；
卡 hover 抬；`.mt-panel-title`（下边一条线）；`.mt-key-row/.mt-model-row`（输入 + 按钮同排）；
`.mt-hint-warn`；`.mt-endpoint/.mt-radio`；`textarea.mt-prompt` min-height 130px；
`.mt-image-preview/.mt-img-thumb/.mt-img-del`；`.mt-actions`；
`.mt-response-card`（min-height 420）`.mt-response-head`（标题 uppercase + letter-spacing .04em + tertiary）
`.mt-meta/-item/-label/-val`（主色 + tabular-nums）；
**`.mt-chat`**（min 260 / max 620 / overflow-y / gap 1rem）`.mt-chat-placeholder`；
**气泡**：`.mt-msg`（`mtFadeIn .25s`）、`.mt-msg-user{flex-direction:row-reverse}`、
`.mt-avatar` 34px 圆（用户 = 实心主色白字；助手 = bg-tertiary + 2px 主色边 + 主色字）、
`.mt-bubble`（max-width 78%、padding .7rem .95rem、radius 12px、`.9rem/1.65`、`pre-wrap`；
用户气泡实心主色白字 + 右上角 4px；助手气泡 bg-secondary + 边 + 左上角 4px）、
`.mt-chat-img`（200×200 上限）、`.mt-typing-cursor`（`mtBlink`）、`.mt-error/-title/-body`）→
**日志**（`.logs-toolbar` `.logs-search`（左内嵌 16px 图标）`.logs-levels` `.logs-actions`；
`.logs-status` 三态胶囊（connected 绿 + 呼吸 / reconnecting 黄 / disconnected 红）；
`.logs-layout` flex，**`.logs-console` 亮色下也是深色面**（`--code-bg`、560px 高、等宽 .78rem/1.55）；
`.log-line`（hover `rgba(255,255,255,.06)`、`.selected` 主色 10%）`.log-ts`(#7d8590) `.log-level`(min-width 48px、
uppercase、700) `.log-msg`；**五档级别配色**（error `#ff7b72`/`#ffb3ad`、warn `#f0b849`/`#e8cf9a`、
info `#6cb6ff`、debug `#8b949e`/`#9aa4af`、trace `#6e7681`/`#8b949e`）；`.logs-empty`；
`.logs-detail`（360px 右栏）+ `-head/-body`）→ `@media 1024/768`。

### 2.11 agnes 侧要**丢弃**的 CSS（逐条点名）

P-1 是「整套搬」，所以必须点名**哪些不要**，避免两套混在一起：

| 丢弃 | 替代 |
|---|---|
| `base.css` 的 `--bg/--panel/--panel-2/--text/--muted/--border` 六个 token 名 | kiro 的 `--bg-secondary/--bg-primary/--bg-tertiary/--text-primary/--text-secondary/--border-color` |
| `--danger-fg/--warn-fg/--ok-fg` 一族（「画在有色底上的前景色」） | kiro 无此概念；toast 改成 `bg-primary` + 4px 左色条（见 2.8.7）。⚠️ **agnes 为这一族踩过三次坑**（`.poll-dot`、`.pg-hint`、`.chip.active` 各一次），换成 kiro 的形态之后这个坑的地形整个消失 |
| `--gap`/`--gap-sm`（16/8px 两档） | kiro 的九档 rem 间距标度（2.5 节） |
| 裸 `button` = 主按钮 + `button:where(:not([disabled]):not(.badge)):hover` 那套特指度体操 | kiro 的 `.btn` + 8 个变体类。⚠️ **这是一次真正的减负**：agnes 那条规则的注释写了 20 行解释 `:where()` 为什么不能删、`.badge` 为什么要进 `:not()` 名单，而 kiro 那边**这个问题不存在**（按钮不带默认样式，一切靠类） |
| `.shell.on` 的 grid 栅格 + `.topbar` `.side` `.main` | kiro 的 `.container/.app-header/.main-content/.sidebar/.content` |
| `.gate/.gate-card/.gate-head` | `.login-overlay/.login-card/.login-brand`（gate 的主题按钮留下，见 2.8.9） |
| `.main *:has(> table) { overflow-x:auto }` 那条 `:has()` 兜底 | kiro 的 `.table-container`（显式 wrapper）。⚠️ 代价如实写：改成显式 wrapper 要在每个渲染表格的地方多包一层 `<div class="table-container">` —— 但那正是 agnes 自己注释里登记的「真要那么放时的正确处置」 |
| `.card-row` / `.cfg-col`（`flex:1 1 300px`）/ `.cfg-grid`（`minmax(min(100%, max(180px,40%)),1fr)`） | `.settings-grid`（`repeat(2, minmax(0,1fr))` + ≤768 单列）。⚠️ **这是本次丢弃里最可惜的一条**：agnes 那条网格公式（40% 下界保证「够宽恒两列、不够恒一列，中间没有第三种排布，而且结论不随卡宽变」）是量出来的，kiro 那条简单的 `repeat(2,…)` 在很窄的卡里会挤。**处置**：设置页的**卡级**两列用 kiro 的 `.settings-grid`；**卡内字段**那一层如果仍然要两列，把 agnes 那条 `.cfg-grid` 公式**原样保留**并改名 `.field-grid`（它不与 kiro 的任何类冲突）。理由：agnes 的设置页有 25 个可编辑项，kiro 只有 2 组 —— **kiro 的设置页从来没面对过这个密度** |
| `.btn-toggle` / `.tabs/.tab` / `.chips/.chip` / `.btn-tiny` / `.dropdown-menu` | `.seg-btn` / `.login-tab` / `.chip-btn` / `.acct-icon-btn.is-mini` / `.add-menu` |
| `.banner-danger/-warn/-info` | ⚠️ **kiro 没有横幅组件**。保留 agnes 这三条（换成 kiro 的 token 名 + `--radius-lg`），它们服务的是 agnes 独有的「降级横幅」「用量口径提示」（§4 表里 U-3） |
| `.usage-none`(EN DASH) / `.usage-unknown`(EM DASH) / `.usage-incomplete` / `.approx` | ⚠️ **全部保留**。这是 agnes 明确更强的一面：「读成功但没样本」与「整块没读出来」是两件事，kiro 把两者都画成 `—`（它自己的回归测试第 ⑦ 条正是「统计接口挂了却印出 0.0000 冒充真实零流量」——**同一个病，kiro 只治了一半**） |
| `.badge-off`（`text-decoration: line-through`） | 保留（协议不可用档，WCAG 1.4.1 的非颜色线索） |

### 2.12 新造板块的样式怎么造

用户点名的两个 agnes 独有板块（playground 1,721 行、registrar 665 行）在 kiro 里没有对应物。
**「照 kiro 的设计语言新造」的操作定义如下五条**：

> **R1 先找现成的**。kiro 的 1,470 + 678 + 560 行里已经有 200+ 个类，**绝大多数需求能直接复用**。
> 只有复用不了才新造。
> **R2 新造的类名进对应的文件、用对应的前缀**：playground → `sections-tools.css` 的 `.mt-` 前缀；
> registrar → 新建 `sections-registrar.css`、`.reg-` 前缀。**不往 `base.css` 里塞板块专用规则**。
> **R3 一切取值只从 2.2/2.3 的 token 与 2.5 的两条标度里取**，零硬编码色值、零新字号、零新间距。
> **R4 新组件必须落进三条手感里**（2.1）：卡片 hover 抬起、悬停边框换 `--primary-20`、过渡走 `--transition`。
> **R5 状态不许只由颜色表达**（WCAG 1.4.1）：任何「选中/停用/不可用」都要再配一条非颜色声明
> （字重、删除线、图标、形状）。这条是 agnes 已经为 `.btn-toggle.active` / `.badge-off` / `.nav-item.active`
> 各表过一次态的同一条线。

#### Playground（`sec-playground.js`，9 个 UI 单元）

| agnes 的单元 | 复用 / 新造 | 具体 |
|---|---|---|
| 左右两栏（`.pg-cols`） | **复用** `.mt-layout` | `grid minmax(320px,1fr) minmax(340px,1.2fr)`，≤1024 单栏 |
| 协议档位（4 条）选择 | **复用** `.seg-group/.seg-btn` | 一排四颗，`.is-active` 实心主色 |
| 模态（chat / image / video）选择 | **复用** `.seg-group/.seg-btn` | 第二排，与协议排之间 `gap .5rem` |
| 模型下拉 + 流式开关 | **复用** `select.form-control` + `.mt-check` | |
| 网关口令输入 + 一致性提示（`.pg-hint-ok/-bad`） | **复用** `.mt-key-row` + `.form-hint` / `.mt-hint-warn` | ⚠️ **三态必须同时靠文字表达**（agnes 现有纪律），颜色只是加强 |
| 提示词输入 | **复用** `textarea.mt-prompt` | |
| 对话区（`.pg-turn`） | **复用** `.mt-chat` + `.mt-msg/.mt-avatar/.mt-bubble` | agnes 现在是「上下两条头部信息 + 中间内容」的平铺列表，换成 kiro 的**气泡流** |
| 响应体原文（`.pg-body`，`white-space:pre`） | **新造** `.mt-raw` | 照 `.logs-detail-body` 的规格（等宽 .78rem/1.55、`pre-wrap`、`overflow:auto`）+ `--code-bg` 面 + `max-height:320px` |
| 媒体结果（`.pg-media-row/-url/-img`） | **新造** `.mt-media-row` / `.mt-media-url` / `.mt-media-img` | `.mt-media-img` 照 `.mt-chat-img`（限 320×320、`radius-md`、1px 边）；`.mt-media-url` 照 `.kv-mono` + `word-break:break-all`；行布局照 `.usage-strip .us-item` |
| 视频轮询进度 | **复用** `.meter` | 10px 胶囊；超时档走 `.meter.warm` |
| 「取消」按钮 | **复用** `.btn.btn-outline` | 在飞时与「发送」并排（`.mt-actions`） |
| 「协议目录读不出来」（`.pg-unknown`） | **复用** `.empty-state` + `.text-danger` | |

#### Registrar（`sec-registrar.js`，新建 `sections-registrar.css`）

| agnes 的单元 | 复用 / 新造 | 具体 |
|---|---|---|
| 顶部两页 TAB（运行状态 / 设置） | 🔴 **复用 `.login-tabs/.login-tab/.login-pane`**，改名 `.reg-tabs/.reg-tab/.reg-pane` | kiro 已有一个一模一样的组件（底线 + 选中项用 2px 主色下边框顶开那条线），**别新造**。⚠️ 选择器钉 `aria-selected="true"` 而不是 `.active` 类（agnes 现有纪律：读屏器读的就是那个属性），并保留 `font-weight:600` |
| 池子四格 | **复用** `.accounts-stat-grid` + `.acct-stat-card/-label/-num/-sub` | 固定 4 列，`.ok/.warn` 两档数值色 |
| **两张完全对称的通道卡** | **复用** `.card` + `.svc-card/.svc-row/.svc-row-label/.svc-row-val` | 🔴 **不许出现任何 `[data-channel="…"]` 选择器**（agnes 设计 §10.3 第 2 条，有判据钉着「加一行说明就变红」）。两张卡放进一个 `display:grid; grid-template-columns:repeat(2,minmax(0,1fr)); gap:1rem` 的 `.reg-channels`，**等高由 grid 天然给**（agnes 现在靠 `.card-row > .card + .card { margin-top:0 }` 打补丁，换 grid 之后那个补丁不需要了） |
| 通道连通性测试结果行（`.channel-result`，恒占一行高度） | **新造** `.reg-test-result { min-height:1.2em; font-size:.8rem }` | 恒占位的理由保留：不占位的话测完一条两张卡会一高一低 |
| 补池历史表 | **复用** `.table-container` + `.data-table` | 时间列 `.kv-mono`，原因列 `.badge-*` |
| 「立即补池」确认弹窗（明示消耗） | **复用** `.modal` + `.modal-desc` | 消耗数字用 `.usage-strip` |
| 域名台账（黑白名单计数） | **复用** `.key-stat-grid/.key-stat` | 四格 |
| 退避状态 | **复用** `.status-badge` 的三态形态 | |
| 「设置」分页里那张配置卡 | **复用** `.card` + `.settings-grid`/`.field-grid` | 实现仍然只有一份（`sec-settings.js` 的 `registrarConfigPanel`），只是挂在这里 |

---

## 3. 板块对应表

### 3.1 一一对应（**P-5 / P-6**）

| # | kiro `data-section` | agnes `data-section` | **P2b 的 `data-section`** | 导航文案（zh-CN） | 图标 |
|---|---|---|---|---|---|
| 1 | `dashboard` | `overview` | **`dashboard`** | 仪表盘 | 📊 |
| 2 | `accounts` | `keys` | **`accounts`** | 账号管理 | 👤 |
| 3 | `models` | `models` | **`models`** | 支持模型 | 🤖 |
| 4 | `apikeys` | `apikeys` | **`apikeys`** | API 管理 | 🔑 |
| 5 | `modeltest` | `playground` | **`playground`** | 调试台 | 🧪 |
| 6 | `usage` | `usage` | **`usage`** | 使用统计 | 📈 |
| 7 | `logs` | `events` | **`logs`** | 实时日志 | 📜 |
| 8 | — | `registrar` | **`registrar`** | 注册机 | 🤖→**♻️** |
| 9 | `settings` | `settings` | **`settings`** | 设置 | ⚙️ |

⚠️ **05 §7 那张表把 registrar 排在「第 9 个」，本文改成排第 8**（在 logs 与 settings 之间）：
kiro 的侧栏顺序是「看 → 管 → 试 → 查 → 调」，settings 恒在最后一项；把注册机塞在 settings 之后
会让「设置」不再是最后一格，**而那是 kiro 侧栏唯一一条肉眼可见的排序规律**。
⚠️ 图标撞车：agnes 的 registrar 用 🤖，而 kiro 的 models 也用 🤖 ⇒ 注册机改用 ♻️（「循环补池」）。

⚠️ **P-6 的连带影响（两条，别漏）**：
1. `localStorage` 的板块记忆键 `agnes2api_section` 里存的旧值（`overview`/`keys`/`events`）
   在新版里**不再是合法板块名** ⇒ 必须有「不认识的值回落到默认板块」的兜底
   （两边的 `showSection` 都已经这么写了：kiro `VALID.indexOf(name) < 0` / agnes `if (!SECTIONS[name])`）。**别改这条**。
2. i18n 的 `nav.*` 键要跟着改名（`nav.overview`→`nav.dashboard` 等），
   而 agnes 的 i18n 门禁有一条「未被引用的 key 是硬错」⇒ 旧键必须删干净。

### 3.2 逐板块：kiro 有什么 / agnes 有什么 / P2b 长成什么样

#### ① `dashboard` 仪表盘

**kiro（`sec-dashboard.js` 985 行）**：
- 顶部 **5 张统计卡一行**：账号（可用/总）、今日积分、今日花费、**全局剩余积分**（带「基于 X/N 个已缓存账号 · 更新于 Y 前」的出处小字 + 一颗 `.btn-xs` 刷新）、可用模型数；
- **系统信息面板**（`.dash-sysinfo`，固定 4 列 × 2 行）：版本 / Kiro 版本 / 服务器时间 / OS / 内存 / CPU / 运行模式 / PID；标题行右端一颗「检查更新」；标题里挂版本号；一枚**实时轮询策略徽章**；
- **uptime 卡 + 两张二维码卡**一行；
- 数据源 7 条：`/credentials` `/usage/daily` `/models` `/server-info` `/config` `/config/load-balancing` `/credits/global`，外加冷启动/手动才发的 `/credentials/{id}/balance` 扇出；
- 🔴 扇出有**并发上限 + 401 立刻整轮停 + onHide 作废**三条护栏（`tests/balance-fanout.test.mjs` 7 格钉着）。

**agnes（`sec-overview.js` 381 行）**：池四格（总/可用/冷却/剔除/停用）、运行时信息面板、**新鲜度卡**、累计用量卡、配置摘要（**含「被环境变量锁死的字段数」**）、存储卡（backend + 可写性）、降级横幅。

**P2b**：
- 布局**完全照 kiro**：5 张统计卡 + 系统信息面板 + uptime/二维码行；
- 五张卡的内容按 agnes 的语义重填：**池总数/可用** · **今日请求** · **今日花费**（C3 定价表落地后）· **今日 token** · **可用模型数**；
  🔴 **「全局剩余积分」那张卡先不画**（05 §9.1：Agnes 上游有没有查余额接口，全仓零证据）。**不画 ≠ 留空卡** —— 五张卡就是五张，第五张放「可用模型数」；
- 系统信息面板的 8 格换成：版本 / 运行时（rust + 版本）/ 服务器时间 / OS / 内存 / CPU / 运行模式 / PID；**加一枚轮询策略徽章**（priority/balanced）；
- **agnes 的三样长处并进去，不丢**：
  · **新鲜度提示** → 做成系统信息面板下面一条 `.mng-inline-note`（「配置保存后最多 N 秒生效」「别的实例判定的冷却最多晚 N 秒」）；
  · **被 env 锁死的字段数** → 做成系统信息面板里的第 9 格 `.dash-info-item`，点击跳设置页；
  · **降级横幅** → `.banner-danger`，置于 `.section-header` 之下、统计卡之上；
- 二维码两张卡：agnes 没有社群/赞助二维码 ⇒ **换成「文档」与「GitHub」两张卡**（同样的 `.dash-qr-card` 形态，`.dash-qr-box` 里放 emoji + 一行说明）。⚠️ 别留空的虚线框。
  🔴 **一张卡上是三条各自独立的词条，不是两条 + 一条复用**：框内 `.dash-qr-hint`（`.65rem` / `--text-tertiary`，
  装在 120×120 的定高框里 = **那张不存在的二维码图的替身**，只写「去哪儿」，必须短）、框下 `.dash-qr-title`
  （`.9rem` / 600）、再下面 `.dash-qr-desc`（`.8rem` / `--text-secondary`，整句话）。
  ⚠️ **拿 `desc` 去填框内那一行会让同一句话在屏幕上各印两遍** —— 这不是假设，是真浏览器截图抓到过的实况：
  框里被挤成三到四行（zh 41px / en 54px 高）的那句，与框下那句逐字相同。

#### ② `accounts` 账号管理

**kiro（`sec-accounts.js` 2,412 行，全仓最大）**：4 张汇总卡（可点击 = 状态筛选）→ 工具条（搜索 + 状态筛选 + 排序 + 自动刷新 + **添加账号分组下拉**）→ 批量条（选中才出现）→ **一行一张的账号行卡**（10/页）→ 分页。
行卡左列：`#编号` + 昵称 + 健康徽章 + 授权徽章 + meta 行 + 统计行（权重/成功/失败/限流/RPM/剩余，五色档，部分可点开弹窗）；
右列动作簇：**开关**（44×24）+ 5 颗图标按钮（余额 / 用量 / 编辑 / 刷新 / 删除）。
另有：**七档状态分档**（`accountBucket`：禁用 > 额度耗尽 > 封禁 > 过期 > 续期被拒 > 冷却/失败 > 健康，`tests/status-filter.test.mjs` 17 格钉着）、
新鲜度提示（`#accountsFreshness.is-stale` 超两拍变警示色）、
RPM 来自独立的 `/rpm` 快照（列表接口根本没这个字段 —— 曾经每行恒 0）。

**agnes（`sec-keys.js` 1,007 行）**：状态筛选 → 4 张汇总卡 → 分页表格 → 新鲜度提示；行内 7 颗动作（停用/启用/清冷却/清 strikes/解除剔除/删除/**验活**）；批量条（disable/clearCooldown/delete）；导入弹窗；`reveal` 三控件（掩码/显示/复制）。

**P2b**：
- **表格 → 行卡**（`.accounts-list` + `.account-row`）。这是本板块最大的一次视觉改动，也是用户「太丑了」最直接的落点；
- 动作簇：`.acct-switch`（停用/启用）+ `.acct-icon-btn` ×6（**验活** / 清冷却 / 清 strikes / 解除剔除 / 备注编辑 / 删除，删除是 `.is-danger`）；
- 统计行按 agnes 的字段重填：`优先级 / 权重 / 成功 / 失败 / 连续失败 / 冷却剩余 / 最后使用`；
- 汇总卡四格：`总数 / 可用 / 冷却中 / 停用+剔除`（可点筛选，`.is-clickable`）；
- **添加账号分组下拉**（`.add-menu`）保留形态但换内容：`【自动注册】立即补池 / 去注册机` + `【手动】粘贴导入`。
  ⚠️ 05 §9.6 登记过「照抄 kiro 的弹窗会做出三个点不动的按钮」—— 本条就是那个问题的处置；
- 🔴 三条 kiro 已经踩过并修好的坑**必须照抄护栏**（它们在 agnes 侧同样会发生）：
  ① 自动刷新只重拉列表，**绝不顺带重跑扇出**；② 列表重载要**顶掉**在飞的逐行查询；③ `onHide` 作废在飞请求。

#### ③ `models` 支持模型

**kiro（290 行）**：一张卡 + 一张响应式表（id / 展示名 / 类型 / owned_by / max_tokens / 倍率）+ 一颗「刷新」（全池逐凭据重拉，回填 `errors[]` 逐账号原因）。
**agnes（801 行）**：**三张卡** —— ① 模型目录（id / modality / **四协议可用性矩阵** / 端点，视频模型两条端点）② 上游模型（真打一次，双向差集）③ 模型测试（逐模型串行真发一次最小对话，「模型/结果/延迟」表）。

**P2b**：**三张卡全保留**（agnes 明确更强），外观换成 kiro 的：
- 卡 ①：`.card` + `.models-table`；协议矩阵四个徽章用 `.badge-success` / `.badge-off`（保留删除线）；端点用 `.kv-mono`，一条一行；
- 卡 ②：`.card` + `.model-chip-grid/.model-chip` 铺 id；差集两组各一个 `.key-sub-h` 小标题；失败走 `.banner-danger` + `.models-retry`；
- 卡 ③：`.card` + `.data-table`（模型 / 结果徽章 / 延迟 `tabular-nums`）+ `.meter` 做进度；
- **补 kiro 的「逐凭据刷新回填」**（05 G2 裁定「对标」）：卡 ② 的按钮多一档「刷新全池」，错误用 `.refresh-errors/.rex-*` 列表呈现。

#### ④ `apikeys` API 管理

**kiro（1,337 行）**：服务连接卡 → 5 张状态卡 → 排序+搜索工具条 → **卡片式 key 列表，分「绑定账号」/「全局策略」两组**（绑定组用紫色系）→ 创建/编辑弹窗（编号冲突检查、限制方式分段、时长 chips + 自定义、计量单位分段、额度 chips、绑定账号多选下拉）→ 创建后一次性明文弹窗 → 详情弹窗（用量条 + 按模型分解 + 分页记录表）→ 清理弹窗。
**agnes（598 行）**：统计卡 + 工具条 + `.ak-item` 列表（名称/到期/用量/动作）+ 签发弹窗（到期 chips）+ `reveal` 三控件 + 一键清空。

**P2b**：**外观整套照 kiro**（`.key-card` 全族 + 弹窗全族）。字段按 05 §5 的 D1–D4/D6 裁定补到 11 格
（过期、消费上限 + 单位、绑定 key 白名单、逐条用量归属）。
- agnes 独有的 **明文回显（`reveal` 四态 + sticky toast）保留**，做成 `.key-card` 上的一颗 `.acct-icon-btn.is-mini`；
- 「绑定账号」那一组的紫色系（`#7c3aed` 一族 `color-mix`）**照抄**——它是 kiro 里唯一一处跳出 emerald 的配色，作用是把「有白名单约束的 key」从视觉上切出来。

#### ⑤ `playground` 调试台

**kiro `modeltest`（911 行）**：2 个端点（`/v1/chat/completions` / `/v1/messages`）、多轮上下文、流式、图片附件、token 统计、主 key 或已建 API Key 二选一。
**agnes（1,721 行）**：**4 协议 × chat/image/video 三模态**、多轮、流式、取消、媒体结果预览（data 图内嵌 / 链接新标签）、视频两段式轮询（建任务 + 轮询，带上限）、**两把钥匙严格隔离**（网关口令另存一个 storage 键，管理口令一个字节都不许走上对外那条路）、**同源自查 + `redirect:"error"`**。

**P2b**：**功能全保留，UI 壳照 §2.12 的表重做**。
🔴 **三条安全性质一个字都不许松**（它们是 agnes 明确比 kiro 强的地方，也是用户规则第一句直接命中的）：
① 两把钥匙分存、分路；② 发请求前做**同源自查**（目录被改过 ⇒ 一个字节都不发）；
③ `redirect: "error"`（跨源重定向时 `x-api-key`/`x-goog-api-key` 不会被浏览器剥掉，会原样送到新源）。

#### ⑥ `usage` 使用统计

**kiro（673 行）**：6 张汇总卡一行 → **图表面板**（5 档时间范围 6h/24h/3d/7d/30d + 手搓 inline SVG 柱图 + 浮动 tooltip + 主题切换重绘）→ 日汇总表（整行可点）→ 下钻记录表 + 分页。
**agnes（804 行）**：6 张汇总卡 + 时间范围按钮组 + 日汇总表 + **四维下钻**（小时/模型/协议/密钥）+ **五态互斥的诚实标记**（没开 / 读不出来 / 还没落盘 / 真的是 0 / 有数据）+ `≈` 近似标记 + 两种破折号。

**P2b**：布局照 kiro（**补上 kiro 有而 agnes 没有的 SVG 柱图**），语义按 agnes 保留：
- 🔴 **五态互斥 + 两种破折号 + `≈` 全部保留**。理由硬：kiro 自己的回归测试第 ⑦/⑧ 条正是「统计接口挂了却印出 0.0000 冒充真实零流量」——它只在两张卡上修了这个病，agnes 是**整块系统性地**修的；
- 四维下钻保留（比 kiro 的单维记录表强），每维一张 `.data-table`，四张之间用 `.login-tab` 形态的小 TAB 切；
- 图表：照 `.usage-chart-panel` 全族新写（`createElementNS` 手搓 SVG，`themechange` 时重绘）。

#### ⑦ `logs` 实时日志

**kiro（463 行）**：SSE 流（`history` 回放事件 + `log` 增量）+ REST 快照兜底（反代缓冲 SSE 时）+ 下载（`K.api.raw` 取 blob 以便带头）+ 搜索 + 四档级别 + 暂停 + 自动滚动 + **深色控制台**（亮色下也深）+ 右侧详情栏 + 三态连接徽章 + 2000 行环形缓冲。
**agnes（`sec-events.js` 412 行）**：轮询（非 SSE）+ 搜索 + 级别组 + 暂停 + 清空 + 下载 + 自动滚动 + 三态指示灯 + 分组时间线；**事件是持久的**（写存储，重启不丢，带 TTL 有界），且只采白名单诊断事件。

**P2b**：
- 外观照 kiro（`.logs-console` 深色面 + 五档级别配色 + `.logs-detail` 右栏 + `.logs-status` 三态胶囊）；
- 数据面取**并集**（05 F1–F3）：**全量捕获（kiro）+ 持久化（agnes）**；
- 🔴 **实时推送的凭据怎么带 = 未决**（P-15 / §9 R-2）。在主线裁定之前，**P2b 先做轮询**（agnes 现有形态），
  UI 上预留 `.logs-status` 的 `connected/reconnecting/disconnected` 三态（轮询同样有这三态）。
  ⚠️ **别为了「像 kiro」就把管理口令放进 URL**——那会当面推翻 agnes 自己钉过的一条纪律。

#### ⑧ `registrar` 注册机

kiro **无任何可参照物**。用户明确要求保留。外观按 §2.12 的表造。
⚠️ 本板块**不做自动刷新**（agnes 现有纪律：每刷新一次要付 3 次存储读）。

#### ⑨ `settings` 设置

**kiro（345 行）**：两张卡（认证密钥 / 负载均衡），`.settings-grid` 两列。
**agnes（1,302 行）**：**25 个可编辑项** + 类型校验 + provenance（哪些被 env 锁死）+ 凭据清空 + 一键重置 + 集成示例卡（协议 × 语言）+ 危险区；
🔴 一条不可妥协的产品原则：**成功提示不得早于回读**（`await api.put()` 之后才允许出现任何「已保存」的迹象，且**不弹「已保存并生效」**，而是回读 `effective` 并高亮变化的字段）。

**P2b**：**功能全保留**（agnes 强得多），外观：
- `.settings-grid` 两列放卡；卡内字段用保留下来的 `.field-grid`（见 2.11）；
- 锁定 / 变化 / 非法三态的左边框条（2px `--border-color` / `--success` / `--danger`）**保留**——它们是 agnes 为 WCAG 1.4.1 专门配的非颜色+颜色双信号；
- **集成示例卡照 kiro 的 `.int-card/.int-code/.int-copy` 重做**（kiro 已有这个组件，agnes 的 `.examples-code` 丢弃）；
- 危险区：整卡 `border-left: 2px solid var(--danger)`（agnes 现有），按钮用 `.btn-danger` + `align-self:flex-start`（🔴 **别让销毁凭据的按钮变成横贯整卡的红色大条**——agnes 为这条踩过，真机截图上整页最抢眼的就是三颗销毁按钮）。

---

## 4. 功能并集裁定（逐条）

> 规则（用户原话）：**kiro 没有的 / agnes 更强的 → 保留；kiro 有而 agnes 缺或更弱的 → 对标 kiro。**
> 「谁强」逐条**读代码得出**。下表只列 05-feature-matrix 之外、**面板层**新识别的条目（编号 U-n），
> 05 已裁定的（A/B/C/D/E/F/G/H/I/J/K/L 各组）不重复。

| # | 面 | agnes | kiro | 谁强（证据） | 裁定 |
|---|---|---|---|---|---|
| **U-1** | CSS 规模与组件覆盖 | 1,077 行 / 无板块专用文件 | **3,465 行 / 4 份按板块族分文件** | kiro（`.account-row` `.key-card` `.acct-switch` `.mt-chat` `.logs-console` 等 200+ 类 agnes 一个都没有） | **对标**（P-1） |
| **U-2** | 共用 DOM 原语层 | **有**（`js/ui.js`：`el` `elI18n` `svgIcon` `iconBtn` `toast` `openModal` `confirmModal` `copy` `revealControls`） | **无**（`openModal` 3 份、`confirmModal` 3 份、`svgIcon` 3 份**互不相同**的实现，分散在 sec-*.js 里） | **agnes**（agnes `ui.js` 文件头点名，我逐份核对属实） | **保留** |
| **U-3** | 横幅组件 | 有三档（danger / warn / info） | **无** | agnes | **保留** |
| **U-4** | 「不知道」与「没有」的区分 | **有**（EN DASH `–` = 读成功但无样本；EM DASH `—` = 没读出来；`≈` = 有系统性少计；`.usage-incomplete` = 缺分片） | 部分（只在两张卡上用 `—` 代替假的 0） | **agnes**（kiro 自己的回归测试第 ⑦/⑧ 条正是这个病的局部修复） | **保留** |
| **U-5** | 登录闸上的主题切换 | 有 | 无 | agnes | **保留** |
| **U-6** | modal 的焦点管理 | **四条全有**（初始焦点、Tab 陷阱、Escape、`aria-labelledby`） | **零** | agnes | **保留** |
| **U-7** | toast 的 sticky 档 | 有（+ 自动聚焦关闭键 + Escape） | 无（恒 3.2s 消失） | agnes | **保留** |
| **U-8** | 服务状态徽章 | 有（**点一下重新探**，三档，初始文案是「状态未知」而不是假称「运行中」） | 有（20s 轮询 `/health`，三档） | 平（agnes 的「初始不假称」更诚实，kiro 的自动轮询更省事） | **合并**：20s 轮询 + 可点重探 + 初始「状态未知」 |
| **U-9** | 「重启服务」按钮 | **无**（agnes 注释明写「点了没反应的按钮比没有按钮更糟」） | 有（确认 → POST → 轮询 `/health` → 自动刷新，带 2min 上限） | kiro | **对标**（Rust 单进程 + 容器 restart policy，前提已成立；agnes 那条理由针对的是 Worker 形态，P0 已砍） |
| **U-10** | 会话 12 小时绝对上限 | 有（每请求复查，`pure/session.mjs`） | 无 | agnes | **保留** |
| **U-11** | 口令字符集前端拦截 | 有（`sendable.mjs`，与后端逐码位等价，有 parity 判据） | 无 | agnes | **保留** |
| **U-12** | 移动端侧栏 | 横向滚动导航带（无 JS） | **off-canvas 抽屉**（汉堡 + 遮罩 + 状态） | 「UI 对标」直接命中 | **对标 kiro** |
| **U-13** | 深色/浅色切换按钮的图标语义 | **跟随当前主题**（亮色露太阳、深色露月亮） | 同 | 平 | 保留现状 |
| **U-14** | 语言切换控件 | `<select>` 原生下拉，选项是**语言代码**（`zh-CN`/`en`…） | `.lang-menu` 自绘下拉，选项是**母语名**（简体中文 / English / 日本語 / 한국어） | **kiro**（母语名对用户可读；原生 select 在深色下的下拉面板跟不了主题） | **对标** |
| **U-15** | 键盘可见焦点环 | 有（`:focus-visible` 2px 主色 + offset 2） | 有（逐字相同） | 平 | 保留 |
| **U-16** | 状态不许只由颜色表达 | **系统性执行**（`.btn-toggle.active`/`.badge-off`/`.nav-item.active`/设置三态各有一条非颜色声明 + 判据） | **未执行**（`.nav-item.active` `.seg-btn.is-active` `.login-tab.active` 都只有颜色差；`.status-dot` 四档纯色） | **agnes** | **保留**（给 kiro 的那几个类补非颜色声明；代价见 §2.7 末） |
| **U-17** | i18n 门禁 | **10 条**（齐全性、占位符集合各语言一致、未引用 key 是硬错、带占位符的 key 不许当裸标签、未翻译泄漏、偏好词表…）+ 两份独立实现 | **零**（`kiro2api/scripts/` 下只有 `set-version.sh`） | **agnes** | **保留**（落法见 §6.3） |
| **U-18** | 面板判据 | 1,169 格（46 文件 / 33,600 行，含自研 DOM 垫片） | 35 格（3 文件 / 1,177 行，`new Function` + 迷你 DOM） | **agnes**（数量级差） | **保留其判定内容，换执行器**（§5） |
| **U-19** | 安全响应头 | 六条全套 | **零** | **agnes** | **保留**（P-14） |
| **U-20** | 面板资源的缓存协商 | 强 ETag + `no-cache` + 304 | **逐字相同**（`src/webui/mod.rs`，sha256 内容哈希） | 平 | 保留（P-11） |
| **U-21** | 未命中资源的处置 | 404 纯文本 | **SPA 回退**（不带扩展名的路径回 index.html） | agnes（agnes 面板无 URL 路由；kiro 的注释自己写了 SPA 回退曾让「curl 核对某构建产物是否上线」变成骗人） | **保留 agnes**（P-11） |
| **U-22** | `/favicon.ico` | 有（从面板 HTML 里抠内联 PNG，抠不到就**建应用时抛**） | 无独立路由（靠 `<link rel=icon href=logo.png>`） | agnes 略强 | **合并**：改用独立 `logo.png`（P-13）后，`/favicon.ico` 直接服务同一份嵌入字节 |
| **U-23** | 面板与 README 的图一致性 | 有（`check-png.mjs` 逐像素对账 `docs/logo.png`） | 无 | agnes | **保留**（Rust 侧：`logo.png` 与 `docs/logo.png` 的字节关系做成一格 inline `#[test]`） |

---

## 5. `js/pure/*.mjs` 那一层怎么办（🔴 本节是 P2b 最大的一个设计决定）

### 5.1 事实

- agnes 有 **18 份 `pure/*.mjs` / 6,600 行 / 约 260 个导出**，规则是「顶层只许 `export function`/`export const`，
  禁 `import`，禁出现 `document`/`window`」（`admin-ui/README.md` 硬规则 1，`build-ui.mjs` 会拦）；
- 它背后是 `tests/ui/` 的 **1,169 格**，其中约 500 格直接打这一层（`keys.test.ts` `usage.test.ts`
  `settings.test.ts` `registrar.test.ts` `playground.test.ts` …），另约 670 格打 DOM 层（跑在自研垫片上）；
- kiro **没有这一层**：sec-*.js 直接拼 DOM，判定散在渲染函数里。它的 35 格测试**靠 `new Function`
  把 IIFE 源码抠出来、注入迷你 DOM 替身**才测得动——那是「没有分层」逼出来的取证方式；
- agnes2api-rs 的门禁只有 **`cargo build` / `cargo test` / `cargo clippy --all-targets -D warnings` / `cargo fmt --check`** 四道。
  **仓里没有 node 工具链。**

### 5.2 裁定 P-9：分三档

> 用户那条规则管的是「样式和 UI」，**内部分层不在其中** ⇒ 这一层**不因为 kiro 没有就砍掉**。
> 但它也不能原样搬 —— 原样搬会得到「6,600 行 JS + 1,169 格判据，而 `cargo test` 一格都跑不到」，
> 也就是**一套永远绿、永远没人跑的判据**。那比没有判据更坏。

| 档 | 是什么 | 去哪 | 判据形态 |
|---|---|---|---|
| **A：业务判定** | 「这段数据该读成什么」「这个动作现在能不能点」「这个格子该显示哪一档」 | 🔴 **上移 Rust**（`src/core/admin/view/*.rs`），管理端点直接返回**已经判好的视图 DTO**，面板只负责把 DTO 画出来 | inline `#[test]`，进 606 格体系 |
| **B：本地化格式** | 本地时区的时刻串、时长、百分比、字节数、千分位 | **留 JS**，但**去掉一切分支**：分支的那一半（「这个格子是 EN DASH 还是 EM DASH」）由 A 档的 DTO 用一个 `kind` 字段给出 | 无独立判据；由 A 档的 `kind` 覆盖判定部分 |
| **C：浏览器交互状态机** | 多选的选中集合、TAB 的开合、在飞去重、轮询退避的计时、SSE 分帧的缓冲 | **留 JS**，**如实登记为无判据欠账** | 无。⚠️ 写进 §9 R-3 |

### 5.3 逐份归档（18 份，一个不漏）

| 文件 | 行 | 档 | 处置 |
|---|---:|---|---|
| `overview.mjs` | 247 | **A** | `poolCounts` / `processCells` / `usageStats` / `configSummary` / `storageInfo` / `freshnessValues` → `GET /admin/api/overview` 直接给视图 DTO（它现在已经是「逐块降级」的形状，02 §9.2）。`offsetMs` 是 **B** |
| `keys.mjs` | 181 | **A** | `cardCounts` / `badgeClass` / `bucketLabelKey` / `cooldownRemaining` / `pagerState` / `listMessageKey` → `KeyView` 上加派生字段（`bucket` / `badge` / `cooldownRemainingMs`）。⚠️ 这与 kiro 的 `accountBucket` 是同一件事，而 kiro 那 17 格判据（`status-filter.test.mjs`）**正好是这一档最好的参照** |
| `keys-write.mjs` | 711 | **A**（大半）/ **C**（选择集合） | `isDeletable` / `isMustDisableFirstConflict` / `canClearCooldown` / `canUnevict` / `canClearStrikes` / `verifyDisabledReason` / `adminErrorText` → Rust（前五个本来就该是服务端的前置校验，现在是前后端各一份）。`toggleSelection` / `applySelectAll` / `headerSelectAllChecked` → **C** |
| `apikeys.mjs` | 267 | **A** | `akCounts` / `akListState` / `akBadgeClass` / `akNameProblem` / `akExpiresAt` → DTO |
| `usage.mjs` | 782 | **A** | 🔴 **最重要的一份**：`usageState` 五态、`cellKind` / `ratioKind` 两种破折号、`summaryCards` / `dayRows` / `breakdownRows` / `pendingTail` → 全部上移。这一层是「不许伪造 0」那条纪律的实现体 |
| `models.mjs` | 277 | **A** | `protocolBadges` / `catalogModels` / `upstreamModelsView`（双向差集）→ DTO。⚠️ `protocol_catalog.rs` 在 00 §2.1 里已经规划好了，这正是它的消费者 |
| `model-test.mjs` | 298 | **A** + **C** | `testableModels` / `testProgress` / `*ResultCode` → A；`nextTestDelayMs` / `initTestRows` / `withRow*` → C |
| `registrar.mjs` | 571 | **A** | `statusView` / `channelCards` / `poolView` / `tendCost` / `historyRows` / `roundOutcome` / `domainLedgerView` / `backoffView` → `GET /admin/api/registrar/status` 的 DTO。🔴 `CHANNELS` 顺序（字母序，唯一可辩护的中立规则）跟着上移 |
| `settings.mjs` | 845 | **A** | `fieldView` / `buildPatch` / `localErrors` / `changedFields` / `propagationView` / `displayValue` / `resetWarnings` → 大半本来就与 `core/admin/config_validate.rs` 是同一件事的两份实现。**合并成一份** |
| `events.mjs` | 468 | **A** + **C** | `levelBadgeClass` / `eventsQuery` / `bufferStatus` / `groupEvents` / `buildDetailText` → A；`initialPollState` / `pollOutcome` / `nextPollDelayMs`（退避）→ C |
| `playground.mjs` | 950 | **A** + **C** | `playgroundProtocols` / `buildRequest` / `mediaEndpoints` / `deltaText` / `videoTaskIdOk` / `buildPollRequest` → **A**（它们就是「协议目录」这份单一真源的投影，00 §2.1 的 `protocol_catalog.rs` 已经在规划里）；`sseFrames`（跨块分帧）/ `trimTurns` / `videoPollNext` → **C**。⚠️ `sseFrames` 在 Rust 侧其实已经有孪生体（`core/protocol/sse.rs`），**两边的判据可以互为参照** |
| `examples.mjs` | 171 | **A** | `allExamples` / `exampleFor` → 设置页的集成示例由 `GET /admin/api/models` 的目录现拼 ⇒ 上移到 `core/admin/examples.rs` |
| `reveal.mjs` | 123 | **A** | `revealOutcome` / `revealErrorOutcome` 四态 → 端点直接给 `kind` |
| `health.mjs` | 59 | **A** | `healthState` / `healthBadge` → `/health` 响应已经有这些字段 |
| `format.mjs` | 79 | **B** | **留 JS**（`Intl` 只有浏览器有）。⚠️ `fmtDash(null)` 返回 EM DASH 的那条分支**移走**，改由 DTO 的 `kind` 决定 |
| `session.mjs` | 80 | **B**（`SESSION_MAX_AGE_MS`）+ **C** | 留 JS。⚠️ 12 小时这个常数**同时在 5 份 DEPLOY.md 里承诺过** ⇒ Rust 侧建议也放一份常量并做一格「两处同值」的判据（`include_str!` 比对） |
| `storage-keys.mjs` | 84 | **C** | 留 JS。7 个存储键名的单一真源（曾经「只改一处 ⇒ 登录成功后每请求送空口令头 ⇒ 登出循环，而全套用例照绿」） |
| `sendable.mjs` | 83 | **A** | 🔴 与后端 `SENDABLE` 逐码位等价，现在靠一格 parity 判据维持。上移之后：Rust 是唯一真源，JS 侧只留一个**由 Rust 端点吐出来的字符集描述**或干脆保留这一份并配一格 Rust 侧的「同一份表」判据 |

**净效果**：`js/pure/` 从 18 份 / 6,600 行收缩到约 **5 份 / 900 行**（format / session / storage-keys / 三个 C 档状态机），
其余判定全部进 Rust 的 inline `#[test]`。面板 JS 总量估计从 17,776 行降到 **9,000–11,000 行**。

### 5.4 代价与反对意见（如实写）

1. **端点会变「胖」**：DTO 从「原始数据」变成「已判好的视图」。
   ⚠️ 这不是纯粹的损失 —— agnes 的管理端点**本来就是这个形状**（02 §9.2 的 `/overview` 是「逐块降级」，
   §9.23 的 `/usage` 直接给 `tier`），P-9 只是把已经开始的事做完。
2. **一次网络往返换一次本地计算**：`usageState` 这类判定现在在浏览器里零成本，上移后跟着响应走。
   ⇒ **不新增往返**（判定结果搭在原来那次响应里），只增加响应体几十字节。
3. **失去「不起浏览器就能验前端判定」这个性质**：现在是 `vitest` 直接 import `.mjs`。
   ⇒ 上移后变成「不起浏览器就能验**服务端**判定」，性质更强（服务端判定同时服务 API 用户）。
4. 🔴 **真正的损失是 C 档那三类**（选择集合、轮询退避、SSE 分帧缓冲）：
   它们**留在 JS 且没有执行器** ⇒ 判据归零。**这是必须写进 §9 的欠账。**
   替代方案（要主线裁）：在 Rust 仓引入 node + vitest 作为**第五道门禁**，只跑 C 档那几百格。
   代价：prepush 多一个工具链、CI 多一步、Docker 构建阶段多一层。

---

## 6. i18n

### 6.1 两边的事实

| | kiro2api | agnes2api |
|---|---|---|
| 语言 | 5：`zh-CN` `zh-TW` `en` `ja` `ko` | **逐字相同** |
| 默认 | `zh-CN`；**会读 `navigator.language` 归一化**（`zh-hans`/`zh-sg`→zh-CN，`zh-hant`/`zh-tw`/`zh-hk`/`zh-mo`→zh-TW，前缀匹配 ja/ko/en，兜底 en） | `zh-CN`；**不读 navigator**，只读 `localStorage`，否则恒 `zh-CN` |
| 兜底链 | `lang` → `en` → key 本身 | `lang` → `zh-CN` → key 本身（+ `DEBUG_STORE` 为真时 `console.warn`） |
| 结构 | `window.I18N = { 'key': { 'zh-CN':…, 'zh-TW':…, 'en':…, 'ja':…, 'ko':… } }`，**key-major** | `export const I18N = { "key": { … } }`，**key-major**（文件头解释：key-major 下「加了键只填一种语言」在 diff 里一眼可见，lang-major 下它散在五个相距几百行的地方） |
| key 数 | **649** | **743** |
| 命名空间 | 25 个（`acc` 79 / `key` 67 / `dash` 64 / `keyForm` 53 / `mt` 45 / `usg` 42 / `us` 32 / `imp` 31 / `common` 27 / `login2` 26 …） | 15 个（`set` 121 / `reg` 107 / `keys` 95 / `usage` 71 / `ak` 68 / `models` 66 / `pg` 60 / `ov` 52 / `ev` 36 / `err` 26 / `gate` 10 / `nav` 9 / `common` 9 / `shell` 8 / `reveal` 5） |
| 三钩子 | `data-i18n`→textContent、`data-i18n-ph`→placeholder、`data-i18n-title`→title **+ aria-label（无则写）** | **逐字相同** |
| 插值 | `t(key, vars)`，`{token}` 用 `split().join()` 替换（不用正则） | **逐字相同** |
| 门禁 | **零** | **10 条**（见下） |

### 6.2 agnes 的 10 条 i18n 门禁（`scripts/check-i18n.mjs` + `tests/unit/i18n-dict.test.ts` 两份独立实现）

① 每个 key 五种语言齐全且非空；② 板块里出现的每个 key 都在字典里（含 `elI18n`/`labelKey` 这类**当参数传**的）；
③ 字典里每个 key 都**被引用过**（未引用是**硬错**，不是警告）；④ 属性与 `t()` 调用两种窄形态的扫描；
⑤ 命名空间登记表 + 三条反向自检；⑥ **偏好词前缀表**（两条邮箱通道必须完全平级，`reg.*` 等前缀下不许出现「默认/首选/推荐」这类词）；
⑦ 枚举穷尽（`TendFailureReason` 的每个成员都有对应 key）；
⑧ **带 `{占位符}` 的 key 不许被当成裸标签用**（面板上会出现裸的 `{status}`；这条是人工冒烟真抓出来的已上线缺陷）；
⑨ 未核实事项的红线词；⑩ **未翻译泄漏**（整段中文抄进别的语言）。
另有一条隐含的：**同一 key 的插值占位符集合在五种语言之间必须一致**
（`/\{(\w+)\}/g` 取集合后比对，不一致当场报 `key 的插值占位符在各语言间不一致: … | …`）。

### 6.3 裁定 P-10：字典进 Rust 判据的落法

**问题**：这 10 条是 node 脚本，Rust 仓没有执行器。
**方案**：把 `js/i18n-dict.js` 的文件体改写成
```js
/* …文件头注释… */
export const I18N =
{
  "nav.dashboard": { "zh-CN": "仪表盘", "zh-TW": "儀表板", "en": "Dashboard", "ja": "…", "ko": "…" },
  …
}
;
```
—— 即 **`export const I18N =` 与 `;` 之间是一整块合法 JSON**（键全带双引号、无尾逗号、无注释）。
于是：
- 浏览器侧照旧 `import { I18N } from "./i18n-dict.js"`，**零变化**；
- Rust 侧 `include_str!("../webui/js/i18n-dict.js")`，掐掉 `export const I18N =` 前缀与末尾 `;`，
  `serde_json::from_str::<BTreeMap<String, BTreeMap<String, String>>>()` **严格解析**（解析失败即判据红）。

据此可落的 inline `#[test]`（**至少 8 格**，全部进 606 格体系）：

| 格 | 断言 | 变异复检怎么做 |
|---|---|---|
| i18n-1 | 字典能被严格解析成 `key → {lang → text}` | 往字典里加一个尾逗号 |
| i18n-2 | 每个 key 恰好有 5 种语言，且都非空 | 删掉某个 key 的 `ko` |
| i18n-3 | 同一 key 的 `{占位符}` 集合五语言一致 | 把 `en` 的 `{status}` 改成 `{code}` |
| i18n-4 | 字典里每个 key 都在面板源码里被引用过（扫 `webui/` 下 `.js`/`.html` 的 `data-i18n*="…"` 与 `t("…")`/`elI18n("…"` 两种形态） | 加一个没人用的 key |
| i18n-5 | 面板源码里出现的每个 key 都在字典里 | 在某个 sec 文件里写一个不存在的 key |
| i18n-6 | 带占位符的 key 每一次以字面量出现时**后面紧跟 `,`**（= 真的传了参数） | 把一处 `t("gate.httpError", {status})` 改成 `t("gate.httpError")` |
| i18n-7 | 未翻译泄漏：非 zh 的三种语言里不许整段与 `zh-CN` 逐字相同（白名单：纯符号 / 专名 / `ADMIN_TOKEN` 这类） | 把某条 `en` 抄成中文 |
| i18n-8 | 偏好词前缀表：`reg.` / `set.field.channel.` 等前缀下不许出现偏好词；**且表里每条前缀都至少命中一个真 key**（反向控制） | 往 `reg.*` 某条里塞「默认」 |

⚠️ **第 ⑦ 条（枚举穷尽）在 Rust 侧反而更好做**：`TendFailureReason` 是 Rust 枚举，
`strum`/手写 `ALL` 数组 + 逐个查字典即可 —— 这是**从 node 搬到 Rust 之后变强的一条**，
agnes 那边因为「联合类型是编译期的、node 脚本枚举不出来」不得不把它放进 vitest。

### 6.4 词条并集

- kiro 的 `header.*`（重启四态）、`nav.menu`、`login2.*`（交互式登录）等**新增**；
- agnes 的 `reg.*` 107 条、`pg.*` 60 条、`err.*` 26 条**保留**；
- agnes 的 `ov.*` 52 条随 `overview`→`dashboard` **改名到 `dash.*`**（P-6 的连带）；
- 预估 P2b 的字典规模：**900–1,000 个 key × 5 语言**。

---

## 7. 面板怎么进 Rust 二进制

### 7.1 kiro 怎么做的（`kiro2api/src/webui/mod.rs`，138 行 + 4 格测试）

```rust
#[derive(RustEmbed)] #[folder = "admin-ui-v2"] struct AdminAssets;
```
- **零构建步骤**：`rust-embed` 在编译期把整个目录烧进二进制，没有 npm、没有打包器、没有生成的 `.rs`/`.ts` 文件入仓；
- ETag = `f.metadata.sha256_hash()` 的十六进制（**内容哈希**，同一份构建里内容不变哈希也不变）；
- `Cache-Control: no-cache`（= 可以存但每次必须回源验证）+ `If-None-Match` 命中回 **304 不带 body**；
  注释里记着这条的起因：**「后端已是 0.7.9,面板却显示 0.7.6，而服务端每个接口查出来都是对的，这类现象最难查」**；
- MIME 由 `mime_guess::from_path` 推；
- 路径含 `..` → 400；
- 三条路由（`/admin`、`/admin/`、`/admin/{*file}`）都落到同一个 `serve::<A>()`，
  **刻意不用 `.nest()`**（axum 0.8 的 nest 对挂载路径末尾斜杠敏感，`nest("/admin", ..)` 不匹配 `/admin/`）；
- **SPA 回退只对「不带扩展名」的路径生效**，带扩展名的一律 404
  （无条件回退时 `assets/index-abc123.js` 也拿到 200 + 一整篇 HTML ⇒ 浏览器报 `Unexpected token '<'`，
  而且**用 curl 核对某个构建产物是否上线时，不存在的文件也返回 200 —— 校验本身在骗人**）；
- 4 格判据：缺失资源 404 / 资源带 ETag + no-cache / 未变回 304 且空 body / 不同资源 ETag 不同。

### 7.2 agnes TS 版怎么做的（`agnes2api/src/ui/serve.ts` + `scripts/build-ui.mjs`）

- `build-ui.mjs` 把 `admin-ui/` **逐字节**烧进 `src/ui/assets.generated.ts`（不转译/不打包/不压缩），
  **生成物入仓**（因为 `wrangler deploy` 与 CF 的 Deploy 按钮会绕过一切 npm script）；
- `tests/unit/ui-assets.test.ts` 重跑一遍生成器再整文件比对 ⇒ **改了面板忘跑 `pnpm ui:build` 会红**；
- 扩展名白名单 `.html/.css/.js/.mjs/.json`（**刻意没有 `.svg`**，理由见 P-13）；
- 2 MiB 原始字节预算（早期预警线）；
- 查表命中制（**零路径拼接** ⇒ 路径穿越在结构上就不成立）；
- `/admin/` **301** → `/admin`；未命中 404 纯文本；`/favicon.ico` 从面板 HTML 里抠内联 PNG；
- 六条安全头**每一条响应都带**（含 301 与 404 —— 注释写着理由：301 会被近乎永久缓存）。

### 7.3 裁定 P-11 / P-12：两种形态的取舍

| 轴 | `build-ui.mjs` 生成 `.ts`（TS 版） | `rust-embed`（kiro） | 取 |
|---|---|---|---|
| 构建步骤 | **有**（忘跑会被判据抓住，但仍是一个人必须记住的步骤） | **零** | **rust-embed** |
| 生成物入仓 | 是（1 份与源同构的巨大 `.ts`，review 时噪声大） | 否 | **rust-embed** |
| 「产物与源一字不差」 | 靠一格判据 | **编译期读原文件，结构上不可能不一致** | **rust-embed** |
| 扩展名白名单 / 脚本校验 / 体积预算 | 有（生成器里） | 无 | ⚠️ **需要补**（见下） |
| 路径穿越 | 结构上不成立（查表） | 靠一句 `path.contains("..")` | ⚠️ rust-embed 的 `A::get(path)` 本身也是查表（键是相对路径），`..` 那一判是**额外**的一道，**保留** |
| 未命中 | 404 纯文本 | **SPA 回退** | **404 纯文本**（agnes 面板无 URL 路由） |
| `/admin/` | **301** | 直接发内容 | **直接发内容**（301 会被近乎永久缓存；两种写法在绝对路径下都工作） |

🔴 **改用 `rust-embed` 之后，TS 版生成器里那三件事没有对象了，必须在 Rust 侧补回来**（各一格 inline `#[test]`）：

| 补什么 | 判据形态 |
|---|---|
| **扩展名白名单** | 遍历 `AdminAssets::iter()`，断言每个文件的扩展名 ∈ `{html, css, js, mjs, json, png}`（🔴 **`svg` 不在内**） |
| **零内联脚本** | 对每份 `.html` 断言：`<script` 之后到 `>` 之前必须出现 ` src=`（属性边界匹配，`<script data-src="x">payload</script>` 这类假 src 伪装拦得住） |
| **体积预算** | `AdminAssets::iter()` 的字节总和 ≤ 一个明写的上限（建议 2 MiB，与 TS 版同值）。⚠️ 这条在 Rust 下不再是平台约束，纯粹是「早期预警线」，理由要写在常量旁边 |

### 7.4 🔴 CSP × `<base>` 的冲突（P-7，本节最容易踩的一条）

kiro 的 `index.html` 第 7 行是 `<base href="/admin/">`，全部资源用相对路径（`css/base.css`、`js/app.js`、`logo.png`）。
agnes 的 CSP 含 **`base-uri 'none'`**。

⇒ **照抄 kiro 的 `index.html` + 保留 agnes 的 CSP = `<base>` 被浏览器静默忽略
⇒ `css/base.css` 解析成 `/css/base.css`（不是 `/admin/css/base.css`）⇒ 整份 CSS 与 JS 全部 404
⇒ 一张没有样式、没有行为的裸 HTML。**

⚠️ **这个故障形态 agnes 自己在别的路径上撞过一次并写进了 README**（`file://` 打开面板时绝对路径解析成 `file:///admin/…` 而全部 404），
所以它**长得很像一个"环境问题"**，排查时容易往别处找。

**裁定 P-7**：删掉 `<base>`，一律写 `/admin/` 绝对前缀（agnes 现有做法），`base-uri 'none'` 不动。
**验收判据**：一格 inline `#[test]` —— 断言嵌入的 `index.html` **不含 `<base`**，
且每一个 `href=`/`src=` 的值要么以 `/admin/` 开头、要么是 `data:`、要么是绝对 URL（`https://github.com/...`）。
（变异复检：把某个 `/admin/js/app.js` 改回 `js/app.js`。）

### 7.5 路由与响应契约（P2b 的 `src/webui/mod.rs` 规格）

```
GET /admin            → index.html
GET /admin/           → index.html   （🔴 不是 301）
GET /admin/{*file}    → 查表；命中 → 200 + 正确 MIME + 强 ETag + 六条安全头
                                   若 If-None-Match 命中 → 304（无 body，仍带 ETag + Cache-Control）
                        未命中 → 404 text/plain "Not Found"（**不是 JSON 信封** —— 它服务浏览器不服务 SDK）
                        路径含 ".." → 400
GET /favicon.ico      → 200 image/png + 同一份 logo.png 字节 + 自己的 ETag（🔴 前缀区分，不许与别的资源共用校验子）
```
🔴 **整棵树跟着 `/admin` 一起存在或消失**：没配 `ADMIN_TOKEN` 时这几条路由**根本不注册**
（`server/app.rs` 已经是这个形状：`if has_admin { app = app.merge(admin_router(...)) }`），
静态树也要进同一个 `if`。⚠️ **不泄漏「这里有个后台」这条性质，别在 P2b 破掉。**

⚠️ **六条安全头的落点**：kiro 是在 `serve_asset()` 里逐条拼进响应；agnes TS 版是一个常量表。
建议照 agnes：一个 `const SECURITY_HEADERS`，**200 / 304 / 400 / 404 四条路径都带**
（一格判据：四种状态码的响应各断言一次 `content-security-policy` 存在）。

---

## 8. P2b 的交付清单与验收判据

### 8.1 交付物

| # | 交付 | 文件 | 估算 |
|---|---|---|---|
| D1 | 静态树投递 | `src/webui/mod.rs` | ~250 行 + 12 格 |
| D2 | 面板 HTML 骨架 | `webui/index.html` | ~190 行 |
| D3 | 四份 CSS（搬 kiro + P-3/P-4 两处改 + agnes 保留项） | `webui/css/{base,sections,sections-manage,sections-stats,sections-tools}.css` | ~3,500 行 |
| D4 | 新建注册机 CSS | `webui/css/sections-registrar.css` | ~200 行 |
| D5 | 壳层 JS | `webui/js/{boot,i18n,i18n-dict,theme,ui,api,gw-api,health,app}.js` | ~2,400 行 |
| D6 | 九个板块 JS | `webui/js/sec-*.js` | ~7,000 行 |
| D7 | 留下的纯函数 | `webui/js/pure/{format,session,storage-keys,selection,poll,sse}.mjs` | ~900 行 |
| D8 | **A 档上移的视图层** | `src/core/admin/view/*.rs`（overview / keys / apikeys / usage / models / registrar / settings / events / playground） | ~3,000 行 + **约 200 格** |
| D9 | 品牌资源 | `webui/logo.png` | — |
| D10 | 本文的执行记录 | 更新 `docs/porting/05-feature-matrix.md` §7 与 §9（把本文裁掉的未决项划掉） | — |

### 8.2 验收判据（🔴 每一格都要做变异复检）

**A 组 · 静态投递（12 格，`src/webui/mod.rs` inline）**
1. `GET /admin` 与 `GET /admin/` 都回 200 + `text/html`，**且 body 逐字节相同**（变异：把其中一条改成 301）
2. 命中资源带强 ETag 且 `Cache-Control: no-cache`（变异：去掉 ETag）
3. `If-None-Match` 命中回 304 且 **body 为空**（变异：304 分支里带上 body）
4. 两个内容不同的资源 **ETag 不同**（变异：改成按路径长度生成 ETag）
5. 不存在的资源回 **404 `text/plain`**，body 不是 JSON（变异：加回 SPA 回退）
6. 路径含 `..` 回 400（变异：删掉那一判）
7. **200 / 304 / 400 / 404 四条路径都带全六条安全头**（变异：把 `SECURITY_HEADERS` 从 404 分支上摘掉）
8. `GET /favicon.ico` 回 `image/png`，字节 = 嵌入的 `logo.png`，**且 ETag 与任何别的资源都不同**（变异：直接复用 `/admin/logo.png` 的 ETag）
9. **没配 `ADMIN_TOKEN` 时 `/admin` 与 `/favicon.ico` 都 404**（变异：把静态树挪到 `if` 外面）
10. 嵌入文件的扩展名全在白名单内、**且不含 `.svg`**（变异：往 `webui/` 里放一个 `icon.svg`）
11. 每份 `.html` 零内联脚本（属性边界匹配）（变异：塞一个 `<script>alert(1)</script>`）
12. 嵌入总字节 ≤ 2 MiB（变异：把上限改成 0，确认它真的在比）

**B 组 · HTML/CSS 结构（9 格）**
13. `index.html` 不含 `<base`，且每个 `href`/`src` 要么 `/admin/` 开头、要么 `data:`、要么绝对 URL（§7.4）
14. `index.html` 里的 `data-section` 集合 **恰好** = 九个规定值（变异：删一个 / 多一个 / 改名）
15. 九个 `data-section` 与九份 `sec-*.js`、九组 `nav.*` 字典键**三方双向对齐**（变异：只删两处）
16. CSS 里**零硬编码色值**：除 `:root` 与 `[data-theme=dark]` 两个块、以及登记在案的白名单（`#7c3aed` 紫、`#ff7b72` 等日志级别色、`#fff`、`#0b1f17`）外，不许出现 `#` 开头的颜色（变异：往某条规则里写 `color:#333`）
17. 每个被 `var(--x)` 引用的 token 都在亮色 `:root` 里定义过（变异：引一个 `var(--radius)`；agnes 记过这个坑——引不存在的名字是**静默直角**）
18. **token 层**：`--primary-strong` 与 `--text-secondary`（深色）的对比度现算 ≥ 4.5:1，**报文里带算出来的比值**（变异：改回 `#059669` / `#94a3b8`，确认报文打出 3.77 / 4.04）；
    并且**每一档语义正文色 × 三张主题面 × 它自己那层 tint**（`--info-text`/`--info-bg`、`--success-text`/`--success-bg`、`--danger-text`/`--danger-bg`、`--warning-text`/`--warning-15`、`--primary-strong`/`--primary-10`）两个主题都过 AA（§2.4.1）。
    ⚠️ **P-3 / P-4 那两条具名断言不许并进循环**：它们的报文里带着 kiro 原值的实算数 = 这两条裁定**为什么存在**的取证，循环打不出来
    - **（B-18b，本组的第 9 格）规则层**（P-3e / §2.4.2）：逐条 CSS 规则现算「这条规则画出来的字在**它自己的底**上读不读得清」——
      前景取 `color: var(--x)` 在两套主题块里各解析一次，背景取规则自己声明的 `background`、没声明就落到卡片面 `--bg-primary`（半透明 tint 先合成再算），门槛 4.5:1、**只有真够得上大文本才降到 3.0**；
      两张登记表（`.badge-off` 等豁免、日志控制台那条非继承底）各带**过期自检**（变异：把 `--primary` 塞回某条正文规则，确认那格红）
19. `.nav-item.active` / `.seg-btn.is-active` / `.login-tab.active` 各至少有一条**非颜色**声明（变异：删掉 `font-weight`）
20. 五档断点（1200/1024/900/768/640）各至少命中一条规则（变异：删掉 ≤900 那一档）

**C 组 · i18n（8 格，见 §6.3 表）**

**D 组 · 视图层上移（约 200 格，随 D8 逐函数走）**
—— 判据形态由各自的语义定，但**三条纪律统一**：
① 钉行为不钉实现（例如「读失败时那一格是 `unknown` 而不是 `0`」，不是「函数返回了某个结构体」）；
② 每一族至少有一格**反向控制**（把被测行为改坏，确认那格真的红）；
③ 🔴 **「五态互斥」那一族必须原样搬过来**（agnes `tests/ui/usage.test.ts` 的
「五种状态互不重叠 —— 『没开』『读不出来』『还没落盘』『真的是 0』『有数据』揉在一起就是撒谎」）。

### 8.3 人工冒烟（判据覆盖不到、但每轮必做）

1. 五语言 × 两主题 = **10 格**逐格截图，比对：无裸 `{占位符}`、无未翻译泄漏、无溢出、无空方块图标；
2. 视口 1920 / 1440 / 1280 / 1100 / 900 / 768 / 640 / 380 **八档**，确认：整页无横向滚动条、侧栏在 ≤900 变抽屉；
3. 键盘全流程：Tab 能走完登录闸 → 顶栏 → 侧栏 → 板块，弹窗打开后 Tab 不出去、Escape 能关；
4. 断网 / 接口 500 / 401 三种故障各走一遍，确认：**没有任何一格伪造 0**，`—` 与 `–` 显示正确。

---

## 9. 风险与要主线裁的问题

### R-1 🔴 「完全对标」与 WCAG AA 的冲突（阻塞 D3）

kiro 的配色有两处实算不过 AA（§2.4：白字/`#059669` = 3.77:1；`#94a3b8`/`#334155` = 4.04:1，后者正是 `.data-table th`）。
- **选项 A**：逐字照抄，接受不达标。忠于「完全对标」，但要删掉 agnes 那两格判据。
- **选项 B（本文推荐，已按此写进 P-3/P-4）**：`--primary` 保持 `#059669` 只做装饰；
  新增 `--primary-strong: #047857` 用于实心按钮底与主色文字；深色 `--text-secondary` → `#aab8c9`。
  并排看几乎分不出，两处都过 AA。
**要主线一句话确认走哪个。** 默认按 B 执行。

🔴 **偏离面已经从「两格 token」长成了下面这一串，一并在这一条下裁**（施工中查出，逐条现算见 §2.4.1 / §2.4.2）：

| 裁定 | 偏离了什么 | 量 |
|---|---|---|
| P-3 / P-4 | 新增 `--primary-strong`；深色 `--text-secondary` | 2 格 token |
| P-3b / P-3c | 亮色 `--success-text` `#059669`→`#047857`、`--danger-text` `#dc2626`→`#b91c1c` | 2 格 token |
| P-3d | **新增 kiro 没有的 `--warning-text`**（亮 `#92400e` / 暗 `#fbbf24`） | 1 个新 token |
| P-3e | 深色 `--text-tertiary` `#64748b`→`#94a3b8`；约 110 处正文字色改指 `*-text` / `--primary-strong` | 1 格 token + ~110 处规则 |

**选项 A（逐字照抄）如果被选中，要连这一串一起回退** —— 代价是：`.usage-unknown`
（「这个数不可知」那个标记，面板上最该被看见的一个）回到白底 **2.15:1**，
深色下约 50 处三级正文回到 **3.07:1**。**默认仍按 B。**

### R-2 🔴 实时日志的凭据怎么带（阻塞板块 ⑦ 的推送形态）

kiro 用 `?api_key=`（`EventSource` 设不了头）。agnes 有一条成文的相反纪律并有判据钉着
（`gw-api.js` 文件头：「口令进 URL 会落进浏览器历史、Referer、CF 访问日志、反代日志」）。
- **选项 A**：照抄 `?api_key=`。**代价：当面推翻 agnes 自己钉过的一条纪律**，且与 fail-closed / 24 位口令下限那套姿态不一致。
- **选项 B（本文推荐）**：`POST /admin/api/logs/ticket` 换一张**短时效一次性票**（60s，一次性消费），
  SSE 用 `?ticket=`。票进日志无所谓（它过期即废、且只能用一次）。
- **选项 C（P2b 先落）**：仍用轮询（agnes 现状），UI 预留三态。
**默认按 C 交付 P2b，B 排到 P2b 之后。**

### R-3 🔴 C 档（浏览器交互状态机）判据归零（§5.4 第 4 点）

上移 A 档之后，留在 JS 的三类（选择集合、轮询退避、SSE 分帧缓冲）**没有执行器 ⇒ 零判据**。
今天在 agnes 侧它们有约 200 格。
- **选项 A（本文默认）**：如实登记为欠账，人工冒烟兜底。
- **选项 B**：Rust 仓引入 node + vitest 作为第五道门禁。代价：prepush 多一个工具链、CI 多一步、Docker 多一层。
**要主线裁。**

### R-4 🟡 仪表盘第 4 张卡的内容（阻塞板块 ①）

kiro 那张是「全局剩余积分」。05 §9.1 登记着「Agnes 上游到底有没有查余额接口，全仓零证据」。
本文的处置是**不画那张卡**、第五张放「可用模型数」。
⚠️ 如果主线后来确认上游有这条接口，卡位要重排（五张卡是**固定 5 列栅格**，不是 auto-fit）。

### R-5 🟡 逐条用量记录的有界性（阻塞板块 ⑥ 的下钻表与图表）

05 §9.4 已登记：改成逐条之后一台跑满的网关一天几万条，而 kiro 侧读不到条数上限/滚动删除的代码。
**这条没定之前，用量板块的「下钻记录表」画不出分页规格**（不知道总量上界 ⇒ 不知道是不是要游标分页）。

### R-6 🟡 `.field-grid`（agnes 那条 40% 下界的网格公式）要不要留

§2.11 的裁定是留（设置页 25 项的密度 kiro 从没面对过）。
⚠️ 代价：**它是本次唯一一条从 agnes 带过来的复杂布局规则**，与 kiro 的 `.settings-grid` 并存
⇒ 面板里同时有两套「两列」的实现。如果主线觉得这是不可接受的二义性，
备选是把 25 项拆成 4–5 张卡、每张卡内单列 —— 那样纯用 kiro 的 `.settings-grid` 就够，代价是设置页变长。

### R-7 🟡 `logo.png` 与 `docs/logo.png` 的对账（U-23）

agnes 有 `check-png.mjs` 逐像素对账（「项目不该在 README 与面板上长着两副面孔」）。
改用独立 `logo.png` 之后，Rust 侧要么**同一个文件**（`webui/logo.png` 就是 `docs/logo.png` 的软链/拷贝 + 一格字节相等判据），
要么保留降采样对账（需要一个 PNG 解码依赖）。**推荐前者**（同尺寸同文件，零解码依赖）。

### R-8 🟢 P2b 的真实体量

面板 JS ~10,000 行 + CSS ~3,700 行 + Rust ~3,250 行 + 约 **230 格**新判据。
**这不是一轮能交的量。** 建议的内部切分（不改本文任何裁定）：
- **P2b-1**：D1 + D2 + D3 + D4 + D5（静态树 + 壳层 + 全套 CSS）+ A/B/C 三组共 28 格 —— 交付物是「一个能登录、能切板块、九个板块全是空壳但样式已就位」的面板；
- **P2b-2**：D8 的九份视图层 + 对应端点（Rust 侧，约 200 格）；
- **P2b-3**：D6 + D7（九个板块的 DOM 拼装）+ 人工冒烟四项。

---

## 附录 A · 本文引用的源文件清单

| 仓 | 路径 | 用到它的节 |
|---|---|---|
| kiro2api | `admin-ui-v2/index.html` | §1.1 §1.2 §2.6 §2.7 §3.1 §7.4 |
| kiro2api | `admin-ui-v2/css/base.css` | §2.2–§2.10 |
| kiro2api | `admin-ui-v2/css/sections{,-manage,-stats,-tools}.css` | §2.8 §2.9 §2.10 §2.12 |
| kiro2api | `admin-ui-v2/js/{boot,theme,i18n,i18n-dict,api,app}.js` | §1.2 §2.3 §6.1 |
| kiro2api | `admin-ui-v2/js/sec-*.js` × 8 | §3.2 |
| kiro2api | `admin-ui-v2/tests/{balance-fanout,status-filter,ui-regressions}.test.mjs` | §1.1 §3.2① §3.2② §4 U-4 §5.1 |
| kiro2api | `src/webui/mod.rs` | §7.1 §7.3 §7.5 |
| agnes2api | `admin-ui/index.html` / `README.md` | §1.1 §1.2 §3.1 §7.4 |
| agnes2api | `admin-ui/css/{base,shell,sections}.css` | §2.4 §2.9 §2.11 |
| agnes2api | `admin-ui/js/{boot,theme,i18n,i18n-dict,ui,api,gw-api,health,app}.js` | §1.2 §4 §6.1 §7.4 |
| agnes2api | `admin-ui/js/sec-*.js` × 9 | §3.2 |
| agnes2api | `admin-ui/js/pure/*.mjs` × 18 | §5 |
| agnes2api | `scripts/{build-ui,check-i18n,check-png}.mjs`、`src/ui/serve.ts` | §6.2 §7.2 |
| agnes2api | `tests/ui/**`（46 文件 / 1,169 格） | §1.1 §5.1 §8.2 |
| agnes2api-rs | `docs/porting/{00,02,05}` | 全文 |
| agnes2api-rs | `src/server/app.rs` | §7.5 |
