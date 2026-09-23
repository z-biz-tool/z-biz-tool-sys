import { Alert, Button, Divider, Drawer, InputNumber, Space, Switch, Typography } from "antd";
import { useEffect, useState } from "react";
import {
  ALERT_METRIC_LABELS,
  ALERT_METRIC_ORDER,
  busiestDisk,
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
  return busiestDisk(snapshot)?.usagePercent ?? null;
}

export function AlertSettingsDrawer({ open, onClose, config, onChange, snapshot, alerts }: Props) {
  const { pushed, syncError, listenError, notifyStatus, notifyError, refreshNotifyStatus } = alerts;
  const [testing, setTesting] = useState(false);
  const [testOutcome, setTestOutcome] = useState<{ level: "success" | "warning"; text: string } | null>(null);
  const patch = (part: Partial<AlertConfig>) => onChange({ ...config, ...part });
  const patchThresholds = (metric: AlertMetric, part: Partial<AlertThresholds>) =>
    onChange({ ...config, [metric]: { ...config[metric], ...part } });

  // 记账是累计的、又不急，所以只在抽屉打开这一动作上读一次（不在挂载时读，也不每帧读）
  useEffect(() => {
    if (open) refreshNotifyStatus();
  }, [open, refreshNotifyStatus]);

  const notifyCountLine = notifyStatus
    ? `已提交 ${notifyStatus.submitted} 条 · 失败 ${notifyStatus.failed} 条${
        notifyStatus.lastError ? `；最后一次失败：${notifyStatus.lastError}` : "；没有失败记录"
      }`
    : "还没有读到这一轮的投递记账";

  const onTestNotification = () => {
    setTesting(true);
    setTestOutcome(null);
    alerts
      .sendTestNotification()
      .then(() =>
        setTestOutcome({
          level: "success",
          text: "这条测试通知已提交给系统的通知接口 —— 看到了就说明通道可用，没看到就是系统那边拦着",
        })
      )
      .catch(() => {
        /* 失败原文由 notifyError 呈现，两处不重复报同一句话 */
      })
      .finally(() => setTesting(false));
  };

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

        <Divider style={{ margin: "8px 0" }} />
        <Text strong>系统通知</Text>
        <Text type="secondary">
          告警触发时，除了这个窗口里的提示，还会往操作系统的通知中心投一条同样文案的通知。
          它与上面的总开关同进同退：静默期间两边都不发，也不存在"窗口没弹但通知弹了"。
        </Text>
        <Space align="center" wrap>
          <Button size="small" loading={testing} onClick={onTestNotification}>
            发一条测试通知
          </Button>
          <Text type="secondary">{notifyCountLine}</Text>
        </Space>
        {notifyStatus && notifyStatus.suppressed > 0 && (
          <Text type="secondary">
            本会话有 {notifyStatus.suppressed} 条因窗口全屏（演示）没投系统通知 ——
            只有横幅被静默，告警事件与落盘历史照常，列表里能查到这几条。
          </Text>
        )}
        {testOutcome && <Text type={testOutcome.level}>{testOutcome.text}</Text>}
        {/* macOS 的通知后端不会把投递结果报回来，所以这里只能说"提交"，不能说"已送达" */}
        {notifyStatus && !notifyStatus.deliveryIsReported && (
          <Text type="secondary">
            "已提交"只表示这条已经交给操作系统的通知接口；这台机器的后端不把投递结果报回来，
            真弹没弹请以通知中心为准（看不到就先按上面那颗按钮试一次）。
          </Text>
        )}
        {notifyError && (
          <Alert
            type="warning"
            showIcon
            title={
              notifyError.action === "test"
                ? `测试通知没发出去：${notifyError.message}`
                : `读不到系统通知的投递记账：${notifyError.message}`
            }
            description="这与「告警有没有判定」是两件事：判定与落盘在后端照常进行。"
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
              {/* antd 6 已废弃 InputNumber 的 addonBefore，标签只能自己摆（探针实测到该条 console error） */}
              <Text type="secondary" style={{ fontSize: 12 }}>
                警告 %
              </Text>
              <InputNumber
                min={1}
                max={100}
                value={config[metric].warning}
                onChange={(v) => patchThresholds(metric, { warning: v ?? config[metric].warning })}
                style={{ width: 64 }}
              />
              <Text type="secondary" style={{ fontSize: 12 }}>
                严重 %
              </Text>
              <InputNumber
                min={1}
                max={100}
                value={config[metric].critical}
                onChange={(v) => patchThresholds(metric, { critical: v ?? config[metric].critical })}
                style={{ width: 64 }}
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
