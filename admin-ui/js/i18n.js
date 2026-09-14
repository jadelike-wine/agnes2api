/**
 * i18n。三个标记钩子 + 一个兜底机制。
 *
 * `t()` 找不到 key 时**返回 key 本身**（生产不该因为缺一句翻译就白屏），
 * 但 `DEBUG_STORE` 这个存储键为真时在控制台告警 —— 开发期看得见，生产期不打扰。
 * ⚠️ 这条兜底**不是**「缺翻译没关系」的许可证：字典的齐全性、占位符一致性、
 * 「未引用的 key 是硬错」等 8 格门禁在 `src/webui/mod.rs` 里，`cargo test` 会跑。
 *
 * **兜底机制（照抄 kiro2api，理由相同）**：切语言时框架层做两件事 ——
 * `apply(document)` **加上**重新调用当前板块的 `onShow()` 整页重绘。
 * 有些标签是渲染时用 `t()` 拼进 textContent 的、没有 `data-i18n` 属性，`apply()` 抓不到。
 * 框架层强制重跑保证**不管板块自己写得对不对，切语言都一定生效**。
 * ⇒ 🔴 **板块内不许再各自监听 `langchange`**（kiro2api 有 4 处冗余监听，是历史遗留）。
 */
import { I18N } from "./i18n-dict.js";
// 键名的单一真源，见该模块文件头。
import { LANG_STORE as STORE, DEBUG_STORE } from "./pure/storage-keys.mjs";

/** 五种语言。顺序即语言菜单的顺序。 */
export const LANGS = ["zh-CN", "zh-TW", "en", "ja", "ko"];

/**
 * 语言菜单里显示的**母语名**（U-14 对标 kiro）。
 * agnes 原来那个原生 `<select>` 的选项是语言**代码**（`zh-CN` / `ja`），对用户不可读。
 * ⚠️ **这些字符串刻意不进 i18n 字典**：一个看不懂当前界面语言的人，要靠"日本語"
 * 这四个字找到自己那一项 —— 把它翻译掉恰好毁掉它唯一的用处。
 */
export const LANG_LABELS = {
  "zh-CN": "简体中文",
  "zh-TW": "繁體中文",
  en: "English",
  ja: "日本語",
  ko: "한국어",
};

/**
 * 兜底链：`lang` → `zh-CN` → key 本身。
 * ⚠️ kiro 的兜底语言是 `en`，本仓是 `zh-CN`（与 agnes 一致）：本项目的一切文档、
 * 注释、缺陷报告都以中文为准，兜到 `en` 只会让一条缺翻译**看起来**是正常的。
 */
const FALLBACK = "zh-CN";

function readLang() {
  try {
    const v = localStorage.getItem(STORE);
    if (v && LANGS.includes(v)) return v;
  } catch (e) { /* 隐私模式：走默认 */ }
  return FALLBACK;
}

let lang = readLang();

export function currentLang() { return lang; }

export function t(key, params) {
  const row = I18N[key];
  let s = row ? (row[lang] ?? row[FALLBACK]) : undefined;
  if (s === undefined) {
    try {
      if (localStorage.getItem(DEBUG_STORE)) console.warn("[i18n] 缺 key:", key);
    } catch (e) { /* ignore */ }
    return key;
  }
  // `split().join()` 而不是正则：占位符里出现正则元字符时不会炸，也不必转义。
  if (params) for (const [k, v] of Object.entries(params)) s = s.split(`{${k}}`).join(String(v));
  return s;
}

/** 三个标记钩子。全部走 textContent / setAttribute，**永不 innerHTML**。 */
export function apply(root) {
  const scope = root || document;
  for (const el of scope.querySelectorAll("[data-i18n]")) el.textContent = t(el.getAttribute("data-i18n"));
  for (const el of scope.querySelectorAll("[data-i18n-ph]")) el.setAttribute("placeholder", t(el.getAttribute("data-i18n-ph")));
  for (const el of scope.querySelectorAll("[data-i18n-title]")) {
    const s = t(el.getAttribute("data-i18n-title"));
    el.setAttribute("title", s);
    // 无 aria-label 时一并写：图标按钮只有 title 的话读屏器读不出来。
    if (!el.hasAttribute("aria-label")) el.setAttribute("aria-label", s);
  }
}

export function setLang(next) {
  if (!LANGS.includes(next)) return;
  lang = next;
  try { localStorage.setItem(STORE, next); } catch (e) { /* ignore */ }
  document.documentElement.setAttribute("data-lang", next);
  document.documentElement.lang = next;
  apply(document);
  document.dispatchEvent(new CustomEvent("langchange", { detail: { lang: next } }));
}
