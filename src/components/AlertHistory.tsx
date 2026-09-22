import { Alert, Button, Segmented, Space, Tag, Tooltip, Typography } from "antd";
import { ReloadOutlined } from "@ant-design/icons";
import dayjs from "dayjs";
import { alertIdentity, ALERT_LEVEL_LABELS, ALERT_HISTORY_SPANS, alertText } from "../lib/alert";
import type { AlertEvent } from "../ipc_contract";
import type { AlertHistoryInfo } from "../hooks/useAlerts";

const { Text } = Typography;

interface Props {
  /** 会话内 + 落盘回填合并后的完整列表（倒序） */
  events: AlertEvent[];
  /** 只来自落盘文件的那些：界面上标"回填"，因为它的挂载点是脱敏过的 */
  persisted: AlertEvent[];
  info: AlertHistoryInfo | null;
  error: string | null;
  span: number;
  onSpanChange: (seconds: number) => void;
  onRefresh: () => void;
}

/**
 * 告警历史列表（T5-03）。
 *
 * 三个数各说各的事，所以都要露出来：窗口内几条、文件现存几条、本次会话几条。
 * 只显一个数就会让"存储不可用"和"这台机器从没告警过"长得一模一样。
 */
export function AlertHistory({
  events,
  persisted,
  info,
  error,
  span,
  onSpanChange,
  onRefresh,
}: Props) {
  const fromDisk = new Set(persisted.map(alertIdentity));

  return (
    <Space orientation="vertical" size="small" style={{ width: "100%" }}>
      <Space align="center" wrap>
        <Text>告警记录</Text>
        <Segmented
          size="small"
          value={span}
          onChange={(v) => onSpanChange(Number(v))}
          options={ALERT_HISTORY_SPANS.map((r) => ({ label: r.label, value: r.value }))}
        />
        <Tooltip title="重新读取落盘文件">
          <Button size="small" icon={<ReloadOutlined />} onClick={onRefresh} />
        </Tooltip>
      </Space>

      {error && (
        <Alert
          type="error"
          showIcon
          title={`读不到落盘的告警历史：${error}`}
          description="下面的列表只包含本次会话推送到的告警，不代表这台机器没告警过。"
        />
      )}
      {info && info.unreadableLines > 0 && (
        <Alert
          type="warning"
          showIcon
          title={`有 ${info.unreadableLines} 行历史读不出来`}
          description="进程被强杀时文件尾部会留半行。这里只报告、不修补，也不会用假记录占位。"
        />
      )}

      {events.length === 0 ? (
        <Text type="secondary">
          {info === null
            ? "暂无可显示的告警。"
            : info.totalEvents === 0
              ? "落盘文件里还没有任何告警记录。"
              : `当前窗口内没有告警，但文件里现存 ${info.totalEvents} 条更早的记录 —— 换个窗口看看。`}
        </Text>
      ) : (
        events.map((event) => {
          const disk = fromDisk.has(alertIdentity(event));
          return (
            <Space key={alertIdentity(event)} align="center" wrap size={6}>
              <Tag color={event.level === "critical" ? "red" : "orange"}>
                {ALERT_LEVEL_LABELS[event.level]}
              </Tag>
              {disk && <Tag>回填</Tag>}
              {/* 跨重启的点只有时分秒会看不出是哪天，日期一律带上 */}
              <Text type="secondary" style={{ fontSize: 12 }}>
                {dayjs(event.timestampMs).format("MM-DD HH:mm:ss")}
              </Text>
              <Text style={{ fontSize: 12 }}>{alertText(event)}</Text>
            </Space>
          );
        })
      )}

      {info && (
        <Text type="secondary" style={{ fontSize: 12 }}>
          窗口内 {info.storedEvents} 条 · 本次会话 {events.length - persisted.length} 条 · 文件现存{" "}
          {info.totalEvents} 条
          {info.storedEvents > info.returnedEvents &&
            ` · 只列出最近 ${info.returnedEvents} 条，另有 ${info.storedEvents - info.returnedEvents} 条更早的未列`}
          {info.oldestMs !== null &&
            ` · 最早可回溯 ${dayjs(info.oldestMs).format("MM-DD HH:mm")}`}
        </Text>
      )}
    </Space>
  );
}
