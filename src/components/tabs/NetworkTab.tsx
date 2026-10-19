import { useEffect, useMemo, useState } from "react";
import { Card, Col, Descriptions, Row, Table, Tag, Typography } from "antd";
import type { MetricsSnapshot, NetworkMetrics } from "../../ipc_contract";
import { formatBytes, formatRate } from "../../lib/format";
import { monitorCardStyle } from "../../lib/ui";
import { networkRows, summarizeNetwork, type NetworkSeries } from "../../lib/networkSeries";
import { TrendChart } from "../TrendChart";

const { Text } = Typography;

interface Props {
  snapshot: MetricsSnapshot | null;
  /** per-interface 时序（由 `useNetworkSeries` 维护） */
  series: NetworkSeries;
}

/**
 * 网络 Tab（T3-02）：per-interface **实时曲线** + **接口详情**。
 *
 * 之前这里只有一张接口表 —— 后端落盘的历史只有**聚合**上下行速率，
 * 所以「是哪个网卡在跑流量」这个问题在界面上无处可答。
 * 现在每个接口各自一条曲线，选中哪个接口就画哪个的。
 */
export function NetworkTab({ snapshot, series }: Props) {
  // 用 useMemo 稳定引用：snapshot 为 null 时 `?? []` 每次渲染都是新数组，
  // 会让下面的选中回落 effect 每渲染都跑一遍。
  const nics = useMemo(() => snapshot?.networks ?? [], [snapshot]);
  const [selected, setSelected] = useState<string | null>(null);

  // 选中的接口消失时（网卡被拔掉 / 容器退掉）要落到还存在的那个上，
  // 否则详情区会一直显示一个已经不存在的接口的旧数据。
  useEffect(() => {
    if (!nics.length) {
      setSelected(null);
      return;
    }
    if (!selected || !nics.some((n) => n.interface === selected)) {
      setSelected(nics[0].interface);
    }
  }, [nics, selected]);

  const current: NetworkMetrics | null = useMemo(
    () => nics.find((n) => n.interface === selected) ?? null,
    [nics, selected]
  );

  const points = useMemo(
    () => (selected ? series.get(selected) ?? [] : []),
    [series, selected]
  );
  const rows = useMemo(() => networkRows(points), [points]);
  const summary = useMemo(() => summarizeNetwork(points), [points]);

  return (
    <Row gutter={[16, 16]}>
      <Col span={24}>
        <Card title="网络接口" className="monitor-card" style={{ ...monitorCardStyle }}>
          <Table
            rowKey="interface"
            columns={[
              { title: "接口", dataIndex: "interface", key: "interface", width: 120 },
              {
                title: "状态",
                dataIndex: "status",
                key: "status",
                width: 90,
                render: (s: string) => (
                  <Tag color={s === "up" ? "success" : s === "down" ? "error" : "default"}>
                    {s === "up" ? "在线" : s === "down" ? "离线" : "未知"}
                  </Tag>
                ),
              },
              {
                title: "IPv4",
                dataIndex: "ipv4",
                key: "ipv4",
                render: (v: string | null) => v ?? "—",
              },
              {
                title: "IPv6",
                dataIndex: "ipv6",
                key: "ipv6",
                ellipsis: true,
                render: (v: string | null) => v ?? "—",
              },
              {
                title: "MAC",
                dataIndex: "mac",
                key: "mac",
                width: 160,
                render: (v: string | null) => (v ? <Text code>{v}</Text> : "—"),
              },
              {
                title: "下行",
                dataIndex: "rxBytesPerSec",
                key: "rx",
                width: 120,
                render: (v: number) => formatRate(v),
              },
              {
                title: "上行",
                dataIndex: "txBytesPerSec",
                key: "tx",
                width: 120,
                render: (v: number) => formatRate(v),
              },
            ]}
            dataSource={nics}
            size="small"
            pagination={false}
            locale={{ emptyText: "等待采集首帧…" }}
            rowClassName={(r) => (r.interface === selected ? "net-row-selected" : "")}
            onRow={(r) => ({
              onClick: () => setSelected(r.interface),
              style: { cursor: "pointer" },
            })}
          />
        </Card>
      </Col>

      <Col span={24}>
        <Card
          title={current ? `接口详情 · ${current.interface}` : "接口详情"}
          className="monitor-card"
          style={{ ...monitorCardStyle }}
        >
          {current ? (
            <Descriptions size="small" column={4} bordered>
              <Descriptions.Item label="状态">
                {current.status === "up" ? "在线" : current.status === "down" ? "离线" : "未知"}
              </Descriptions.Item>
              <Descriptions.Item label="IPv4">{current.ipv4 ?? "—"}</Descriptions.Item>
              <Descriptions.Item label="IPv6">{current.ipv6 ?? "—"}</Descriptions.Item>
              <Descriptions.Item label="MAC">{current.mac ?? "—"}</Descriptions.Item>
              <Descriptions.Item label="当前下行">{formatRate(current.rxBytesPerSec)}</Descriptions.Item>
              <Descriptions.Item label="当前上行">{formatRate(current.txBytesPerSec)}</Descriptions.Item>
              <Descriptions.Item label="窗口峰值">
                ↓ {formatRate(summary.peakRx)} / ↑ {formatRate(summary.peakTx)}
              </Descriptions.Item>
              <Descriptions.Item label="窗口均值">
                ↓ {formatRate(summary.avgRx)} / ↑ {formatRate(summary.avgTx)}
              </Descriptions.Item>
              <Descriptions.Item label="累计接收">
                {formatBytes(current.totalReceivedBytes)}（{current.packetsReceived} 包）
              </Descriptions.Item>
              <Descriptions.Item label="累计发送">
                {formatBytes(current.totalTransmittedBytes)}（{current.packetsTransmitted} 包）
              </Descriptions.Item>
              <Descriptions.Item label="采样点">{summary.count} 个</Descriptions.Item>
              <Descriptions.Item label="曲线口径">
                前端实时采样（后端落盘历史只有聚合上下行，不含按接口拆分）
              </Descriptions.Item>
            </Descriptions>
          ) : (
            <Text type="secondary">等待采集首帧…</Text>
          )}
        </Card>
      </Col>

      <Col span={12}>
        <TrendChart
          title={selected ? `${selected} 下行速率` : "下行速率"}
          data={rows as unknown as Array<Record<string, number | string>>}
          dataKey="rx"
          color="#36cfc9"
          gradientId={`colorNetRx-${selected ?? "none"}`}
          unitFormatter={(v) => formatRate(v)}
        />
      </Col>
      <Col span={12}>
        <TrendChart
          title={selected ? `${selected} 上行速率` : "上行速率"}
          data={rows as unknown as Array<Record<string, number | string>>}
          dataKey="tx"
          color="#faad14"
          gradientId={`colorNetTx-${selected ?? "none"}`}
          unitFormatter={(v) => formatRate(v)}
        />
      </Col>
    </Row>
  );
}
