import { useState } from "react";
import { Alert, Button, Card, Input, Space, Tag, Typography } from "antd";
import type { AgentFinding, AgentReply, AgentSuggestion, AgentSeverity } from "../ipc_contract";

const { Text, Paragraph } = Typography;

/**
 * Agent 诊断面板（T5-05 / T5-06）。
 *
 * 这里**没有** IPC：本文件不 import 任何 tauri API，提问由 `App` 下发（那条命令叫
 * `agent_query`），建议按钮只做两件事 —— 切页签、把 PID 填进进程表关键字。
 * 于是"Agent 建议 kill → 界面出现一条无确认的执行调用"这类事故在结构上不可能发生，
 * `agent.rs` 的 `agent_panel_has_no_ipc_channel_of_its_own` 会扫这个文件把它钉住。
 */

const EXAMPLES = [
  "哪个进程占 CPU 最高？",
  "为什么这么卡？",
  "磁盘还剩多少空间",
  "内存占用最多的进程",
  "现在网速怎么样",
];

const LEVEL_COLOR: Record<AgentSeverity, string> = {
  info: "default",
  warning: "orange",
  critical: "red",
};

const LEVEL_TEXT: Record<AgentSeverity, string> = {
  info: "正常",
  warning: "偏高",
  critical: "越限",
};

const METRIC_TEXT: Record<AgentFinding["metric"], string> = {
  cpu: "CPU",
  memory: "内存",
  disk: "磁盘",
  network: "网络",
  process: "进程",
  thermal: "温度",
};

const INTENT_TEXT: Record<AgentReply["intent"], string> = {
  rankProcesses: "进程排行",
  diagnoseSlowness: "综合诊断",
  memoryPressure: "内存压力",
  diskSpace: "磁盘空间",
  networkThroughput: "网络吞吐",
  startupItems: "启动项",
  junkFiles: "垃圾清理",
  temperature: "温度/风扇",
  unknown: "未识别",
};

interface AgentPanelProps {
  reply: AgentReply | null;
  asking: boolean;
  onAsk: (query: string) => void;
  onOpenTab: (tab: string) => void;
  onFocusProcess: (pid: number, name: string) => void;
}

/**
 * 未认出的建议类型一律不渲染。后端加新模板时必须同时在这里登记动作，
 * 否则界面上就是一条"有按钮但点了没反应"的假建议。
 */
function SuggestionCard({
  suggestion,
  onOpenTab,
  onFocusProcess,
}: {
  suggestion: AgentSuggestion;
  onOpenTab: (tab: string) => void;
  onFocusProcess: (pid: number, name: string) => void;
}) {
  switch (suggestion.kind) {
    case "openTab":
      return (
        <Button size="small" onClick={() => onOpenTab(suggestion.tab)}>
          {suggestion.label}
        </Button>
      );
    case "focusProcess":
      return (
        <Button
          size="small"
          onClick={() => onFocusProcess(suggestion.pid, suggestion.name)}
        >
          {suggestion.label}（PID {suggestion.pid}）
        </Button>
      );
    default:
      return null;
  }
}

export function AgentPanel({
  reply,
  asking,
  onAsk,
  onOpenTab,
  onFocusProcess,
}: AgentPanelProps) {
  const [draft, setDraft] = useState("");

  const ask = (value: string) => {
    const trimmed = value.trim();
    if (!trimmed || asking) return;
    setDraft("");
    onAsk(trimmed);
  };

  return (
    <Space orientation="vertical" size={16} style={{ width: "100%" }}>
      <Card
        title="诊断助手"
        className="monitor-card"
        extra={<Tag color="blue">只建议 · 不执行</Tag>}
      >
        <Space.Compact style={{ width: "100%" }}>
          <Input
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            onPressEnter={() => ask(draft)}
            placeholder="问我：哪个进程占 CPU 最高？为什么这么卡？"
            maxLength={200}
            disabled={asking}
            allowClear
          />
          <Button type="primary" loading={asking} onClick={() => ask(draft)}>
            问一问
          </Button>
        </Space.Compact>
        <Space size={[8, 8]} wrap style={{ marginTop: 12 }}>
          {EXAMPLES.map((example) => (
            <Button key={example} size="small" onClick={() => ask(example)} disabled={asking}>
              {example}
            </Button>
          ))}
        </Space>
        <Paragraph type="secondary" style={{ fontSize: 12, marginTop: 12, marginBottom: 0 }}>
          判定全部在后端 <Text code>agent.rs</Text> 完成：读的是与界面同一份采集快照和同一套告警阈值，
          不接任何外部模型，也不采集命令行参数与环境变量。它能给出的建议只有"打开某个页面"和
          "在进程表里定位某个 PID"；终止进程、清理文件、刷新 DNS 都必须在对应页面上由你确认后
          走各自的校验流程（应用内不提权、不执行 sudo）。
        </Paragraph>
      </Card>

      {asking && !reply ? (
        <Alert type="info" showIcon title="正在读取采集快照…" />
      ) : null}

      {reply ? (
        <Card
          size="small"
          title={
            <Space size={8}>
              <Text strong>“{reply.query || "（空）"}”</Text>
              <Tag>{INTENT_TEXT[reply.intent]}</Tag>
              {reply.rankBy ? <Tag color="geekblue">按 {reply.rankBy === "cpu" ? "CPU" : "内存"} 排序</Tag> : null}
            </Space>
          }
          extra={<Tag color="default">已执行：否</Tag>}
        >
          {reply.refused ? (
            <Alert
              type="warning"
              showIcon
              style={{ marginBottom: 12 }}
              title="这条像是在要求我代为执行操作，已拒绝"
              description="Agent 不执行任何命令，也不会生成命令行文本。需要动手的请去对应页面，那里有确认步骤。"
            />
          ) : null}

          {reply.findings.length > 0 ? (
            <Space orientation="vertical" size={8} style={{ width: "100%" }}>
              {reply.findings.map((finding, index) => (
                <div
                  key={`${finding.metric}-${index}`}
                  style={{ display: "flex", alignItems: "baseline", gap: 8 }}
                >
                  <Tag color={LEVEL_COLOR[finding.level]}>{LEVEL_TEXT[finding.level]}</Tag>
                  <Tag>{METRIC_TEXT[finding.metric]}</Tag>
                  <Text style={{ flex: 1 }}>{finding.text}</Text>
                  {/* 后端没有来源时给的是 null，这里显示 — 而不是 0 */}
                  <Text type="secondary" style={{ fontVariantNumeric: "tabular-nums" }}>
                    {finding.value ?? "—"}
                  </Text>
                </div>
              ))}
            </Space>
          ) : (
            <Text type="secondary">这一轮没有任何结论。</Text>
          )}

          {reply.notes.length > 0 ? (
            <Space orientation="vertical" size={8} style={{ width: "100%", marginTop: 12 }}>
              {reply.notes.map((note, index) => (
                <Alert key={index} type="info" showIcon title={note} />
              ))}
            </Space>
          ) : null}

          <div style={{ marginTop: 16 }}>
            <Text type="secondary" style={{ fontSize: 12 }}>
              待确认的建议（点了只会跳转，不会执行任何东西）
            </Text>
            <div style={{ marginTop: 8 }}>
              {reply.suggestions.length > 0 ? (
                <Space size={[8, 8]} wrap>
                  {reply.suggestions.map((suggestion, index) => (
                    <SuggestionCard
                      key={`${suggestion.kind}-${index}`}
                      suggestion={suggestion}
                      onOpenTab={onOpenTab}
                      onFocusProcess={onFocusProcess}
                    />
                  ))}
                </Space>
              ) : (
                <Text type="secondary">没有可跳转的目标。</Text>
              )}
            </div>
          </div>
        </Card>
      ) : null}
    </Space>
  );
}
