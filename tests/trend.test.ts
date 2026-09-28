// 趋势降采样 + 度量算术的纯逻辑回归（无 React、无 IPC）。
// 跑法：node --experimental-strip-types --test tests/*.test.ts
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  HISTORY_LIMIT,
  MAX_TREND_POINTS,
  alignAlertMarkers,
  formatClock,
  resampleEnvelope,
  toChartRows,
  windowHistory,
} from "../src/lib/trend.ts";
import {
  counterDelta,
  counterRatePerSec,
  pickByteUnit,
  sumRates,
  usageLevel,
} from "../src/lib/metrics_math.ts";
import { formatBytes, formatRate, usageColor } from "../src/lib/format.ts";
import type { AlertEvent, HistoryPoint } from "../src/ipc_contract.ts";

/** 造一段等间隔采样：`values` 决定每条的取值，起点固定以免时间影响断言 */
function series(values: (i: number) => number, startMs = 1_700_000_000_000): HistoryPoint[] {
  return Array.from({ length: 14_400 }, (_, i) => ({
    t: startMs + i * 500,
    cpu: values(i),
    memory: values(i),
    rxBytesPerSec: values(i),
    txBytesPerSec: 0,
  }));
}

test("14 400 点里一个 1% 的尖峰必须在降采样后还看得见", () => {
  // 基线 0%，只有第 7 200 帧是 1%（0.5 s 一帧 = 一桶 60 帧里的孤立一帧）
  const points = series((i) => (i === 7200 ? 1 : 0));
  const out = resampleEnvelope(points, (p) => p.cpu);
  assert.ok(
    out.some((x) => x.v === 1),
    "均值降采样会把这一帧摊成 0.0167%，图上等于没发生过"
  );
  assert.ok(!out.every((x) => x.v === 0), "全零说明极值丢了");
});

test("降采样后的点数受桶数上限约束，且时间严格不减", () => {
  const out = resampleEnvelope(series((i) => (i % 97 === 0 ? 100 : 20)), (p) => p.cpu);
  assert.ok(out.length <= MAX_TREND_POINTS * 2, `每桶最多 min+max 两点，实际 ${out.length}`);
  for (let i = 1; i < out.length; i += 1) assert.ok(out[i].t >= out[i - 1].t, "桶内必须按时间落点");
});

test("极值点只能是真实存在过的采样时刻，不造点", () => {
  const points = series((i) => (i % 5) * 10);
  const times = new Set(points.map((p) => p.t));
  for (const x of resampleEnvelope(points, (p) => p.cpu)) {
    assert.ok(times.has(x.t), `时间戳 ${x.t} 不在原始采样里`);
    assert.ok(
      points.some((p) => p.t === x.t && p.cpu === x.v),
      `值 ${x.v} 与时刻 ${x.t} 对不上任何一帧`
    );
  }
});

test("平直线每桶只落一个点，空输入与单点输入都不炸", () => {
  assert.deepEqual(resampleEnvelope([], (p) => p.cpu), []);
  const one: HistoryPoint[] = [{ t: 10, cpu: 42, memory: 0, rxBytesPerSec: 0, txBytesPerSec: 0 }];
  assert.deepEqual(resampleEnvelope(one, (p) => p.cpu), [{ t: 10, v: 42 }]);
  const flat = series(() => 30);
  assert.equal(resampleEnvelope(flat, (p) => p.cpu).length, MAX_TREND_POINTS);
});

test("窗口裁剪以最新一帧为锚，数据停滞时不把整段历史挤掉", () => {
  const points = series(() => 10);
  const windowed = windowHistory(points, 60);
  // 两端都含（`>=`）：60 s 跨度在 0.5 s 间隔上是 120 段 = 121 帧
  assert.equal(windowed.length, 121);
  assert.equal(windowed[windowed.length - 1].t, points[points.length - 1].t);
  assert.equal(windowHistory([], 60).length, 0);
  // 锚点是"最后一帧"而不是 Date.now()：采集暂停后窗口不该继续往前滑
  const stopped = points.slice(0, 200);
  assert.equal(windowHistory(stopped, 60).length, 121);
});

test("chartData 与告警标注共用同一套 time 字符串", () => {
  const points = series(() => 5);
  const rows = toChartRows(resampleEnvelope(points, (p) => p.cpu));
  assert.equal(rows.length > 0, true);
  for (const r of rows) {
    assert.equal(r.time, formatClock(r.t));
    assert.match(r.time, /^\d{2}:\d{2}:\d{2}$/);
  }
});

test("告警竖线只落在曲线上真存在的行，落不上的老实计数", () => {
  const rows = toChartRows(resampleEnvelope(series(() => 5), (p) => p.cpu));
  const inside = rows[10];
  const events: AlertEvent[] = [
    {
      metric: "cpu",
      level: "warning",
      value: 88,
      threshold: 80,
      target: null,
      consecutive: 3,
      timestampMs: inside.t,
    },
    {
      metric: "cpu",
      level: "critical",
      value: 99,
      threshold: 95,
      target: null,
      consecutive: 1,
      timestampMs: inside.t + 7_200_000_000,
    },
  ];
  const result = alignAlertMarkers(events, rows, "cpu");
  assert.equal(result.markers.length, 1);
  assert.equal(result.markers[0].x, inside.time, "竖线必须对齐到那一行自己的 time");
  assert.equal(result.outside, 1, "窗口外的告警不画线，也不许造一个插值位置");
  assert.equal(result.maxSkewMs, 0);
});

test("网络累计计数器复位不会算出负速率", () => {
  assert.equal(counterDelta(1_000, 1_500), 500);
  assert.equal(counterDelta(1_000, 1_000), 0);
  // 复位/回绕：只承认 next 本身（从零重新计），绝不给出负数
  assert.equal(counterDelta(1_000, 200), 200);
  assert.ok(counterDelta(1_000, 200) >= 0);
  // 首帧没有基线，不能把"开机以来的总字节"当成这一秒的增量
  assert.equal(counterDelta(null, 9_000_000_000), 0);
  assert.equal(counterDelta(1_000, Number.NaN), 0);
  assert.equal(counterDelta(Number.NaN, 1_000), 0);
});

test("速率换算：窗口过短或计数器坏了都不给天文数字", () => {
  assert.equal(counterRatePerSec(0, 1024, 1), 1024);
  assert.equal(counterRatePerSec(1_000, 200, 1), 200, "复位后按新基线给速率");
  assert.equal(counterRatePerSec(0, 1_000_000, 0), 0, "0 秒窗口上的除法会炸成 Infinity");
  assert.equal(counterRatePerSec(0, 1_000_000, Number.NaN), 0);
});

test("多接口速率求和时坏掉的一条不带走其余", () => {
  assert.equal(sumRates([10, 20, 30]), 60);
  assert.equal(sumRates([10, Number.NaN, -5, undefined, 5]), 15);
  assert.equal(sumRates([]), 0);
});

test("字节单位：非有限值不许变成 NaN B", () => {
  assert.deepEqual(pickByteUnit(0, ["B", "KB"]), { value: 0, unit: "B" });
  assert.equal(pickByteUnit(1024, ["B", "KB", "MB"]).unit, "KB");
  assert.equal(pickByteUnit(1536, ["B", "KB", "MB"]).value, 1.5);
  assert.equal(pickByteUnit(Number.POSITIVE_INFINITY, ["B", "KB"]).unit, "B");
  assert.equal(formatBytes(0), "0 B");
  assert.equal(formatBytes(Number.NaN), "0 B");
  assert.equal(formatBytes(1024 ** 2, 1), "1.0 MB");
  assert.equal(formatRate(null), "—", "读不到不能报成 0 B/s");
  assert.equal(formatRate(2048), "2.0 KB/s");
});

test("阈值分档：超过红、八成黄、其余绿，阈值坏了不猜颜色", () => {
  assert.equal(usageLevel(90, 80), "danger");
  assert.equal(usageLevel(70, 80), "warn");
  assert.equal(usageLevel(10, 80), "ok");
  assert.equal(usageLevel(Number.NaN, 80), null);
  assert.equal(usageLevel(10, 0), null, "阈值为 0 说明这一档被关了");
  assert.equal(usageColor(90, 80), "#ff4d4f");
  assert.equal(usageColor(70, 80), "#faad14");
  assert.equal(usageColor(10, 80), "#52c41a");
  assert.equal(usageColor(10, Number.NaN), "#8c8c8c");
});

test("历史缓冲上限就是契约里的那个 14 400", () => {
  assert.equal(HISTORY_LIMIT, 14_400);
});
