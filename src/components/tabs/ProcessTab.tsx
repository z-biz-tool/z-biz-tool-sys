import { Alert, Button, Card, Input, Select, Space, Switch, Table, Tag, Tooltip, Typography } from "antd";
import { ReloadOutlined, SearchOutlined, StopOutlined } from "@ant-design/icons";
import { useMemo } from "react";
import type {
  ListeningReport,
  ProcessInfo,
  ProcessPage,
  ProcessRollup,
  ProcessSort,
} from "../../ipc_contract";
import { formatBytes, formatPercent, formatUptime, usageColor } from "../../lib/format";
import { ListeningPortsPanel } from "../ListeningPortsPanel";
import { ProcessRollupPanel } from "../ProcessRollupPanel";
import {
  PROCESS_PAGE_SIZE_OPTIONS,
  PROCESS_SORT_LABELS,
  PROCESS_VIRTUAL_THRESHOLD,
} from "../../lib/ui";

const { Text } = Typography;

export interface ProcessSortState {
  by: ProcessSort;
  desc: boolean;
}

interface Props {
  page: ProcessPage;
  streaming: boolean;
  hasSnapshot: boolean;
  keyword: string;
  onKeywordChange: (value: string) => void;
  sort: ProcessSortState;
  onSortChange: (next: ProcessSortState) => void;
  pageNumber: number;
  offset: number;
  pageSize: number;
  onPageSizeChange: (next: number) => void;
  onPageChange: (next: number) => void;
  onRefresh: () => void;
  onOpenDetail: (pid: number) => void;
  onRequestKill: (pid: number) => void;
  /** 只看"父进程已是 launchd"的行（后端筛选，不是前端过滤当前页） */
  onlyDetached: boolean;
  onOnlyDetachedChange: (next: boolean) => void;
  /** 系统性观察两份载荷：null 都表示"这一轮没读到"，与"读到了但是空的"是不同状态 */
  rollup: ProcessRollup[] | null;
  listeners: ListeningReport | null;
  observationLoading: boolean;
  onRefreshObservation: () => void;
}

/** 排序、过滤、分页都在后端执行，这里只映射表头指示与当前页窗口 */
export function ProcessTab({
  page,
  streaming,
  hasSnapshot,
  keyword,
  onKeywordChange,
  sort,
  onSortChange,
  pageNumber,
  offset,
  pageSize,
  onPageSizeChange,
  onPageChange,
  onRefresh,
  onOpenDetail,
  onRequestKill,
  onlyDetached,
  onOnlyDetachedChange,
  rollup,
  listeners,
  observationLoading,
  onRefreshObservation,
}: Props) {
  const pageCount = Math.max(1, Math.ceil(page.total / pageSize));
  const rows = page.items;
  // 只有真的铺到几十上百行时才值得开虚拟滚动：它要固定行高与列宽，小页签没必要付这个代价。
  const virtual = rows.length > PROCESS_VIRTUAL_THRESHOLD;

  // pid → 监听端口。反查方向（端口→进程）在下面的端口面板里，这一列给的是正查，
  // 两边同一份数据，不会出现"面板说 18090 是 java、表里那行却没端口"。
  const portsByPid = useMemo(() => {
    const map = new Map<number, number[]>();
    for (const socket of listeners?.sockets ?? []) {
      const list = map.get(socket.pid);
      if (list) list.push(socket.port);
      else map.set(socket.pid, [socket.port]);
    }
    return map;
  }, [listeners]);

  const orderFor = (by: ProcessSort) =>
    sort.by === by ? (sort.desc ? ("descend" as const) : ("ascend" as const)) : undefined;

  const columns = [
    {
      title: "PID",
      dataIndex: "pid",
      key: "pid",
      width: 90,
      sorter: true,
      sortOrder: orderFor("pid"),
    },
    {
      title: "进程名",
      dataIndex: "name",
      key: "name",
      ellipsis: true,
      width: 260,
      sorter: true,
      sortOrder: orderFor("name"),
      render: (name: string) => <Text strong>{name}</Text>,
    },
    {
      title: "CPU",
      dataIndex: "cpuUsage",
      key: "cpu",
      width: 110,
      sorter: true,
      sortOrder: orderFor("cpu"),
      render: (value: number) =>
        page.warming ? (
          <Text type="secondary">采样中…</Text>
        ) : (
          <span style={{ color: usageColor(value, 80) }}>{formatPercent(value)}</span>
        ),
    },
    {
      title: "内存",
      dataIndex: "memoryBytes",
      key: "memory",
      width: 130,
      sorter: true,
      sortOrder: orderFor("memory"),
      render: (value: number) => formatBytes(value, 1),
    },
    {
      title: "运行时长",
      dataIndex: "runTimeSeconds",
      key: "runTimeSeconds",
      width: 140,
      render: (value: number | null) => formatUptime(value ?? 0),
    },
    {
      title: "端口",
      key: "ports",
      width: 120,
      render: (_: unknown, record: ProcessInfo) => {
        const ports = portsByPid.get(record.pid);
        if (!ports || ports.length === 0) return <Text type="secondary">—</Text>;
        const shown = ports.slice(0, 3).join(", ");
        return (
          <Tooltip title={`TCP LISTEN：${ports.join(", ")}`}>
            <Text style={{ fontSize: 12 }}>
              {shown}
              {ports.length > 3 ? ` +${ports.length - 3}` : ""}
            </Text>
          </Tooltip>
        );
      },
    },
    {
      title: "属主",
      dataIndex: "ownedByCurrentUser",
      key: "ownedByCurrentUser",
      width: 110,
      // false 有两种含义（确实是别人的进程 / 这一轮读不到 uid），所以文案说"其他或未知"，
      // 不说"root 进程"——后者是本列给不出的结论。
      render: (value: boolean) =>
        value ? (
          <Tag color="blue">本用户</Tag>
        ) : (
          <Tooltip title="可能是其他用户的进程，也可能是本进程无权读取 uid 的系统进程">
            <Tag>其他/未知</Tag>
          </Tooltip>
        ),
    },
    {
      title: "脱离",
      dataIndex: "detached",
      key: "detached",
      width: 90,
      render: (value: boolean) =>
        value ? (
          <Tooltip title="父进程已经是 launchd：当初启动它的那个 shell/程序已退出。系统守护进程也长这样，所以这是线索不是判决。">
            <Tag color="orange">launchd</Tag>
          </Tooltip>
        ) : (
          <Text type="secondary">—</Text>
        ),
    },
    {
      title: "操作",
      key: "action",
      width: 100,
      render: (_: unknown, record: ProcessInfo) => (
        <Button
          type="link"
          danger
          icon={<StopOutlined />}
          size="small"
          onClick={(e) => {
            // 行本身会打开详情抽屉，操作按钮必须吃掉冒泡
            e.stopPropagation();
            onRequestKill(record.pid);
          }}
        >
          结束
        </Button>
      ),
    },
  ];

  return (
    <Space direction="vertical" size={16} style={{ width: "100%" }}>
      <ProcessRollupPanel
        rollup={rollup}
        loading={observationLoading}
        onRefresh={onRefreshObservation}
        onOpenDetail={onOpenDetail}
      />
      <Card
        title={`进程列表（命中 ${page.total}，按${PROCESS_SORT_LABELS[sort.by]}${sort.desc ? "↓" : "↑"}）`}
        className="monitor-card"
        extra={
          <Space>
            <Tooltip title="只看父进程已经是 launchd 的行。筛选在后端做，所以是对全量进程筛，不是只筛当前这一页。">
              <Switch
                size="small"
                checked={onlyDetached}
                onChange={onOnlyDetachedChange}
                checkedChildren="只看脱离"
                unCheckedChildren="只看脱离"
              />
            </Tooltip>
            <Input
              allowClear
              prefix={<SearchOutlined />}
              placeholder="按名称或 PID 搜索全部进程"
              value={keyword}
              onChange={(e) => onKeywordChange(e.target.value)}
              style={{ width: 240 }}
            />
            <Select
              size="small"
              value={pageSize}
              style={{ width: 104 }}
              onChange={(next: number) => onPageSizeChange(next)}
              options={PROCESS_PAGE_SIZE_OPTIONS.map((n) => ({ value: n, label: `${n} 行/页` }))}
            />
            <Button icon={<ReloadOutlined />} onClick={onRefresh}>
              刷新
            </Button>
          </Space>
        }
      >
        {!streaming && !hasSnapshot ? <Alert type="info" showIcon title="正在建立进程采集…" /> : null}
        <Table<ProcessInfo>
          rowKey="pid"
          columns={columns}
          dataSource={rows}
          size="small"
          loading={rows.length === 0 && streaming}
          pagination={false}
          virtual={virtual}
          scroll={virtual ? { x: 1150, y: 560 } : { x: 1150 }}
          onRow={(record) => ({
            onClick: () => onOpenDetail(record.pid),
            style: { cursor: "pointer" },
          })}
          onChange={(_pagination, _filters, sorter) => {
            const picked = Array.isArray(sorter) ? sorter[0] : sorter;
            onSortChange({
              by: (picked?.columnKey ?? "cpu") as ProcessSort,
              desc: picked?.order !== "ascend",
            });
          }}
          locale={{ emptyText: onlyDetached ? "没有脱离启动者的进程" : "没有匹配的进程" }}
        />
        <Space style={{ marginTop: 12 }} size={8}>
          <Button size="small" disabled={pageNumber <= 1} onClick={() => onPageChange(pageNumber - 1)}>
            上一页
          </Button>
          <Text type="secondary" style={{ fontSize: 12 }}>
            第 {pageNumber} / {pageCount} 页 ·{" "}
            {page.total === 0 ? "无数据" : `${offset + 1}–${offset + rows.length} 条`} ·{" "}
            {virtual ? `可视窗口渲染（共 ${rows.length} 行数据）` : "整页渲染"}
          </Text>
          <Button
            size="small"
            disabled={pageNumber >= pageCount}
            onClick={() => onPageChange(pageNumber + 1)}
          >
            下一页
          </Button>
        </Space>
      </Card>
      <ListeningPortsPanel
        report={listeners}
        loading={observationLoading}
        onRefresh={onRefreshObservation}
        onOpenDetail={onOpenDetail}
      />
    </Space>
  );
}
