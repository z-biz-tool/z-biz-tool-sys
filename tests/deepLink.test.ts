// sys 深链语义层（src/_shared/deepLink.ts）的纯函数回归。
// 跑法：node --experimental-strip-types --test tests/*.test.ts
//
// 这一层要防的是：
//   1. 白名单外视图被当成合法 tab 透传给 setActiveTab（界面闪到不存在的页）
//   2. 未知 action 被静默忽略（用户「点了没反应」）
//   3. pid 与 tab 同时出现时到底听谁的（pid 隐含「去 processes」，应优先）
//   4. 参数前后空格 / 空串被当成合法值
import { test } from "node:test";
import assert from "node:assert/strict";

import { resolveSysDeepLink, SYS_TABS } from "../src/_shared/deepLink.ts";
import type { Handoff } from "z-biz-tool-shared/protocol";

/** 造一条深链，默认 product=sys、action=open */
function mk(params: Record<string, string>, action = "open"): Handoff {
  return { product: "sys", action, params };
}

test("正常：open + 合法 tab 切到该视图", () => {
  const out = resolveSysDeepLink(mk({ tab: "processes" }));
  assert.equal(out.problem, null);
  assert.equal(out.tab, "processes");
  assert.equal(out.pid, null);
});

test("正常：open + overview 等其它合法 tab", () => {
  for (const tab of SYS_TABS) {
    const out = resolveSysDeepLink(mk({ tab }));
    assert.equal(out.problem, null);
    assert.equal(out.tab, tab);
  }
});

test("缺参数：open 不带任何参数 = 仅唤起，不报错", () => {
  const out = resolveSysDeepLink(mk({}));
  assert.equal(out.problem, null);
  assert.equal(out.tab, null);
  assert.equal(out.pid, null);
});

test("不认识 tab：白名单外视图给出 problem", () => {
  const out = resolveSysDeepLink(mk({ tab: "foo" }));
  assert.equal(out.problem, "不认识的 tab foo");
  assert.equal(out.tab, null);
});

test("pid 分支：open + pid 切到 processes 并按 pid 过滤", () => {
  const out = resolveSysDeepLink(mk({ pid: "1234" }));
  assert.equal(out.problem, null);
  assert.equal(out.pid, "1234");
  assert.equal(out.tab, "processes");
});

test("pid 优先于 tab：同时给 pid 和 tab 时去 processes，tab 被忽略", () => {
  const out = resolveSysDeepLink(mk({ pid: "1234", tab: "network" }));
  assert.equal(out.problem, null);
  assert.equal(out.pid, "1234");
  assert.equal(out.tab, "processes");
});

test("未知 action：非 open 一律报不认识的动作", () => {
  const out = resolveSysDeepLink(mk({}, "close"));
  assert.equal(out.problem, "不认识的动作 close");
});

test("action 缺省：空 action 视为 open，不报错", () => {
  const out = resolveSysDeepLink(mk({ tab: "disk" }, ""));
  assert.equal(out.problem, null);
  assert.equal(out.tab, "disk");
});

test("参数前后空格：tab 的空格被 trim 后仍认得出", () => {
  const out = resolveSysDeepLink(mk({ tab: "  processes  " }));
  assert.equal(out.problem, null);
  assert.equal(out.tab, "processes");
});

test("空字符串参数：tab 给空串等同于不带 tab，不报错", () => {
  const out = resolveSysDeepLink(mk({ tab: "" }));
  assert.equal(out.problem, null);
  assert.equal(out.tab, null);
});

test("from 透传：发送方只用于提示与日志，不影响定位", () => {
  const out = resolveSysDeepLink(mk({ tab: "system", from: "box" }));
  assert.equal(out.problem, null);
  assert.equal(out.tab, "system");
  assert.equal(out.from, "box");
});

test("tab 非法时即使带 pid 也优先报 tab 问题", () => {
  const out = resolveSysDeepLink(mk({ tab: "nope", pid: "99" }));
  assert.equal(out.problem, "不认识的 tab nope");
  assert.equal(out.tab, null);
});
