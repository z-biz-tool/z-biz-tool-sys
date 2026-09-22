import { Badge, Button, Select, Tooltip, Typography } from "antd";
import { ExpandOutlined } from "@ant-design/icons";
import dayjs from "dayjs";
import { ALERT_LEVEL_COLORS, busiestDisk, ALERT_LEVEL_LABELS, ALERT_METRIC_LABELS } from "../lib/alert";
import {
  formatBytes,
  formatGb,
  formatPercent,
  formatRate,
  formatUptime,
  usageColor,
} from "../lib/format";
import { INTERVAL_OPTIONS, monitorCardStyle } from "../lib/ui";
import { downsampleHistory, TREND_RANGES, windowHistory } from "../lib/trend";
import type {
  AlertConfig,
  AlertEvent,
  HistoryPoint,
  MemoryPressure,
  MetricsSnapshot,
} from "../ipc_contract";
import type { LinkStatus } from "../hooks/useSystemMonitor";

const { Text } = Typography;

interface Props {
  snapshot: MetricsSnapshot | null;
  status: LinkStatus;
  /** 与概览页趋势图同一份实时点 + 同一个时间窗：迷你模式不另开一条采样通路 */
  history: HistoryPoint[];
  trendRange: number;
  /** 只取它的 warning 档做配色，判定仍以后端推送的告警为准 */
  alertConfig: AlertConfig;
  latestAlert: AlertEvent | null;
  intervalMs: number;
  onIntervalChange: (interval: number) => void;
  onExit: () => void;
}

const PRESSURE_COLOR: Record<MemoryPressure, string> = {
  normal: "#52c41a",
  warning: "#faad14",
  critical: "#ff4d4f",
};

const PRESSURE_LABEL: Record<MemoryPressure, string> = {
  normal: "正常",
  warning: "警告",
  critical: "危险",
};

/** 没有读数时的折线颜色：灰，不给一个"看起来像正常值"的绿色 */
const IDLE_STROKE = "#8c8c8c";

const STRIP_HEIGHT = 60;

/** 一格读数：拿不到值时只能显示 `—`，这里没有"先给个 0"的分支 */
function Readout({
  label,
  value,
  sub,
  color,
  testId,
}: {
  label: string;
  value: string;
  sub?: string;
  color?: string;
  testId: string;
}) {
  return (
    <div data-mini={testId} style={{ display: "flex", flexDirection: "column", minWidth: 92 }}>
      <Text type="secondary" style={{ fontSize: 11, lineHeight: "16px" }}>
        {label}
      </Text>
      <Text strong style={{ fontSize: 18, lineHeight: "22px", color }}>
        {value}
      </Text>
      <Text type="secondary" style={{ fontSize: 11, lineHeight: "14px" }}>
        {sub ?? " "}
      </Text>
    </div>
  );
}

/**
 * 折线只用真实存在的点：少于两点就什么都不画（一条线段的斜率是猜出来的）。
 * 纵轴固定 0–100 %，与 `HistoryPoint.cpu/memory` 的百分比口径一致。
 */
function Sparkline({
  points,
  label,
  testId,
  color,
}: {
  points: number[];
  label: string;
  testId: string;
  color: string;
}) {
  if (points.length < 2) {
    return (
      <div data-mini={testId} style={{ height: 34 }}>
        <Text type="secondary" style={{ fontSize: 11 }}>
          {label}：采样点不足（{points.length} 点），不画曲线
        </Text>
      </div>
    );
  }
  const step = 100 / (points.length - 1);
  const line = points
    .map((value, index) => `${(index * step).toFixed(2)},${(30 - (Math.min(value, 100) / 100) * 28).toFixed(2)}`)
    .join(" ");
  return (
    <div data-mini={testId} style={{ display: "flex", alignItems: "center", gap: 6 }}>
      <Text type="secondary" style={{ fontSize: 11, whiteSpace: "nowrap" }}>
        {label}
      </Text>
      <svg width={160} height={32} viewBox="0 0 100 32" preserveAspectRatio="none" aria-hidden>
        <polyline points={line} fill="none" stroke={color} strokeWidth={1.2} vectorEffect="non-scaling-stroke" />
      </svg>
      <Text type="secondary" style={{ fontSize: 11, whiteSpace: "nowrap" }}>
        {points.length} 点
      </Text>
    </div>
  );
}

function Panel({
  title,
  testId,
  children,
}: {
  title: string;
  testId: string;
  children: React.ReactNode;
}) {
  return (
    <section
      data-mini={testId}
      style={{ ...monitorCardStyle, padding: "8px 10px", overflow: "hidden", flex: "1 1 0", minWidth: 0 }}
    >
      <Text type="secondary" style={{ fontSize: 11 }}>
        {title}
      </Text>
      <div style={{ marginTop: 6 }}>{children}</div>
    </section>
  );
}

const rowStyle = {
  display: "flex",
  justifyContent: "space-between",
  gap: 8,
  fontSize: 12,
  lineHeight: "20px",
} as const;

/**
 * 迷你模式（T5-07）：一屏只留关键指标，纯投影已有的 `sys://metrics` 帧。
 *
 * 刻意不做的事：① 不新增任何 IPC —— 这一屏要的数全在 App 已有的快照/历史里，
 * 再开一条"迷你模式专用采集"只会让同一个指标出现两套节奏；② 不缩放原生窗口、
 * 不做系统菜单栏常驻 —— capabilities 是 8 条显式权限且不含任何窗口写类权限
 * （逐条见 doc/优化方案/04 的 SEC-V02），要缩窗口或常驻菜单栏得先扩权限，
 * 与 T5-02 是同一道授权门槛；③ 不自己判阈值：越限的颜色按 `alertConfig` 的 warning 档上色（那份配置是后端
 * 回过的生效值），真正的"越限"文案只来自后端推来的告警事件。
 */
export function MiniMenuBar({
  snapshot,
  status,
  history,
  trendRange,
  alertConfig,
  latestAlert,
  intervalMs,
  onIntervalChange,
  onExit,
}: Props) {
  const disk = busiestDisk(snapshot);
  const usableNetworks = snapshot?.networks ?? [];
  // 无接口 ≠ 零流量：拿不到接口时是 `—`，不能把"没有数据"显示成"没在传"
  const rx = usableNetworks.length ? usableNetworks.reduce((a, n) => a + n.rxBytesPerSec, 0) : null;
  const tx = usableNetworks.length ? usableNetworks.reduce((a, n) => a + n.txBytesPerSec, 0) : null;
  const perCore = snapshot?.cpu.perCore ?? [];
  const usableDisks = (snapshot?.disks ?? []).filter((d) => d.available);
  // 与概览页趋势图同一套裁剪与降采样：同一时刻两条曲线不该给出两个形状的折线
  const windowed = downsampleHistory(windowHistory(history, trendRange));
  const rangeLabel = TREND_RANGES.find((r) => r.value === trendRange)?.label ?? `${trendRange} 秒`;

  return (
    <div
      data-mini="root"
      style={{
        display: "flex",
        flexDirection: "column",
        gap: 10,
        height: "100%",
        padding: "10px 12px",
        overflow: "hidden",
        background: "transparent",
      }}
    >
      <div
        data-mini="strip"
        style={{
          ...monitorCardStyle,
          minHeight: STRIP_HEIGHT,
          padding: "6px 12px",
          display: "flex",
          alignItems: "center",
          gap: 18,
          flexWrap: "wrap",
        }}
      >
        <Text strong style={{ fontSize: 13 }}>
          迷你监控
        </Text>
        <Badge
          status={status === "live" ? "success" : status === "stalled" ? "error" : "processing"}
          text={status === "live" ? "实时采集" : status === "stalled" ? "采集停滞" : "连接中"}
        />
        <Readout
          testId="cpu"
          label={`CPU（${snapshot ? `${snapshot.cpu.coreCount} 核` : "核数未知"}）`}
          value={snapshot ? formatPercent(snapshot.cpu.total) : "—"}
          color={snapshot ? usageColor(snapshot.cpu.total, alertConfig.cpu.warning) : undefined}
          sub={snapshot ? `越限线 ${alertConfig.cpu.warning}%` : "等待首帧"}
        />
        <Readout
          testId="memory"
          label="内存"
          value={snapshot ? formatPercent(snapshot.memory.usagePercent) : "—"}
          color={snapshot ? PRESSURE_COLOR[snapshot.memory.pressure] : undefined}
          sub={
            snapshot
              ? `${formatBytes(snapshot.memory.usedBytes, 1)} / ${formatGb(snapshot.memory.totalBytes)} · ${PRESSURE_LABEL[snapshot.memory.pressure]}`
              : "等待首帧"
          }
        />
        <Readout
          testId="disk"
          label="磁盘"
          value={disk ? formatPercent(disk.usagePercent) : "—"}
          color={disk ? usageColor(disk.usagePercent, alertConfig.disk.warning) : undefined}
          sub={disk ? `${disk.mountPoint} 剩 ${formatGb(disk.availableBytes)}` : "无可用分区"}
        />
        <Readout
          testId="network"
          label="网络"
          value={
            rx === null && tx === null
              ? "—"
              : `↓ ${rx === null ? "—" : formatRate(rx)} ↑ ${tx === null ? "—" : formatRate(tx)}`
          }
          sub={snapshot ? `${usableNetworks.length} 个接口` : "等待首帧"}
        />
        <Readout
          testId="uptime"
          label="运行"
          value={snapshot ? formatUptime(snapshot.uptimeSeconds) : "—"}
          sub={snapshot ? `帧 ${dayjs(snapshot.timestampMs).format("HH:mm:ss")}` : "尚无采样帧"}
        />
        <div style={{ marginLeft: "auto", display: "flex", alignItems: "center", gap: 8 }}>
          <Select
            size="small"
            value={intervalMs}
            style={{ width: 92 }}
            options={INTERVAL_OPTIONS}
            onChange={onIntervalChange}
          />
          <Tooltip title="退出迷你模式（Esc）">
            <Button size="small" icon={<ExpandOutlined />} onClick={onExit}>
              退出迷你
            </Button>
          </Tooltip>
        </div>
      </div>

      <div data-mini="trend" style={{ ...monitorCardStyle, padding: "8px 12px", display: "flex", gap: 24, flexWrap: "wrap" }}>
        <Sparkline
          testId="spark-cpu"
          label={`CPU · 近 ${rangeLabel}`}
          color={snapshot ? usageColor(snapshot.cpu.total, alertConfig.cpu.warning) : IDLE_STROKE}
          points={windowed.map((p) => p.cpu)}
        />
        <Sparkline
          testId="spark-memory"
          label={`内存 · 近 ${rangeLabel}`}
          color={snapshot ? PRESSURE_COLOR[snapshot.memory.pressure] : IDLE_STROKE}
          points={windowed.map((p) => p.memory)}
        />
        <div data-mini="alert" style={{ marginLeft: "auto", fontSize: 12 }}>
          {!alertConfig.enabled ? (
            <Text type="secondary">告警已静默：后端不再判定，这里也不会补报</Text>
          ) : latestAlert ? (
            <Text style={{ color: ALERT_LEVEL_COLORS[latestAlert.level] }}>
              {ALERT_LEVEL_LABELS[latestAlert.level] ?? latestAlert.level} ·{" "}
              {ALERT_METRIC_LABELS[latestAlert.metric] ?? latestAlert.metric}{" "}
              {latestAlert.value.toFixed(1)} %（
              {dayjs(latestAlert.timestampMs).format("HH:mm:ss")}）
            </Text>
          ) : (
            <Text type="secondary">后端未推送越限告警</Text>
          )}
        </div>
      </div>

      <div style={{ display: "flex", gap: 10, flex: 1, minHeight: 0 }}>
        <Panel title="每核心" testId="cores">
          {perCore.length ? (
            <div style={{ display: "flex", alignItems: "flex-end", gap: 3, height: 96 }}>
              {perCore.map((value, index) => (
                <Tooltip key={index} title={`核心 ${index}：${value.toFixed(1)} %`}>
                  <div
                    data-mini="core-bar"
                    style={{
                      width: 10,
                      height: `${Math.max(2, Math.min(value, 100))}%`,
                      background: usageColor(value, alertConfig.cpu.warning),
                      borderRadius: 2,
                    }}
                  />
                </Tooltip>
              ))}
            </div>
          ) : (
            <Text type="secondary">后端本帧未给每核心数据</Text>
          )}
        </Panel>
        <Panel title="磁盘分区" testId="disks">
          {usableDisks.length ? (
            usableDisks.map((d) => (
              <div key={d.mountPoint} data-mini="disk-row" style={rowStyle}>
                <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                  {d.mountPoint}
                </span>
                <span style={{ color: usageColor(d.usagePercent, alertConfig.disk.warning) }}>
                  {formatPercent(d.usagePercent)} · 读 {formatRate(d.readBytesPerSec)} / 写{" "}
                  {formatRate(d.writeBytesPerSec)}
                </span>
              </div>
            ))
          ) : (
            <Text type="secondary">无可用分区</Text>
          )}
        </Panel>
        <Panel title="网络接口" testId="networks">
          {usableNetworks.length ? (
            usableNetworks.map((n) => (
              <div key={n.interface} data-mini="net-row" style={rowStyle}>
                <span>
                  {n.interface}
                  {n.status === "down" ? "（未连接）" : ""}
                </span>
                <span>
                  ↓ {formatRate(n.rxBytesPerSec)} · ↑ {formatRate(n.txBytesPerSec)}
                </span>
              </div>
            ))
          ) : (
            <Text type="secondary">后端本帧未给接口数据</Text>
          )}
        </Panel>
      </div>

      {!snapshot && (
        <Text type="secondary" style={{ fontSize: 12 }} data-mini="empty-hint">
          等待后端首帧：以上读数全部是占位符，迷你模式不会显示推测值或 0 值。
        </Text>
      )}
    </div>
  );
}
