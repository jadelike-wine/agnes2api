/**
 * 公共 UI 原语。**kiro2api 没有这一层**，后果是它那边 `openModal` 有 3 份、
 * `confirmModal` 3 份、`svgIcon` 3 份**互不相同**的实现，散在各个 `sec-*.js` 里
 * —— 那是全局命名空间下没有强制约束的必然结果。
 * 裁定 P-8：**保留这一层**（用户那条规则管的是"样式和 UI"，内部分层不在其中），
 * 但**外观整套换成 kiro 的皮**（`.toast` / `.modal-overlay` / `.modal-header` …）。
 *
 * 两条纪律贯穿本文件：
 * ① **一切来自接口的内容一律 `textContent`，永不 `innerHTML`。** 上游返回的内容会
 *    出现在日志与错误字段里。
 * ② **SVG 用 `createElementNS` 构造，不用 `innerHTML`**（CSP 的 `script-src 'self'`
 *    要求零内联脚本，而 `innerHTML` 拼 SVG 是这条约束最容易被顺手破坏的地方）。
 */
import { t } from "./i18n.js";

const SVG_NS = "http://www.w3.org/2000/svg";

/** 建元素。`text` 走 textContent；`attrs` 里的键直接 setAttribute。 */
export function el(tag, attrs, text) {
  const node = document.createElement(tag);
  if (attrs) for (const [k, v] of Object.entries(attrs)) {
    if (v === null || v === undefined || v === false) continue;
    node.setAttribute(k, v === true ? "" : String(v));
  }
  if (text !== undefined && text !== null) node.textContent = String(text);
  return node;
}

/** 建一个带 `data-i18n` 的元素：文案由 `apply()` 统一刷，切语言时不必板块自己管。 */
export function elI18n(tag, key, attrs) {
  const node = el(tag, { ...(attrs || {}), "data-i18n": key });
  node.textContent = t(key);
  return node;
}

/**
 * 内联 SVG 图标。`d` 是 path 的 `d` 属性。
 *
 * ⚠️ **`stroke-linecap` / `stroke-linejoin` 这两行别删。** 缺省值是 `butt` / `miter`，
 * 而本仓的图标是「细线 + 很短的划」那一路：太阳那八根射线每根只有约 2px 长，
 * 配 2px 的描边，齐头端点会把每一根渲染成一个 2×2 的**方块**（斜的那四根成菱形），
 * 观感是一圈小方块围着一个圆环，不是圆头短划的太阳；月亮的两个尖角同理被切平。
 * **设在这里而不是设在某几枚图标上**：本文件是全站唯一造 SVG 的地方，
 * 逐枚去设等于把同一个决定抄成好几份，其中一份迟早漏掉。
 */
export function svgIcon(d, size) {
  const svg = document.createElementNS(SVG_NS, "svg");
  svg.setAttribute("viewBox", "0 0 24 24");
  svg.setAttribute("width", String(size || 16));
  svg.setAttribute("height", String(size || 16));
  svg.setAttribute("fill", "none");
  svg.setAttribute("stroke", "currentColor");
  svg.setAttribute("stroke-width", "2");
  svg.setAttribute("stroke-linecap", "round");
  svg.setAttribute("stroke-linejoin", "round");
  svg.setAttribute("aria-hidden", "true");
  const path = document.createElementNS(SVG_NS, "path");
  path.setAttribute("d", d);
  svg.appendChild(path);
  return svg;
}

/** 一颗图标按钮（`.acct-icon-btn` 的行内动作簇用它）。 */
export function iconBtn(d, i18nTitleKey, onClick, extraClass) {
  const b = el("button", {
    class: `acct-icon-btn${extraClass ? ` ${extraClass}` : ""}`,
    type: "button",
    "data-i18n-title": i18nTitleKey,
  });
  b.setAttribute("title", t(i18nTitleKey));
  b.setAttribute("aria-label", t(i18nTitleKey));
  b.appendChild(svgIcon(d));
  b.addEventListener("click", onClick);
  return b;
}

/** `kind` → kiro 那套 toast 变体类名。两套拼写 CSS 都认，这里只用一套。 */
const TOAST_CLASS = { ok: "success", err: "error", warn: "warning", info: "info" };

/**
 * toast。`kind` ∈ {"ok","warn","err","info"}。**文案一律 textContent**。
 *
 * 默认 **3.2 秒**自动消失（kiro 的取值）。
 *
 * `opts.sticky: true` 时**不自动消失**，改成挂一颗「×」按钮手动关闭。
 * kiro 没有这一档（U-7 保留 agnes 的）。**只给"运维必须看见、看漏了会造成误判"
 * 的那类信息用** —— 例如批量操作里有一部分被后端拒绝：那类信息藏在 HTTP 200 的
 * 响应体里，3 秒一闪而过等于把一条诚实信号做成了几乎看不见的信号。
 * **其余提示保持自动消失**，不然每一次「已复制」都要用户自己去点掉。
 *
 * ⚠️⚠️ **sticky 出现时关闭按钮直接拿到焦点**，且它自己身上挂了 Escape。
 * 理由：`#toastContainer` 是 `<body>` 最后一个元素，一次 20 行批量删除之后表格里
 * 可能有上百颗按钮，键盘用户要把它们全部 Tab 过去才够得到那颗"必须被看见、必须被
 * 点掉"的关闭按钮 —— 把最要紧的信号放在整页最后一个 Tab 停靠点，等于对键盘用户
 * 取消了 sticky 这个决定本身带来的收益。
 */
export function toast(message, kind, opts) {
  const host = document.getElementById("toastContainer");
  if (!host) return;
  const sticky = !!(opts && opts.sticky);
  const variant = TOAST_CLASS[kind] || TOAST_CLASS.ok;
  const node = el("div", { class: `toast ${variant}${sticky ? " toast-sticky" : ""}`, role: "status" });
  node.appendChild(el("span", null, message));
  if (sticky) {
    const close = el("button", { type: "button", class: "toast-close", "aria-label": t("common.dismiss") });
    close.textContent = "×";
    const dismiss = () => node.remove();
    close.addEventListener("click", dismiss);
    close.addEventListener("keydown", (ev) => { if (ev.key === "Escape") dismiss(); });
    node.appendChild(close);
    host.appendChild(node);
    // `.show` 要在**下一帧**加，否则与初始 transform 同帧生效 ⇒ 没有滑入动画。
    requestAnimationFrame(() => node.classList.add("show"));
    close.focus();
  } else {
    host.appendChild(node);
    requestAnimationFrame(() => node.classList.add("show"));
    setTimeout(() => node.remove(), 3200);
  }
}

/** 弹窗容器 `aria-labelledby` 用的 id 前缀，逐个自增，不许两个同时打开的弹窗撞号。 */
let modalSeq = 0;

/**
 * 一棵子树里"可以拿到键盘焦点"的元素，按 DOM 顺序。焦点陷阱与初始焦点都靠它。
 *
 * ⚠️⚠️ **只用 `.children` + `.tagName` 手写递归，不要用任何测试夹具发明的遍历方法。**
 * agnes 那边第一版写的是 `root.walk()` —— 那是 DOM 垫片自己实现的方法，真实
 * `HTMLElement` 上**根本没有**：全套 DOM 测试绿，**真实浏览器里一按 Tab 就抛
 * `TypeError: root.walk is not a function`，焦点陷阱整个不生效**。
 * `tagName` 在真实浏览器上恒为大写（`"BUTTON"`），这里统一转小写再比。
 */
function focusableIn(root) {
  const FOCUSABLE_TAGS = new Set(["button", "input", "textarea", "select", "a"]);
  const out = [];
  const visit = (node) => {
    const tag = String(node.tagName || "").toLowerCase();
    if (FOCUSABLE_TAGS.has(tag) && !node.disabled) out.push(node);
    for (const child of node.children) visit(child);
  };
  visit(root);
  return out;
}

/**
 * 通用弹窗。**外观是 kiro 的**（`.modal-overlay` / `.modal` / `.modal-header` /
 * `.modal-body` / `.modal-footer` / `.modal-close`），**行为是 agnes 的**。
 *
 * 🔴 **kiro 的 modal 四条焦点管理一条都没有**（无初始焦点、无焦点陷阱、无 Escape、
 * `<h3>` 与容器之间没有 `aria-labelledby`）。它现在守着删除 / 批量操作这类破坏性
 * 动作 —— 键盘 / 读屏用户可以在「确认删除 20 把」弹窗还开着的时候 Tab 出去操作
 * 背后被弹窗盖住的表格。四条全在这里：
 *   ① 打开时焦点落在弹窗容器本身（`tabindex="-1"` + `.focus()`）—— **不猜"第一个
 *      输入框"**，因为确认类弹窗里可能只有两颗按钮；
 *   ② Tab / Shift+Tab 只在弹窗内部的可聚焦元素之间循环；
 *   ③ Escape 直接关闭（等同取消，不跑任何 `onClick`）；
 *   ④ `<h3>` 与 `role="dialog"` 的容器之间用 `aria-labelledby` 关联。
 *
 * **`actions[i].keepOpen: true`**：点击后不自动关闭，`onClick` 拿到 `close` 自己决定
 * 什么时候关。这条修的是一个真缺陷 —— `onClick` 原来无条件先关再跑，导入弹窗校验
 * 失败/网络失败时弹窗已经关了，粘进去的 200 行只剩一句 toast，其余全丢。
 */
export function openModal(titleKey, bodyNode, actions, opts) {
  const root = document.getElementById("modalRoot") || document.body;
  const back = el("div", { class: "modal-overlay" });
  const box = el("div", {
    class: `modal${opts && opts.large ? " modal-lg" : ""}`,
    role: "dialog",
    "aria-modal": "true",
    tabindex: "-1",
  });
  const titleId = `modal-title-${++modalSeq}`;
  box.setAttribute("aria-labelledby", titleId);

  const close = () => back.remove();

  const head = el("div", { class: "modal-header" });
  head.appendChild(elI18n("h3", titleKey, { id: titleId }));
  const x = el("button", { class: "modal-close", type: "button", "aria-label": t("common.close") });
  x.textContent = "×";
  x.addEventListener("click", close);
  head.appendChild(x);
  box.appendChild(head);

  const body = el("div", { class: "modal-body" });
  body.appendChild(bodyNode);
  box.appendChild(body);

  const bar = el("div", { class: "modal-footer" });
  for (const a of actions) {
    const b = elI18n("button", a.labelKey, {
      type: "button",
      class: `btn ${a.danger ? "btn-danger" : "btn-outline"}`,
    });
    b.addEventListener("click", () => {
      const result = a.onClick && a.onClick(close);
      if (!a.keepOpen && result !== false) close();
    });
    bar.appendChild(b);
  }
  box.appendChild(bar);

  box.addEventListener("keydown", (ev) => {
    if (ev.key === "Escape") { close(); return; }
    if (ev.key !== "Tab") return;
    const items = focusableIn(box);
    if (items.length === 0) return;
    const first = items[0];
    const last = items[items.length - 1];
    const current = document.activeElement;
    if (ev.shiftKey) {
      if (current === first || !items.includes(current)) { ev.preventDefault(); last.focus(); }
    } else if (current === last || !items.includes(current)) {
      ev.preventDefault();
      first.focus();
    }
  });

  back.appendChild(box);
  root.appendChild(back);
  // `.active` 在下一帧加：`.modal-overlay` 的 `opacity` 过渡与 `display` 同帧生效
  // 时不会动画。
  requestAnimationFrame(() => back.classList.add("active"));
  box.focus();
  return close;
}

export function confirmModal(titleKey, messageKey, onConfirm) {
  return openModal(titleKey, elI18n("p", messageKey), [
    { labelKey: "common.cancel" },
    { labelKey: "common.confirm", danger: true, onClick: onConfirm },
  ]);
}

export async function copy(text) {
  try {
    await navigator.clipboard.writeText(text);
    toast(t("common.copied"), "ok");
  } catch (e) {
    // 剪贴板在非 TLS 下不可用是常态，**不要**假装成功。
    toast(t("common.copyFailed"), "warn");
  }
}

/**
 * 一次「取明文」的失败该说成哪一档。**刻意不做成注入项**，理由是硬的：
 * 注入项**漏传一个不会有任何编译期或运行期信号** —— 它只会在下面那个 `catch` 里抛
 * `TypeError: … is not a function`，而那一抛落回的正是「无人接的 promise 拒绝 +
 * 屏幕上什么都不变」，也就是这个 `catch` 本身要修的那个缺陷。
 * 别的注入项漏传会当场炸在渲染主路径上（一眼就能看见），这一个不会。
 *
 * 四态各说各的话，**不并成一句「取不到」**：
 *   · `gone`        404 —— 这一行在别的标签页被删掉了（轮询表上的常态路径）；
 *   · `unavailable` 403 / 501 —— 这个部署关掉了明文回显；
 *   · `expired`     401 —— 会话失效（`api.js` 已经把人送回登录闸了）；
 *   · `failed`      其余（含网络失败）。
 */
function revealErrorKind(e) {
  const status = e && typeof e.status === "number" ? e.status : 0;
  if (status === 404) return "gone";
  if (status === 403 || status === 501) return "unavailable";
  if (status === 401) return "expired";
  return "failed";
}

/**
 * 「掩码 + 显示明文 + 复制」这一组控件。**账号池与 API 密钥共用同一份实现。**
 *
 * 🔴 **明文不写回列表数据**：它只活在注入的 `state` 里，而列表随轮询整份重建
 * ⇒ 切板块、刷新之后自动回到掩码态。把明文塞进列表数据会让它跟着进下一次渲染、
 * 进任何对列表做的序列化。
 *
 * ⚠️ **复制那颗按钮在未显示时也能用**：它会先取一次明文再写剪贴板 —— 运维要的是
 * 「把密钥贴到别处」，不是「先让它出现在屏幕上」。少一次「明文上屏」反而更安全
 *（肩窥、录屏、截图）。
 *
 * 参数：`{ id, masked, state, fetchSecret, messageKeyOf }`。
 * `fetchSecret(id)` 由调用方注入（两族的端点不同），**成功时直接返回明文字符串**
 *（"读成功但这个部署不给明文"那一档由端点自己用非 2xx 表达，见上面那张表）。
 */
export function revealControls(o) {
  const wrap = el("span", { class: "reveal" });
  const text = el("code", { class: "reveal-text" }, o.masked);
  wrap.appendChild(text);

  const render = () => {
    const s = o.state.secretOf(o.id);
    const shown = o.state.isShown(o.id);
    text.textContent = shown && s !== null ? s : o.masked;
    if (shown) text.setAttribute("data-revealed", "");
    else text.removeAttribute("data-revealed");
    // 🔴 **`data-i18n` 必须跟着一起写**，不能只写 `textContent`。
    // 这颗按钮的标签是**动态的**（在「显示明文 / 隐藏」之间切换）⇒ 建的时候挂不了静态键。
    // 而 `js/i18n.js` 的 `apply()` 只重译带 `data-i18n` 的元素：只写 textContent 的话，
    // 切语言时这颗按钮会**留在旧语言**，唯一兜住它的是 `js/app.js` 的 `langchange`
    // 处理器紧接着还调了 `currentSection.onShow()` 整块重渲。
    // ⚠️ **那是一条没有判据钉着的隐式依赖** —— 哪天有人把 `langchange` 收成「只 `apply(document)`」
    //（看起来是个合理的优化），这颗按钮就会静默留在旧语言。同步写上之后两条机制各自独立成立。
    const key = shown ? "reveal.hide" : "reveal.show";
    eye.setAttribute("data-i18n", key);
    eye.textContent = t(key);
  };

  const load = async () => {
    const had = o.state.secretOf(o.id);
    if (had !== null) return had;
    try {
      const secret = await o.fetchSecret(o.id);
      o.state.remember(o.id, secret);
      return secret;
    } catch (e) {
      // 🔴 **这个 catch 是这两颗按钮唯一的反馈来源，别把它「顺手」删回去。**
      // 没有它时：`js/api.js` 的 `json()` 对任何非 2xx 一律抛，异常穿过本函数落在
      // 下面两个 `async` 监听器上**无人接**（全站零处 `unhandledrejection`）
      // ⇒ 点「显示明文」/「复制」屏幕上一个字都不变、剪贴板里也什么都没有，
      // 运维只会以为自己没点上、反复点。
      toast(t(o.messageKeyOf(revealErrorKind(e))), "warn", { sticky: true });
      return null;
    }
  };

  // `data-i18n` 的初值与 `render()` 里那一处必须是同一个键（见 `render()` 上方那段）。
  const eye = el(
    "button",
    { type: "button", class: "acct-icon-btn is-mini", "data-i18n": "reveal.show" },
    t("reveal.show"),
  );
  eye.addEventListener("click", async () => {
    if (o.state.isShown(o.id)) { o.state.hide(o.id); render(); return; }
    const s = await load();
    if (s === null) return;
    o.state.show(o.id);
    render();
  });
  wrap.appendChild(eye);

  const cp = elI18n("button", "common.copy", { type: "button", class: "acct-icon-btn is-mini" });
  cp.addEventListener("click", async () => {
    const s = await load();
    if (s === null) return;
    await copy(s);
  });
  wrap.appendChild(cp);

  render();
  return wrap;
}
