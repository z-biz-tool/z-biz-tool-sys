import { Col, Row, Segmented, Space, Typography } from "antd";
import { CloudOutlined, DesktopOutlined, HddOutlined, WifiOutlined } from "@ant-design/icons";
import dayjs from "dayjs";
import { useMemo } from "react";
import type { HistoryPoint, MetricsSnapshot, StaticInfo } from "../../ipc_contract";
import type { HistorySeedInfo } from "../../hooks/useSystemMonitor";
import { formatBytes, formatGb, formatPercent, formatRate } from "../../lib/format";
import { TREND_RANGES, downsampleHistory, windowHistory } from "../../lib/trend";
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
}: {
  staticInfo: StaticInfo | null;
  snapshot: MetricsSnapshot | null;
  history: HistoryPoint[];
  trendRange: number;
  onTrendRangeChange: (seconds: number) => void;
  seedInfo: HistorySeedInfo | null;
}) {
  const cpu = snapshot?.cpu;
  const memory = snapshot?.memory;

  const networkTotal = useMemo(() => {
    if (!snapshot) return { rx: 0, tx: 0, ipv4: null as string | null };
    const active = snapshot.networks.filter((n) => n.status === "up" && n.ipv4);
    const pool = active.length ? active : snapshot.networks;
    return {
      rx: pool.reduce((a, n) => a + n.rxBytesPerSec, 0),
      tx: pool.reduce((a, n) => a + n.txBytesPerSec, 0),
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

  const chartData = useMemo(
    () =>
      downsampleHistory(windowHistory(history, trendRange)).map((p) => ({
        time: dayjs(p.t).format("HH:mm:ss"),
        cpu: Number(p.cpu.toFixed(2)),
        memory: Number(p.memory.toFixed(2)),
        network: Number(((p.rxBytesPerSec + p.txBytesPerSec) / 1024).toFixed(2)),
      })),
    [history, trendRange]
  );

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
                  }`
                : "等待首帧…"
            }
          />
        </Col>
      </Row>

      <Row gutter={[16, 16]} style={{ marginBottom: 12, marginTop: 4 }}>
        <Col span={24} style={{ display: "flex", justifyContent: "flex-end" }}>
          <Space size={8}>
            <Text type="secondary" style={{ fontSize: 12 }}>
              趋势范围（仅显示已采集的 {chartData.length} 个点 · {trendSourceText}）
            </Text>
            <Segmented
              size="small"
              value={trendRange}
              onChange={(v) => onTrendRangeChange(Number(v))}
              options={TREND_RANGES.map((r) => ({ label: r.label, value: r.value }))}
            />
          </Space>
        </Col>
      </Row>

      <Row gutter={[16, 16]}>
        <Col span={12}>
          <TrendChart
            title="CPU 使用率趋势"
            data={chartData}
            dataKey="cpu"
            color="#667eea"
            gradientId="colorCpu"
            yMax={100}
            unitFormatter={(v) => `${v.toFixed(1)}%`}
          />
        </Col>
        <Col span={12}>
          <TrendChart
            title="内存使用趋势"
            data={chartData}
            dataKey="memory"
            color="#764ba2"
            yMax={100}
            gradientId="colorMemory"
            unitFormatter={(v) => `${v.toFixed(1)}%`}
          />
        </Col>
      </Row>

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
