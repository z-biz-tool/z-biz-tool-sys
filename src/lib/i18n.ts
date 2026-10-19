/**
 * 最小 i18n 基建（T4-09）。
 *
 * 背景：任务表 T4-09 写的是「index.html 修复（**title 本地化** + 移除缺失
 * vite.svg 引用）」。`vite.svg` 那半早就清零了，title 那半一直没做 ——
 * 原因不是偷懒，是**本仓根本没有 i18n 机制**（`grep -rn "i18n\|locale"` 在
 * `src/` 下 0 处命中），一个只有中文的界面，「把 title 本地化」没有落脚点。
 *
 * 所以这里补的是**地基**，不是全文翻译：
 *   · 一份语言白名单（读侧过白名单 —— 与 `usePrefs` 顶部那条纪律同源：
 *     陌生/旧版本残留值不能让界面塌掉）；
 *   · 文档标题按语言取值并写进 `document.title`；
 *   · `<html lang>` 跟着一起改（读屏软件与浏览器翻译都靠它）。
 *
 * ⚠️ **边界要说清楚**：目前只有**标题**是本地化的，界面正文仍是中文。
 * 这正是 T4-09 这一条的范围（它写的就是 index.html 的 title），
 * 但别把它读成「本应用已支持多语言」—— 那是另一件事，量级差一个数量级。
 */

/** 支持的语言。扩展时这里加一项，标题表与下拉项各自补齐即可。 */
export const LANGS = [
  { value: "zh-CN", label: "简体中文" },
  { value: "en-US", label: "English" },
] as const;

export type Lang = (typeof LANGS)[number]["value"];

export const DEFAULT_LANG: Lang = "zh-CN";

/** 读侧白名单：认不出的值一律回落默认，绝不把陌生值透传给 `document.title`。 */
export function normalizeLang(raw: unknown): Lang {
  const text = typeof raw === "string" ? raw.trim() : "";
  const hit = LANGS.find((l) => l.value === text);
  return hit ? hit.value : DEFAULT_LANG;
}

/** 文档标题（窗口标题 / 任务栏 / 浏览器标签页）。 */
export const DOC_TITLES: Record<Lang, string> = {
  "zh-CN": "系统工具箱 · 监控与清理",
  "en-US": "System Toolbox · Monitor & Clean",
};

export function docTitle(lang: unknown): string {
  return DOC_TITLES[normalizeLang(lang)];
}

/**
 * 把语言应用到文档。抽成独立函数是为了能脱离 DOM 测纯逻辑，
 * 也为了让「改标题」和「改 html lang」这两件事永远在同一次调用里发生 ——
 * 分成两处的话，早晚会出现「标题变了、lang 没变」的不一致。
 */
export function applyDocumentLanguage(lang: unknown, doc?: Document): void {
  const normalized = normalizeLang(lang);
  const target = doc ?? (typeof document !== "undefined" ? document : null);
  if (!target) return;
  target.title = DOC_TITLES[normalized];
  target.documentElement.lang = normalized;
}
