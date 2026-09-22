import { Alert, Button, Card, Input, Select, Space, Table, Typography } from "antd";
import { ReloadOutlined, SearchOutlined, StopOutlined } from "@ant-design/icons";
import type { ProcessInfo, ProcessPage, ProcessSort } from "../../ipc_contract";
import { formatBytes, formatPercent, formatUptime, usageColor } from "../../lib/format";
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
}: Props) {
  const pageCount = Math.max(1, Math.ceil(page.total / pageSize));
  const rows = page.items;
  // 只有真的铺到几十上百行时才值得开虚拟滚动：它要固定行高与列宽，小页签没必要付这个代价。
  const virtual = rows.length > PROCESS_VIRTUAL_THRESHOLD;

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
    <Card
      title={`进程列表（命中 ${page.total}，按${PROCESS_SORT_LABELS[sort.by]}${sort.desc ? "↓" : "↑"}）`}
      className="monitor-card"
      extra={
        <Space>
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
        scroll={virtual ? { x: 830, y: 560 } : undefined}
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
        locale={{ emptyText: "没有匹配的进程" }}
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
  );
}
