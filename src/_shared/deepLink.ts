/**
 * zbiz:// 深链接的 **sys 语义层**。
 *
 * ## 分工
 *
 * | 层 | 在哪 | 管什么 |
 * |----|----|--------|
 * | 协议 | `z-biz-tool-shared/protocol` | 字节级编解码（金标 = Rust `cap-hub`） |
 * | 机制 | `z-biz-tool-shared/protocol` 的 `startDeepLinkReceiver` | 冷启动补取、事件监听、不静默吞掉 |
 * | **语义** | **本文件** | **收到深链后 sys 该做什么** |
 *
 * ## sys 支持的动作
 *
 * | action | 含义 |
 * |--------|------|
 * | `open` | 进入/切换到本应用（缺省 action） |
 *
 * `open` 不带参数时只是把 sys 唤到前台（no-op，不报错）；带参数时定位到具体视图。
 *
 * ## 支持的参数
 *
 * | 参数 | 作用 |
 * |------|------|
 * | `tab` | 切到某个视图：`overview` \| `agent` \| `processes` \| `network` \| `disk` \| `cleanup` \| `largefiles` \| `startup` \| `system`（白名单外 ⇒ problem「不认识的 tab X」） |
 * | `pid` | 切到 `processes` 视图并按 pid / 关键字过滤（数值 pid 或进程名都行，直接喂给 `changeProcessKeyword`） |
 * | `from`| 发送方，仅用于日志与回执 |
 *
 * `pid` 与 `tab` 同时出现时，`pid` 优先（它已经隐含「去 processes 页」），`tab` 被忽略。
 */

import type { Handoff } from "z-biz-tool-shared/protocol";

/** sys 的视图（tab）白名单，与 `src/App.tsx` 的 `tabItems` 一致 */
export const SYS_TABS = [
  "overview",
  "agent",
  "processes",
  "network",
  "disk",
  "cleanup",
  "largefiles",
  "startup",
  "system",
] as const;

export type SysTab = (typeof SYS_TABS)[number];

/**
 * sys 的深链结果。
 *
 * `tab` / `pid` 非 null 表示要切到对应视图 / 按 pid 过滤；
 * `problem` 非 null 表示接不住 —— 前端必须给可见提示（不静默吞掉）。
 */
export interface SysDeepLinkOutcome {
  /** 要切换到的视图；null 表示不带切视图意图（pid 分支会强制置 "processes"） */
  tab: SysTab | null;
  /** 要按 pid / 关键字过滤的值；null 表示不带定位意图 */
  pid: string | null;
  /** 发送方（from），仅用于提示与日志 */
  from: string | null;
  /** 无法适配时的原因 */
  problem: string | null;
}

/**
 * 把一条深链翻译成 sys 内的动作。
 *
 * 纯函数 —— 不碰 React state、不碰 Tauri，所以可以直接单测。
 *
 * @param item 深链
 */
export function resolveSysDeepLink(item: Handoff): SysDeepLinkOutcome {
  const outcome: SysDeepLinkOutcome = {
    tab: null,
    pid: null,
    from: item.params.from ?? null,
    problem: null,
  };

  const action = item.action.trim() || "open";
  if (action !== "open") {
    outcome.problem = `不认识的动作 ${action}`;
    return outcome;
  }

  const rawTab = item.params.tab?.trim() ?? "";
  const rawPid = item.params.pid?.trim() ?? "";

  // 视图：白名单校验
  if (rawTab) {
    if ((SYS_TABS as readonly string[]).includes(rawTab)) {
      outcome.tab = rawTab as SysTab;
    } else {
      outcome.problem = `不认识的 tab ${rawTab}`;
      return outcome;
    }
  }

  // pid：定位到 processes 页并过滤。pid 优先于 tab（它已经隐含了去哪页）
  if (rawPid) {
    outcome.pid = rawPid;
    outcome.tab = "processes";
  }

  return outcome;
}
