import { Button, Col, Row, Segmented, Space, Typography } from "antd";
import { CloudOutlined, DesktopOutlined, HddOutlined, WifiOutlined } from "@ant-design/icons";
import { useMemo } from "react";
import type { AlertEvent, HistoryPoint, MetricsSnapshot, StaticInfo } from "../../ipc_contract";
import type { HistorySeedInfo, NetSessionTotals } from "../../hooks/useSystemMonitor";
import { formatBytes, formatGb, formatPercent, formatRate } from "../../lib/format";
import {
  TREND_RANGES,
  alignAlertMarkers,
  resampleEnvelope,
  toChartRows,
  windowHistory,
} from "../../lib/trend";
import { sumRates } from "../../lib/metrics_math";
import { CpuCores } from "../CpuCores";
import { MetricCard } from "../MetricCard";
import { TrendChart } from "../TrendChart";

const { Text } = Typography;

export function OverviewTab({
  staticInfo,
  snapshot,
  history,
  trendRange,
  onTrendRangeChange,
  seedInfo,
  alertEvents,
  netSession,
  onExportCsv,
  csvExporting,
}: {
  staticInfo: StaticInfo | null;
  snapshot: MetricsSnapshot | null;
  history: HistoryPoint[];
  trendRange: number;
  onTrendRangeChange: (seconds: number) => void;
  seedInfo: HistorySeedInfo | null;
  /** 已触发的告警（会话内 + 落盘回填），用来在曲线上标竖线 */
  alertEvents: AlertEvent[];
  /** 本次会话累计收发（按累计计数器差分，接口复位时重算基线） */
  netSession: NetSessionTotals | null;
  /** 导出当前范围的历史趋势为 CSV（02 的 F5"可导出 CSV"） */
  onExportCsv: () => void;
  csvExporting: boolean;
}) {
  const cpu = snapshot?.cpu;
  const memory = snapshot?.memory;

  const networkTotal = useMemo(() => {
    if (!snapshot) return { rx: 0, tx: 0, ipv4: null as string | null };
    const active = snapshot.networks.filter((n) => n.status === "up" && n.ipv4);
    const pool = active.length ? active : snapshot.networks;
    return {
      rx: sumRates(pool.map((n) => n.rxBytesPerSec)),
      tx: sumRates(pool.map((n) => n.txBytesPerSec)),
      ipv4: pool.find((n) => n.ipv4)?.ipv4 ?? null,
    };
  }, [snapshot]);

  const diskTotals = useMemo(() => {
    const internal =
      snapshot?.disks.filter((d) => d.mountPoint === "/" || d.totalBytes > 0) ?? [];
    const total = internal.reduce((a, d) => a + d.totalBytes, 0);
    const used = internal.reduce((a, d) => a + d.usedBytes, 0);
    return { total, used, percent: total > 0 ? (used / total) * 100 : 0 };
  }, [snapshot]);

  // 14 400 点的窗口裁剪 + 降采样只在数据或窗口真的变了才算：本组件每一帧都重渲染，
  // 不 memo 就等于每秒白烧一次 CPU（迷你模式同一套理由，见 MiniMenuBar）
  const windowed = useMemo(() => windowHistory(history, trendRange), [history, trendRange]);
  // 三条曲线各自降采样：极值落在哪一帧是每条指标自己的事，共用一套行会把尖峰抹平
  const cpuRows = useMemo(
    () => toChartRows(resampleEnvelope(windowed, (p) => p.cpu)),
    [windowed]
  );
  const memoryRows = useMemo(
    () => toChartRows(resampleEnvelope(windowed, (p) => p.memory)),
    [windowed]
  );

  // 告警竖线只能落在本条曲线真实存在的那一行上，所以 markers 与曲线共用同一批 time 字符串
  const cpuMarkers = useMemo(
    () => alignAlertMarkers(alertEvents, cpuRows, "cpu"),
    [alertEvents, cpuRows]
  );
  const memoryMarkers = useMemo(
    () => alignAlertMarkers(alertEvents, memoryRows, "memory"),
    [alertEvents, memoryRows]
  );
  const markerSkewSecs = Math.round(
    Math.max(cpuMarkers.maxSkewMs, memoryMarkers.maxSkewMs) / 1000
  );
  const markerOutside = cpuMarkers.outside + memoryMarkers.outside;

  // 曲线的来源必须写在脸上：跨重启回填的点是 10 s（或按桶均值）采样，
  // 与实时链路的 1 s 点混在一张图上，不标注就会让人误读分辨率。
  const trendSourceText = seedInfo?.error
    ? `历史存储不可用：${seedInfo.error}`
    : seedInfo && seedInfo.points > 0
      ? `含 ${seedInfo.points} 个落盘历史点（${seedInfo.bucketSeconds} s 一点）`
      : "仅本次会话实时采集";

  return (
    <>
      <Row gutter={[16, 16]} style={{ marginBottom: 16 }}>
        <Col span={6}>
          <MetricCard
            icon={<DesktopOutlined />}
            title="CPU 使用率"
            value={cpu ? formatPercent(cpu.total) : "—"}
            percent={cpu?.total}
            footer={
              staticInfo ? `${staticInfo.cpuModel} (${staticInfo.coreCount} 核心)` : "读取中…"
            }
          />
        </Col>
        <Col span={6}>
          <MetricCard
            icon={<CloudOutlined />}
            title="内存使用"
            value={memory ? formatGb(memory.usedBytes) : "—"}
            unit="GB"
            percent={memory?.usagePercent}
            footer={
              memory ? `共 ${formatGb(memory.totalBytes)} · ${memory.pressure}` : "等待首帧…"
            }
          />
        </Col>
        <Col span={6}>
          <MetricCard
            icon={<HddOutlined />}
            title="磁盘使用"
            value={snapshot ? formatGb(diskTotals.used) : "—"}
            unit="GB"
            percent={snapshot ? diskTotals.percent : undefined}
            footer={snapshot ? `共 ${formatGb(diskTotals.total)}` : "等待首帧…"}
          />
        </Col>
        <Col span={6}>
          <MetricCard
            icon={<WifiOutlined />}
            title="网络流量"
            value={snapshot ? `${formatBytes(networkTotal.rx, 0)}/s` : "—"}
            footer={
              snapshot
                ? `↓ ${formatRate(networkTotal.rx)} · ↑ ${formatRate(networkTotal.tx)} · ${
                    networkTotal.ipv4 ?? "无 IPv4"
                  } · 会话累计 ↓${formatBytes(netSession?.rxBytes ?? 0, 1)} ↑${formatBytes(
                    netSession?.txBytes ?? 0,
                    1
                  )}`
                : "等待首帧…"
            }
          />
        </Col>
      </Row>

      <Row gutter={[16, 16]} style={{ marginBottom: 12, marginTop: 4 }}>
        <Col span={24} style={{ display: "flex", justifyContent: "flex-end" }}>
          <Space size={8}>
            <Text type="secondary" style={{ fontSize: 12 }}>
              趋势范围（窗口内 {windowed.length} 个原始点 → 图上 {cpuRows.length} 个点，取每桶最低/最高，尖峰不被均值抹平 · {trendSourceText}）
            </Text>
            <Segmented
              size="small"
              value={trendRange}
              onChange={(v) => onTrendRangeChange(Number(v))}
              options={TREND_RANGES.map((r) => ({ label: r.label, value: r.value }))}
            />
            <Button
              size="small"
              loading={csvExporting}
              onClick={onExportCsv}
              title="列为 timestamp_ms / cpu_percent / memory_percent / rx_bytes_per_sec / tx_bytes_per_sec / bucket_seconds；每行自带桶宽，最后一列等于 10 才是原始采样点。全是指势数值，不含路径与进程名。"
            >
              导出 CSV
            </Button>
          </Space>
        </Col>
      </Row>

      <Row gutter={[16, 16]}>
        <Col span={12}>
          <TrendChart
            title="CPU 使用率趋势"
            data={cpuRows}
            dataKey="value"
            color="#667eea"
            gradientId="colorCpu"
            yMax={100}
            unitFormatter={(v) => `${v.toFixed(1)}%`}
            markers={cpuMarkers.markers}
            markerSkewMs={cpuMarkers.maxSkewMs}
          />
        </Col>
        <Col span={12}>
          <TrendChart
            title="内存使用趋势"
            data={memoryRows}
            dataKey="value"
            color="#764ba2"
            yMax={100}
            gradientId="colorMemory"
            unitFormatter={(v) => `${v.toFixed(1)}%`}
            markers={memoryMarkers.markers}
            markerSkewMs={memoryMarkers.maxSkewMs}
          />
        </Col>
      </Row>

      {(cpuMarkers.markers.length > 0 ||
        memoryMarkers.markers.length > 0 ||
        markerOutside > 0) && (
        <Row style={{ marginTop: 8 }}>
          <Col span={24}>
            <Text type="secondary" style={{ fontSize: 12 }}>
              竖线是该指标已触发的告警时刻，对齐到最近的采样点（本图定位误差 ≤{" "}
              {markerSkewSecs} s）；同一格多条合成一根。
              {markerOutside > 0 &&
                ` 另有 ${markerOutside} 条落在当前趋势窗口之外，图上不画。`}
              {" 磁盘告警不画在此图 —— 这两张图没有磁盘曲线。"}
            </Text>
          </Col>
        </Row>
      )}

      {cpu && cpu.perCore.length > 1 ? (
        <Row gutter={[16, 16]} style={{ marginTop: 16 }}>
          <Col span={24}>
            <CpuCores perCore={cpu.perCore} />
          </Col>
        </Row>
      ) : null}
    </>
  );
}
