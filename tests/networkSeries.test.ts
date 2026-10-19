// per-interface 时序（T3-02）的纯逻辑回归：无 React、无 IPC。
// 跑法：node --experimental-strip-types --test tests/*.test.ts
//
// 这一层要防的是**三类错**，每一类都会让界面说谎：
//  1. 把不同接口的流量混成一条线（那就退化成后端已有的聚合曲线，白做）；
//  2. 数据流停了还继续补点（画出一条假的水平线，用户以为流量平稳）；
//  3. 同一时刻重复出点（曲线上出现零宽度的锯齿，读数与图对不上）。
import { test } from "node:test";
import assert from "node:assert/strict";

import {
  NET_SERIES_INTERFACES,
  NET_SERIES_POINTS,
  appendNetworkFrame,
  networkPoints,
  networkRows,
  summarizeNetwork,
  type NetworkSeries,
} from "../src/lib/networkSeries.ts";
import type { MetricsSnapshot, NetworkMetrics } from "../src/ipc_contract.ts";

function nic(name: string, rx: number, tx: number): NetworkMetrics {
  return {
    interface: name,
    status: "up",
    ipv4: "10.0.0.1",
    ipv6: null,
    mac: "aa:bb:cc:dd:ee:ff",
    rxBytesPerSec: rx,
    txBytesPerSec: tx,
    totalReceivedBytes: 1000,
    totalTransmittedBytes: 2000,
    packetsReceived: 10,
    packetsTransmitted: 20,
  };
}

function frame(t: number, nics: NetworkMetrics[]): MetricsSnapshot {
  return {
    timestampMs: t,
    uptimeSeconds: t / 1000,
    cpu: {
      usagePercent: 10,
      perCore: [],
      loadAverage: [0, 0, 0],
      model: "test",
      frequencyMhz: 1000,
      temperatureCelsius: null,
    },
    memory: { totalBytes: 1, usedBytes: 1, availableBytes: 0, usagePercent: 50, swapUsedBytes: 0 },
    disks: [],
    networks: nics,
  } as unknown as MetricsSnapshot;
}

test("每个接口各自一条时序 —— 不能把流量混成一条", () => {
  let s: NetworkSeries = new Map();
  s = appendNetworkFrame(s, frame(1000, [nic("en0", 100, 10), nic("en1", 5000, 700)]));
  s = appendNetworkFrame(s, frame(2000, [nic("en0", 200, 20), nic("en1", 9000, 800)]));

  const en0 = networkPoints(s, "en0");
  const en1 = networkPoints(s, "en1");
  assert.equal(en0.length, 2);
  assert.equal(en1.length, 2);
  assert.deepEqual(en0.map((p) => p.rx), [100, 200]);
  assert.deepEqual(en1.map((p) => p.rx), [5000, 9000]);
  assert.deepEqual(en0.map((p) => p.tx), [10, 20]);
  // 两者的时刻必须对齐（同一帧采集），否则曲线会错位
  assert.equal(en0[0].t, en1[0].t);
});

test("没有数据的接口返回空数组，不抛", () => {
  const s = appendNetworkFrame(new Map(), frame(1000, [nic("en0", 1, 1)]));
  assert.deepEqual(networkPoints(s, "nope"), []);
  assert.deepEqual(networkPoints(s, null), []);
});

test("同一时刻的重复帧：替换而不是追加（否则图上出现零宽度的重复点）", () => {
  let s = appendNetworkFrame(new Map(), frame(1000, [nic("en0", 100, 10)]));
  s = appendNetworkFrame(s, frame(1000, [nic("en0", 999, 99)]));
  const points = networkPoints(s, "en0");
  assert.equal(points.length, 1, "同一时刻只应有一个点");
  assert.equal(points[0].rx, 999, "值要被最新那帧替换");
});

test("数据流停了就不补点 —— 曲线停在停采那一刻", () => {
  let s = appendNetworkFrame(new Map(), frame(1000, [nic("en0", 100, 10)]));
  s = appendNetworkFrame(s, frame(2000, [nic("en0", 200, 20)]));
  // 空帧（后端停采时可能推来一帧空 networks）
  s = appendNetworkFrame(s, frame(3000, []));
  const points = networkPoints(s, "en0");
  assert.equal(points.length, 2, "空帧不得补点");
  assert.equal(points[points.length - 1].t, 2000);
});

test("接口消失后历史保留，但不再增长", () => {
  let s = appendNetworkFrame(new Map(), frame(1000, [nic("en0", 100, 10), nic("usb0", 7, 7)]));
  s = appendNetworkFrame(s, frame(2000, [nic("en0", 200, 20)]));
  const usb = networkPoints(s, "usb0");
  assert.equal(usb.length, 1, "消失的接口历史要留着（曲线不能一段段消失）");
  assert.equal(usb[0].t, 1000);
});

test("点数有界：超过上限时丢最旧的", () => {
  let s: NetworkSeries = new Map();
  for (let i = 0; i < NET_SERIES_POINTS + 50; i += 1) {
    s = appendNetworkFrame(s, frame(1000 + i, [nic("en0", i, i)]));
  }
  const points = networkPoints(s, "en0");
  assert.equal(points.length, NET_SERIES_POINTS);
  assert.equal(points[0].t, 1000 + 50, "丢的应该是最旧的");
  assert.equal(points[points.length - 1].t, 1000 + NET_SERIES_POINTS + 49);
});

test("接口数有界：veth 这类一次性网卡名不能把内存撑爆", () => {
  let s: NetworkSeries = new Map();
  for (let i = 0; i < NET_SERIES_INTERFACES + 10; i += 1) {
    s = appendNetworkFrame(s, frame(1000 + i, [nic(`veth${i}`, i, i)]));
  }
  assert.equal(s.size, NET_SERIES_INTERFACES);
  // 丢的应该是最久没更新的（最旧数据），不是最新那个
  assert.ok(s.has(`veth${NET_SERIES_INTERFACES + 9}`), "最新的接口必须留着");
  assert.ok(!s.has("veth0"), "最旧的接口应被丢掉");
});

test("appendNetworkFrame 不改动入参", () => {
  const prev = appendNetworkFrame(new Map(), frame(1000, [nic("en0", 100, 10)]));
  const snapshot = prev.get("en0");
  const before = snapshot?.length ?? 0;
  appendNetworkFrame(prev, frame(2000, [nic("en0", 200, 20)]));
  assert.equal(prev.get("en0")?.length, before, "入参的队列长度不该变");
  assert.equal(prev.get("en0")?.[0].rx, 100, "入参的值不该被改写");
});

test("空快照与 null 都不炸，且不丢历史", () => {
  let s = appendNetworkFrame(new Map(), frame(1000, [nic("en0", 100, 10)]));
  s = appendNetworkFrame(s, null);
  assert.equal(networkPoints(s, "en0").length, 1);
});

test("畸形快照（没有 networks 字段）不炸，且不动已有历史", () => {
  // 后端 payload 缺字段时，前端不能整页崩掉 —— 这里就是那道防线。
  const bad = { timestampMs: 2000 } as unknown as MetricsSnapshot;
  let s = appendNetworkFrame(new Map(), frame(1000, [nic("en0", 100, 10)]));
  s = appendNetworkFrame(s, bad);
  assert.equal(networkPoints(s, "en0").length, 1);
  assert.equal(networkPoints(s, "en0")[0].t, 1000);
});

test("汇总统计：峰值 / 均值 / 末值，空序列不返回 NaN", () => {
  const points = [
    { t: 1, rx: 100, tx: 10 },
    { t: 2, rx: 300, tx: 30 },
    { t: 3, rx: 200, tx: 20 },
  ];
  const s = summarizeNetwork(points);
  assert.equal(s.count, 3);
  assert.equal(s.lastRx, 200);
  assert.equal(s.lastTx, 20);
  assert.equal(s.peakRx, 300);
  assert.equal(s.peakTx, 30);
  assert.equal(s.avgRx, 200);
  assert.equal(s.avgTx, 20);

  const empty = summarizeNetwork([]);
  assert.equal(empty.count, 0);
  // NaN 会让界面显示 "NaN"，比显示 0 更糟
  for (const v of Object.values(empty)) assert.ok(Number.isFinite(v), `${v} 不是有限数`);
});

test("图表行的 time 是秒精度，且与 TrendChart 的 XAxis 对得上", () => {
  const rows = networkRows([{ t: 1700000000000, rx: 1.23456, tx: 2.34567 }]);
  assert.equal(rows.length, 1);
  assert.match(rows[0].time, /^\d{2}:\d{2}:\d{2}$/);
  assert.equal(rows[0].rx, 1.23);
  assert.equal(rows[0].tx, 2.35);
});
