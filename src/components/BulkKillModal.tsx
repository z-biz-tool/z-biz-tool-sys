import { Alert, Modal, Space, Table, Tag, Typography } from "antd";
import { formatBytes } from "../lib/format";

const { Text } = Typography;

/** 一个目标进程：确认前只有名字，跑完才有结论 */
export interface BulkKillTarget {
  pid: number;
  name: string;
  memoryBytes: number | null;
}

export interface BulkKillRow extends BulkKillTarget {
  done: boolean;
  ok: boolean;
  /** 成功时：是否 SIGTERM 就退了（false 表示升级到 SIGKILL） */
  graceful: boolean | null;
  /** 失败时后端给的原因原文，不在这儿再编一套 */
  reason: string | null;
}

interface Props {
  open: boolean;
  targets: BulkKillTarget[];
  rows: BulkKillRow[];
  running: boolean;
  onConfirm: () => void;
  onCancel: () => void;
  onAfterClose: () => void;
}

/**
 * 批量结束的确认 + 逐条结果。
 *
 * 两条刻意的规矩：
 * 1. 确认框里必须把**每一个 pid 和进程名**摊出来 —— 勾选跨页保留，人很容易忘记自己选过什么。
 * 2. 结果按进程逐条报，不做"整批失败就一句错误"：一个 PID 已退出不该让另外九个的结论消失。
 *    该不该碰（本应用自身、PID ≤ 100、系统关键进程）由后端 `validate_kill` 判，这里只转述它的话，
 *    省得在两处各维护一份受保护名单然后等它俩漂移。
 */
export function BulkKillModal({
  open,
  targets,
  rows,
  running,
  onConfirm,
  onCancel,
  onAfterClose,
}: Props) {
  const started = rows.length > 0;
  const succeeded = rows.filter((r) => r.ok).length;
  const failed = rows.filter((r) => !r.ok).length;
  return (
    <Modal
      open={open}
      title={`结束所选 ${targets.length} 个进程`}
      onCancel={onCancel}
      onOk={onConfirm}
      okText={started ? "完成" : `确认结束 ${targets.length} 个`}
      okButtonProps={{ danger: !started, loading: running }}
      afterClose={onAfterClose}
      width={640}
    >
      {!started ? (
        <Space direction="vertical" size="small" style={{ width: "100%" }}>
          <Text type="danger">
            将逐个发送 SIGTERM，未响应的升级为 SIGKILL；未保存的数据会丢失。
          </Text>
          <Text type="secondary">
            本应用自身进程、PID ≤ 100 的系统进程区间与关键系统进程会被后端逐条拒掉，
            拒因写在结果里 —— 其余进程即使属于其他用户也可能只是权限不足，不会提权。
          </Text>
          <Table
            rowKey="pid"
            size="small"
            pagination={false}
            dataSource={targets}
            columns={[
              { title: "PID", dataIndex: "pid", key: "pid", width: 90 },
              {
                title: "进程名",
                dataIndex: "name",
                key: "name",
                ellipsis: true,
                render: (name: string) => <Text strong>{name}</Text>,
              },
              {
                title: "内存",
                dataIndex: "memoryBytes",
                key: "memoryBytes",
                width: 110,
                render: (v: number | null) => (v === null ? "—" : formatBytes(v, 1)),
              },
            ]}
          />
        </Space>
      ) : (
        <Space direction="vertical" size="small" style={{ width: "100%" }}>
          <Alert
            type={failed ? "warning" : "info"}
            showIcon
            title={`已结束 ${succeeded} 个，未成功 ${failed} 个（共 ${targets.length} 个）`}
          />
          <Table
            rowKey="pid"
            size="small"
            pagination={false}
            dataSource={rows}
            columns={[
              { title: "PID", dataIndex: "pid", key: "pid", width: 90 },
              { title: "进程名", dataIndex: "name", key: "name", ellipsis: true },
              {
                title: "结果",
                key: "result",
                width: 220,
                render: (_: unknown, row: BulkKillRow) =>
                  !row.done ? (
                    <Tag>进行中</Tag>
                  ) : row.ok ? (
                    <Tag color="green">{row.graceful ? "SIGTERM 退出" : "升级为 SIGKILL"}</Tag>
                  ) : (
                    <Tag color="red">未结束</Tag>
                  ),
              },
              {
                title: "说明",
                dataIndex: "reason",
                key: "reason",
                ellipsis: true,
                render: (reason: string | null) => reason ?? "—",
              },
            ]}
          />
        </Space>
      )}
    </Modal>
  );
}
