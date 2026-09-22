import { Alert, Button, Descriptions, Drawer, Space, Tag, Typography } from "antd";
import dayjs from "dayjs";
import type { ProcessDetail } from "../ipc_contract";
import { formatBytes, formatPercent, formatUptime } from "../lib/format";

const { Text } = Typography;

interface Props {
  pid: number | null;
  detail: ProcessDetail | null;
  loading: boolean;
  onClose: () => void;
  onOpenPid: (pid: number) => void;
}

const NO_ACCESS = "—（权限不足或系统不暴露）";

export function ProcessDetailDrawer({ pid, detail, loading, onClose, onOpenPid }: Props) {
  return (
    <Drawer
      open={pid !== null}
      onClose={onClose}
      size={560}
      title={detail ? `进程详情 · ${detail.name}` : "进程详情"}
    >
      {loading ? (
        <Alert type="info" showIcon title="正在读取进程详情…" />
      ) : detail ? (
        <Descriptions
          column={1}
          size="small"
          bordered
          items={[
            {
              key: "pid",
              label: "PID",
              children: (
                <Space size={6}>
                  <Text code>{detail.pid}</Text>
                  {detail.isSelf ? <Tag color="blue">本应用</Tag> : null}
                </Space>
              ),
            },
            { key: "status", label: "状态", children: detail.status },
            {
              key: "parent",
              label: "父进程",
              children:
                detail.parentPid === null ? (
                  "—"
                ) : (
                  <Button
                    type="link"
                    size="small"
                    style={{ padding: 0 }}
                    onClick={() => onOpenPid(detail.parentPid as number)}
                  >
                    PID {detail.parentPid}
                  </Button>
                ),
            },
            { key: "cpu", label: "CPU", children: formatPercent(detail.cpuUsage) },
            { key: "memory", label: "内存", children: formatBytes(detail.memoryBytes, 1) },
            {
              key: "uptime",
              label: "运行时长",
              children: formatUptime(detail.runTimeSeconds),
            },
            {
              key: "start",
              label: "启动时刻",
              children:
                detail.startTime > 0
                  ? dayjs(detail.startTime * 1000).format("YYYY-MM-DD HH:mm:ss")
                  : "—（sysinfo 未提供）",
            },
            {
              key: "exe",
              label: "可执行路径",
              children: detail.exePath ? (
                <Text
                  copyable={{ text: detail.exePath }}
                  style={{ fontSize: 12, wordBreak: "break-all" }}
                >
                  {detail.exePath}
                </Text>
              ) : (
                NO_ACCESS
              ),
            },
            {
              key: "cwd",
              label: "工作目录",
              children: detail.cwd ? (
                <Text style={{ fontSize: 12, wordBreak: "break-all" }}>{detail.cwd}</Text>
              ) : (
                NO_ACCESS
              ),
            },
          ]}
        />
      ) : (
        <Alert type="warning" showIcon title="该进程已退出，或无权读取详情" />
      )}
    </Drawer>
  );
}
