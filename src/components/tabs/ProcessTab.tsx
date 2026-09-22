import { Alert, Button, Card, Input, Space, Table, Typography } from "antd";
import { ReloadOutlined, SearchOutlined, StopOutlined } from "@ant-design/icons";
import type { ProcessInfo, ProcessPage, ProcessSort } from "../../ipc_contract";
import { formatBytes, formatPercent, formatUptime, usageColor } from "../../lib/format";
import { PROCESS_PAGE_SIZE, PROCESS_SORT_LABELS } from "../../lib/ui";

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
  onPageChange,
  onRefresh,
  onOpenDetail,
  onRequestKill,
}: Props) {
  const pageCount = Math.max(1, Math.ceil(page.total / PROCESS_PAGE_SIZE));
  const rows = page.items;

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
          {page.total === 0 ? "无数据" : `${offset + 1}–${offset + rows.length} 条`}
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
