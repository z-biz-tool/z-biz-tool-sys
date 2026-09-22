import { Alert, Divider, Drawer, InputNumber, Space, Switch, Typography } from "antd";
import {
  ALERT_METRIC_LABELS,
  ALERT_METRIC_ORDER,
  DEFAULT_ALERT_CONFIG,
} from "../lib/alert";
import type { AlertConfig, AlertMetric, AlertThresholds, MetricsSnapshot } from "../ipc_contract";
import type { AlertsState } from "../hooks/useAlerts";
import { AlertHistory } from "./AlertHistory";

const { Text } = Typography;

interface Props {
  open: boolean;
  onClose: () => void;
  config: AlertConfig;
  onChange: (next: AlertConfig) => void;
  snapshot: MetricsSnapshot | null;
  /** 整条告警链路的状态都在这里：阈值有没有生效、会话内事件、落盘历史各自失败要分开说 */
  alerts: AlertsState;
}

/** 当前值：拿不到就返回 null，界面上显示 `—`，不拿 0 冒充读数 */
function currentValue(metric: AlertMetric, snapshot: MetricsSnapshot | null): number | null {
  if (!snapshot) return null;
  if (metric === "cpu") return snapshot.cpu.total;
  if (metric === "memory") return snapshot.memory.usagePercent;
  const usable = snapshot.disks.filter((d) => d.available && d.totalBytes > 0);
  if (!usable.length) return null;
  return Math.max(...usable.map((d) => d.usagePercent));
}

export function AlertSettingsDrawer({ open, onClose, config, onChange, snapshot, alerts }: Props) {
  const { pushed, syncError, listenError } = alerts;
  const patch = (part: Partial<AlertConfig>) => onChange({ ...config, ...part });
  const patchThresholds = (metric: AlertMetric, part: Partial<AlertThresholds>) =>
    onChange({ ...config, [metric]: { ...config[metric], ...part } });

  return (
    <Drawer open={open} onClose={onClose} size={520} title="告警阈值与历史">
      <Space orientation="vertical" size="small" style={{ width: "100%" }}>
        <Alert
          type={syncError ? "error" : pushed ? "success" : "warning"}
          showIcon
          title={
            syncError
              ? `阈值未能下发到后端：${syncError}`
              : pushed
                ? "判定在后端采集循环里执行，改动下一帧生效"
                : "后端尚未确认这份阈值，当前仍按它已有的配置判定"
          }
          description={
            syncError
              ? "界面显示的是本地暂存值，不会伪装成已生效的配置。"
              : `连续 ${config.consecutive} 帧越限才触发；同一告警 ${config.cooldownSecs} 秒内不重复推送。`
          }
        />
        {listenError && (
          <Alert
            type="error"
            showIcon
            title={`告警事件订阅失败：${listenError}`}
            description="后端仍会按阈值判定，但触发时这个窗口收不到推送，也不会补报。"
          />
        )}

        <Space align="center">
          <Switch
            checked={config.enabled}
            onChange={(checked) => patch({ enabled: checked })}
            checkedChildren="开"
            unCheckedChildren="静默"
          />
          <Text>启用告警</Text>
          {!config.enabled && <Text type="warning">静默期间后端连连续计数都不攒</Text>}
        </Space>

        <Space align="center">
          <Text>连续越限帧数</Text>
          <InputNumber
            min={1}
            max={60}
            precision={0}
            value={config.consecutive}
            onChange={(v) => patch({ consecutive: v ?? DEFAULT_ALERT_CONFIG.consecutive })}
            style={{ width: 90 }}
          />
          <Text type="secondary">帧（1 即第一帧就越限就报）</Text>
        </Space>

        <Space align="center">
          <Text>重复推送冷却</Text>
          <InputNumber
            min={0}
            max={86400}
            precision={0}
            value={config.cooldownSecs}
            onChange={(v) => patch({ cooldownSecs: v ?? DEFAULT_ALERT_CONFIG.cooldownSecs })}
            style={{ width: 90 }}
          />
          <Text type="secondary">秒</Text>
        </Space>

        <Divider style={{ margin: "8px 0" }} />

        {ALERT_METRIC_ORDER.map((metric) => {
          const value = currentValue(metric, snapshot);
          return (
            <Space key={metric} align="center" wrap>
              <Text style={{ width: 96, display: "inline-block" }}>
                {ALERT_METRIC_LABELS[metric]}
              </Text>
              <Text type="secondary" style={{ width: 84, display: "inline-block" }}>
                当前 {value === null ? "—" : `${value.toFixed(1)} %`}
              </Text>
              <InputNumber
                min={1}
                max={100}
                value={config[metric].warning}
                onChange={(v) => patchThresholds(metric, { warning: v ?? config[metric].warning })}
                style={{ width: 88 }}
                addonBefore="警告"
              />
              <InputNumber
                min={1}
                max={100}
                value={config[metric].critical}
                onChange={(v) => patchThresholds(metric, { critical: v ?? config[metric].critical })}
                style={{ width: 88 }}
                addonBefore="严重"
              />
            </Space>
          );
        })}
        <Text type="secondary">
          磁盘按最满的那一块判；两条阈值都在攒时取更高的那一档，严重阈值不会低于警告阈值。
        </Text>

        <Divider style={{ margin: "8px 0" }} />
        <AlertHistory
          events={alerts.events}
          persisted={alerts.persisted}
          info={alerts.historyInfo}
          error={alerts.historyError}
          span={alerts.historySpan}
          onSpanChange={alerts.setHistorySpan}
          onRefresh={alerts.refreshHistory}
        />
      </Space>
    </Drawer>
  );
}
