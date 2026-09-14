/**
 * 面板入口：登录闸 + 板块路由 + 顶栏接线。**全站唯一的模块脚本入口。**
 *
 * 板块契约：`js/sec-<name>.js` 导出一个 `{ init?, onShow?, onHide? }`，
 * 导出名可以是 `default`，也可以是 `<name>Section`（两种都认，见 `loadSection`）。
 * `showSection()` 负责：切 `.active`、首次 `init(node)`、每次 `onShow()`、
 * 离开时上一个板块 `onHide()`（**停轮询、作废在飞请求**）、把板块名写 localStorage。
 * **没有 URL 路由、没有 hash**（放弃深链接，换与 kiro2api 一致的刷新复原行为）。
 *
 * 🔴 **九个板块用 `import()` 按需加载，而不是九条顶层 `import`。** 三条理由：
 *   ① 一个板块的语法错误不会把整个壳层（连同登录闸）一起带塌 —— 顶层 import 会；
 *   ② P2b 分期交付：板块文件是后面几期才落地的，静态 import 一个还不存在的文件
 *      会让**整张面板白屏**，而按需加载只会让那一个板块显示一句「还没接线」；
 *   ③ 加一个板块不必改 `index.html`（kiro 那边每加一个板块要多一条 `<script>`）。
 * ⚠️ 代价如实写：首次进入某板块多一次网络往返（同源、带 ETag、通常是 304）。
 *
 * ⚠️ **板块内不许各自监听 `langchange`**：框架层在这里统一 `apply(document)` +
 * 重跑当前板块的 `onShow()`。板块自己再监听一次就是 kiro2api 那 4 处冗余监听。
 */
import { t, apply, setLang, currentLang, LANGS, LANG_LABELS } from "./i18n.js";
import { getTheme, toggleTheme, setTheme } from "./theme.js";
import { svgIcon, el } from "./ui.js";
import { onUnauthorized } from "./api.js";
import { sessionExpired } from "./pure/session.mjs";
import { sendable } from "./pure/sendable.mjs";
// **键名不在这里声明**：写入方是这个文件、读取方在 `api.js`，各写一份的后果实测过
// —— 只改这边一处 ⇒ 登录成功进壳层、随后每请求送空口令头 ⇒ 401 ⇒ 登出循环，
// 面板彻底不可用而全套用例照绿。理由全文见那个模块的文件头。
import { KEY_STORE, SAVED_AT_STORE, SECTION_STORE, GW_KEY_STORE } from "./pure/storage-keys.mjs";

/**
 * 九个板块。🔴 **这张表、`index.html` 的 `data-section` 集合、i18n 字典里的
 * `nav.*` 键三方必须逐一对齐**，由 `src/webui/mod.rs` 的
 * `nine_sections_line_up_three_ways` 钉着。
 * ⚠️ 名字照 kiro（裁定 P-6）：agnes 的 `overview`/`keys`/`events` 三个名字退场。
 */
const SECTION_NAMES = [
  "dashboard", "accounts", "models", "apikeys",
  "playground", "usage", "logs", "registrar", "settings",
];
const DEFAULT_SECTION = "dashboard";

const loaded = new Map();

const overlay = document.getElementById("loginOverlay");
const form = document.getElementById("loginForm");
const input = document.getElementById("loginKeyInput");
const errBox = document.getElementById("loginError");

function showLoginError(message) {
  errBox.textContent = message || "";
  errBox.classList.toggle("show", !!message);
}

function store(op, value) {
  try {
    if (op === "get") return localStorage.getItem(KEY_STORE);
    if (op === "getAt") return localStorage.getItem(SAVED_AT_STORE);
    if (op === "set") {
      localStorage.setItem(KEY_STORE, value);
      localStorage.setItem(SAVED_AT_STORE, String(Date.now()));
    }
    if (op === "del") {
      localStorage.removeItem(KEY_STORE);
      localStorage.removeItem(SAVED_AT_STORE);
      // ⚠️ **网关口令一并清**：它是这块存储里的第二把凭据，而它比管理口令还弱
      //    （没有年龄上限、后端没有撤销路径）。理由全文见 `pure/storage-keys.mjs`。
      localStorage.removeItem(GW_KEY_STORE);
    }
  } catch (e) { /* 隐私模式：本次会话照常可用，刷新后要重新输入 */ }
  return null;
}

/**
 * 取一个板块模块。未接线 / 加载失败时**说出来**，不静默留一张白板。
 * 🔴 **失败不缓存成 `null` 之外的东西**：下次再点还会重试一次，而不是永久地把
 * 一次偶发的网络失败钉成"这个板块不存在"。
 */
async function loadSection(name) {
  if (loaded.has(name)) return loaded.get(name);
  let mod = null;
  try {
    mod = await import(`/admin/js/sec-${name}.js`);
  } catch (e) {
    loaded.set(name, null);
    return null;
  }
  const sec = mod[`${name}Section`] || mod.default || null;
  loaded.set(name, sec);
  return sec;
}

let current = null;
let currentSection = null;

function paintNav(name) {
  for (const btn of document.querySelectorAll(".nav-item")) {
    const on = btn.getAttribute("data-section") === name;
    btn.classList.toggle("active", on);
    btn.setAttribute("aria-current", on ? "page" : "false");
  }
  for (const sec of document.querySelectorAll(".section")) {
    sec.classList.toggle("active", sec.id === `sec-${name}`);
  }
}

async function showSection(name) {
  // 🔴 **不认识的值回落到默认板块，别改这条。** 裁定 P-6 改过板块名
  //（`overview`/`keys`/`events` 退场），老用户 localStorage 里存的旧值在新版里
  // 不再是合法板块名 —— 没有这条兜底，他们刷新后会看到一张九个板块全不 active 的空页。
  if (!SECTION_NAMES.includes(name)) name = DEFAULT_SECTION;
  if (current === name) {
    if (currentSection && currentSection.onShow) currentSection.onShow();
    return;
  }
  if (currentSection && currentSection.onHide) currentSection.onHide();
  paintNav(name);
  current = name;
  currentSection = null;
  try { localStorage.setItem(SECTION_STORE, name); } catch (e) { /* ignore */ }

  const node = document.getElementById(`sec-${name}`);
  const sec = await loadSection(name);
  // 切得快时后一次会赢：加载回来发现用户已经走了就什么都不做。
  if (current !== name) return;
  if (!sec) {
    node.textContent = "";
    node.appendChild(el("div", { class: "empty-state", "data-i18n": "section.notWired" }, t("section.notWired")));
    return;
  }
  if (!sec.__inited) { if (sec.init) sec.init(node); sec.__inited = true; }
  currentSection = sec;
  if (sec.onShow) sec.onShow();
}

function enter() {
  overlay.classList.remove("active");
  let saved = DEFAULT_SECTION;
  try { saved = localStorage.getItem(SECTION_STORE) || DEFAULT_SECTION; } catch (e) { /* ignore */ }
  showSection(saved);
  // 顶栏那颗状态徽章探一次并开始轮询。**在这里而不是在模块顶层**：徽章只在壳层里
  // 看得见，而在登录闸上发请求会让「没有已存口令时一个请求都不发」那条性质不再成立。
  startHealth();
}

/**
 * 🔴 **登出要把九个板块的 DOM 整份拆掉，不只是盖一张登录闸上去。**
 *
 * 没有这一步时的实况（复核方实机验证过）：运维在「API 管理」里点过「显示明文」，
 * 那把 `sk-…` 已经写进了 `<code class="reveal-text">`；随后切到别的板块、再点登出。
 * `leave()` 只跑**当前**板块的 `onHide()`，而两个板块的 `onHide()` 清的都只是 JS
 * 那份 `Map`（`revealState.forget()`）—— **没有任何一处重渲或清空 section 节点**
 * ⇒ 那串明文原样躺在 `#sec-apikeys` 里，`display:none` 只是看不见，
 * F12 一开、或者下一个人登进来切回那个板块（渲染前的一瞬）就在屏幕上。
 * 上游池 key 与我们签发的 `sk-` 两族都走同一条路径。
 * **登出的语义就是「让下一个人看不到」**，这是它唯一的用处。
 *
 * ⚠️ **拆完必须把 `__inited` 一起放倒。** 九个板块里有四个
 *（models / playground / registrar / apikeys）是在 `init()` 里建好骨架、把子节点
 * 存进模块级 `nodes` 的：只清 DOM 不重置这一格的话，它们下次进来会往一棵**已经
 * 脱离文档**的子树上渲染 —— 屏幕上是一张永远空白的板块，而控制台里一个字都没有。
 * ⚠️ 代价如实写：重跑 `init()` 会让 `js/sec-accounts.js` 在 `document` 上那两个
 * 「点别处关菜单」的监听各多挂一份（它没有解绑那一半）。两份都调同一个幂等函数，
 * 且只在「登出→再登录」这条路径上各加一次 —— 与「明文留在 DOM 里」相比，
 * 这是明确更小的那一边。已登记在交付说明里。
 */
function teardownSections() {
  for (const name of SECTION_NAMES) {
    const node = document.getElementById(`sec-${name}`);
    if (node) node.textContent = "";
  }
  for (const sec of loaded.values()) {
    if (sec) sec.__inited = false;
  }
}

function leave(reasonKey) {
  if (currentSection && currentSection.onHide) currentSection.onHide();
  current = null;
  currentSection = null;
  stopHealth();
  store("del");
  teardownSections();
  overlay.classList.add("active");
  input.value = "";
  showLoginError(reasonKey ? t(reasonKey) : "");
}

/* ============================================================
   顶栏状态徽章 —— U-8 的合并裁定
   ============================================================ */

/**
 * 20 秒轮询 `/health`（kiro 的自动化）+ 点一下重新探（agnes 的可点）+
 * **初始文案「状态未知」**（agnes 的诚实：这一刻还没探过）。
 *
 * 🔴 **刻意不走 `js/api.js`**：`/health` 是**不鉴权**端点，走那边会给它带上
 * `x-admin-key` —— 把口令送去一个不需要它的端点，等于凭空多一条外发路径。
 */
const HEALTH_POLL_MS = 20000;
const statusBadge = document.getElementById("serverStatus");
const statusText = statusBadge.querySelector(".status-text");
let healthTimer = null;

function paintHealth(state) {
  for (const c of ["is-unknown", "is-offline", "is-degraded"]) statusBadge.classList.remove(c);
  const key = {
    ok: "header.statusRunning",
    degraded: "header.statusDegraded",
    offline: "header.statusOffline",
    unknown: "header.statusUnknown",
  }[state];
  if (state === "degraded") statusBadge.classList.add("is-degraded");
  else if (state === "offline") statusBadge.classList.add("is-offline");
  else if (state === "unknown") statusBadge.classList.add("is-unknown");
  statusText.setAttribute("data-i18n", key);
  statusText.textContent = t(key);
}

async function refreshHealth() {
  try {
    const res = await fetch("/health", { credentials: "omit" });
    // 🔴🔴 **非 2xx 不许短路掉读响应体这一步。**
    // `src/server/routes/health.rs` 在存储不可写时发的是 **503 + `status:"degraded"`**，
    // 而那个 503 是**故意**的（注释逐字写着：只有非 2xx，镜像内置的 HEALTHCHECK 才会
    // 把容器标成 unhealthy）。上一版这里第一行就是 `if (!res.ok) → offline`
    // ⇒ 唯一一条会产出 `degraded` 的真实路径被整条吃掉，`paintHealth("degraded")`
    //   与 `.is-degraded` 那一档成了**永远走不到的死代码**。
    // 后果不是少一个颜色：典型场景是 Docker 绑定挂载的宿主目录属主与容器内用户不一致
    // ⇒ 运维看到顶栏写着「探测失败」，于是去查网络、查反代、查端口，
    //   而真相是**存储写不进去**，面板本来一句话就能说清。
    let body = null;
    try { body = await res.json(); } catch (e2) { body = null; }
    // `/health` 自己说自己是不是降级的；读不出那一格时**不假称健康**。
    const ok = body && (body.status === "ok" || body.ok === true);
    const degraded = body && (body.status === "degraded" || body.degraded === true);
    // ⚠️ 顺序要紧：**先认它自己说的话**，再落到状态码。一个说自己 degraded 的 503
    //    是「服务在，存储写不进去」，与「探测不到」是两个完全不同的下一步。
    if (degraded) { paintHealth("degraded"); return; }
    // 剩下的非 2xx 才是真的「这一次没探到」（502/504、反代拦截、非 JSON 的错误页…）。
    if (!res.ok) { paintHealth("offline"); return; }
    // 2xx 但那一格读不出来：**服务明明答了话，不许说成「探测失败」** ⇒ 「状态未知」。
    paintHealth(ok ? "ok" : "unknown");
  } catch (e) {
    // 这里才是真的一个字节都没拿到（断网 / DNS / 连接被拒）。
    paintHealth("offline");
  }
}

function startHealth() {
  refreshHealth();
  if (healthTimer === null) healthTimer = setInterval(refreshHealth, HEALTH_POLL_MS);
}

function stopHealth() {
  if (healthTimer !== null) { clearInterval(healthTimer); healthTimer = null; }
  paintHealth("unknown");
}

statusBadge.addEventListener("click", () => { refreshHealth(); });

/* ============================================================
   登录闸
   ============================================================ */

/**
 * 登录探针。**刻意不走 `js/api.js`**，三条理由缺一不可：
 * ① 这一刻还没有会话 —— 时刻键要到 `store("set")` 才写下，`api.js` 的 `expired()`
 *    前置会把每一次登录都判成过期（`Number(null) === 0` ⇒ `savedAt <= 0`）；
 * ② 这里的 401 意思是「口令不对」，不是「你掉线了」，走 `onUnauthorized()` 会在
 *    登录闸上再弹一次登录闸；
 * ③ 口令还没进 `localStorage`，`api.js` 的 `readKey()` 读不到它。
 */
async function probe(key) {
  const res = await fetch("/admin/api/session", {
    headers: { "x-admin-key": key },
    credentials: "omit",
  });
  if (res.status === 401) return { ok: false, reason: "login.invalid" };
  // 403 **不当会话失效**：将来某个操作被拒绝时把人踢出后台并告知「密钥无效」，
  // 是 kiro2api 踩过的坑。这里只有 401 才算登录失败。
  if (!res.ok) return { ok: false, reason: "login.httpError", status: res.status };
  return { ok: true };
}

form.addEventListener("submit", async (e) => {
  e.preventDefault();
  const key = input.value.trim();
  showLoginError("");
  if (!key) { showLoginError(t("login.empty")); return; }
  // 字符集判定在 `js/pure/sendable.mjs`，**与后端逐码位等价**。
  // 前端这一半必须有：后端只在启动时看得到自己的口令，看不到用户粘了什么。
  if (!sendable(key)) { showLoginError(t("login.badShape")); return; }
  const submit = document.getElementById("loginSubmitBtn");
  submit.disabled = true;
  try {
    const r = await probe(key);
    if (!r.ok) {
      // 失败**绝不**写入 localStorage，也不显示「已登录」。
      showLoginError(r.reason === "login.invalid" ? t("login.invalid") : t("login.httpError", { status: r.status }));
      return;
    }
    store("set", key);
    enter();
  } catch (e2) {
    showLoginError(t("login.network"));
  } finally {
    submit.disabled = false;
  }
});

// 会话失效由 api.js 统一上报：任何一个 401 都把人送回登录闸，并说清原因。
onUnauthorized(() => leave("common.sessionExpired"));

/* ============================================================
   顶栏图标 / 主题 / 语言 / 侧栏
   ============================================================ */

/**
 * ⚠️ **`index.html` 里那两颗按钮是空的，图标在这里插进去**：`js/ui.js` 的
 * `svgIcon()` 是全站唯一那份构造 SVG 的实现（`createElementNS`，不走 innerHTML），
 * 在 HTML 里再手抄一份 `<path d="…">` 就是第二份真源。
 * ⚠️ **空按钮本身曾经就是缺陷**：只有 `data-i18n-title`（悬停才有提示），里面一个
 * 字符都没有 ⇒ 屏幕上是两个空方块，而字典里那几条文案一直都在、从没被渲染过。
 *
 * ⚠️⚠️ **主题按钮跟着当前主题换图标：亮色露太阳、深色露月亮。**
 * 画的是**现在处在哪一档**，不是「点下去会变成哪一档」—— 两颗按钮、两个入口，
 * 读法只有这一种。**两枚图标都在按钮里，切换只改 `style.display`，不重建节点。**
 */
const ICON_SUN = "M 12 8 a 4 4 0 1 0 0 8 a 4 4 0 1 0 0 -8 z M 12 2 v 2 M 12 20 v 2 M 4.9 4.9 l 1.4 1.4 M 17.7 17.7 l 1.4 1.4 M 2 12 h 2 M 20 12 h 2 M 4.9 19.1 l 1.4 -1.4 M 17.7 6.3 l 1.4 -1.4";
const ICON_MOON = "M 21 12.8 A 9 9 0 1 1 11.2 3 a 7 7 0 0 0 9.8 9.8 z";
const ICON_LOGOUT = "M 9 21 H 5 a 2 2 0 0 1 -2 -2 V 5 a 2 2 0 0 1 2 -2 h 4 M 16 17 l 5 -5 l -5 -5 M 21 12 H 9";

const themeIcons = [];
function paintThemeIcon(id) {
  const btn = document.getElementById(id);
  if (!btn) return;
  const sun = svgIcon(ICON_SUN, 18);
  sun.setAttribute("class", "icon-sun");
  const moon = svgIcon(ICON_MOON, 18);
  moon.setAttribute("class", "icon-moon");
  btn.appendChild(sun);
  btn.appendChild(moon);
  themeIcons.push({ sun, moon });
}

/** 亮色只露太阳、深色只露月亮。两颗按钮是同一个开关的两个入口，一起刷。 */
function paintThemeState() {
  const dark = getTheme() === "dark";
  for (const pair of themeIcons) {
    pair.sun.style.display = dark ? "none" : "";
    pair.moon.style.display = dark ? "" : "none";
  }
}

paintThemeIcon("themeToggleBtn");
paintThemeIcon("loginThemeBtn");
document.getElementById("logoutBtn").prepend(svgIcon(ICON_LOGOUT, 18));
// `theme.js` 每次切主题都广播这一条 ⇒ 两处入口点哪一颗，两颗都跟着换。
document.addEventListener("themechange", paintThemeState);
paintThemeState();

document.getElementById("themeToggleBtn").addEventListener("click", () => toggleTheme());
// 登录闸上那颗：**登录之前也得切得动主题**，顶栏那颗要登录之后才够得着。
document.getElementById("loginThemeBtn").addEventListener("click", () => toggleTheme());
document.getElementById("logoutBtn").addEventListener("click", () => leave(null));

/* 语言：自绘下拉 + 母语名（U-14 对标 kiro）。 */
const langBtn = document.getElementById("langBtn");
const langMenu = document.getElementById("langMenu");
const langLabel = langBtn.querySelector(".lang-label");

function paintLangMenu() {
  langMenu.textContent = "";
  for (const code of LANGS) {
    const b = el("button", { type: "button", class: code === currentLang() ? "active" : null }, LANG_LABELS[code]);
    b.addEventListener("click", () => { setLang(code); closeLangMenu(); });
    langMenu.appendChild(b);
  }
  langLabel.textContent = LANG_LABELS[currentLang()];
}

function closeLangMenu() {
  langMenu.classList.remove("open");
  langBtn.setAttribute("aria-expanded", "false");
}

langBtn.addEventListener("click", (ev) => {
  ev.stopPropagation();
  const open = langMenu.classList.toggle("open");
  langBtn.setAttribute("aria-expanded", open ? "true" : "false");
});
document.addEventListener("click", closeLangMenu);
document.addEventListener("keydown", (ev) => { if (ev.key === "Escape") closeLangMenu(); });
paintLangMenu();

/* 移动端侧栏抽屉（≤900px）。 */
const sidebar = document.getElementById("sidebar");
const backdrop = document.getElementById("sidebarBackdrop");
function closeSidebar() {
  sidebar.classList.remove("open");
  backdrop.classList.remove("show");
}
document.getElementById("sidebarToggle").addEventListener("click", () => {
  const open = sidebar.classList.toggle("open");
  backdrop.classList.toggle("show", open);
});
backdrop.addEventListener("click", closeSidebar);

for (const btn of document.querySelectorAll(".nav-item")) {
  btn.addEventListener("click", () => {
    showSection(btn.getAttribute("data-section"));
    closeSidebar();
  });
}

// **框架层的兜底：切语言时整页重绘。** 板块自己不许再监听这个事件。
document.addEventListener("langchange", () => {
  apply(document);
  paintLangMenu();
  if (currentSection && currentSection.onShow) currentSection.onShow();
});

apply(document);
setTheme(getTheme());

/* ============================================================
   进站
   ============================================================ */

// 已经存过口令就直接验一次；**验不过要清掉**，否则用户会卡在一个永远进不去的页面。
// 只有 401 才清：接口 500 或断网时清掉等于让一次运维事故顺手把人锁在外面。
//
// 但在验之前先过一道**会话绝对上限**：存下来超过 12 小时（或者压根没存时刻 ——
// 旧版本存的）就当场清掉、回登录闸，连这一次 probe 都不发。
//
// 🔴 **登录闸在 `index.html` 里就是可见的**（`class="login-overlay active"`），
// 这里只负责在验过之后把它摘掉 —— 不是「验不过再显示」。方向反过来就是 fail open。
const saved = store("get");
if (saved && !sessionExpired(Number(store("getAt")), Date.now())) {
  probe(saved)
    .then((r) => { if (r.ok) enter(); else if (r.reason === "login.invalid") store("del"); })
    .catch(() => { /* 断网 / 接口 500：**不清口令**，闸照旧开着，用户重试即可 */ });
} else if (saved) {
  store("del");
  showLoginError(t("common.sessionExpired"));
}
