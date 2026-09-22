import { Card } from "antd";
import {
  Area,
  AreaChart,
  CartesianGrid,
  ReferenceLine,
  ResponsiveContainer,
  Tooltip as ChartTooltip,
  XAxis,
  YAxis,
} from "recharts";
import { gradientText, monitorCardStyle } from "../lib/ui";
import { ALERT_LEVEL_COLORS, ALERT_LEVEL_LABELS } from "../lib/alert";
import type { TrendMarker } from "../lib/trend";

export function TrendChart({
  title,
  data,
  dataKey,
  color,
  gradientId,
  yMax,
  unitFormatter,
  markers,
  markerSkewMs,
}: {
  title: string;
  data: Array<Record<string, number | string>>;
  dataKey: string;
  color: string;
  gradientId: string;
  yMax?: number | "auto";
  unitFormatter: (v: number) => string;
  /** 已触发告警在这条曲线上的标注（T5-03）；为空就一根线都不画 */
  markers?: TrendMarker[];
  /** 标注对齐到采样点的最大偏移，用来说明"线只代表这一格" */
  markerSkewMs?: number;
}) {
  // markers 已按时间升序；密集告警时保留**最近**的 40 处，否则一次故障风暴能把整张图画满而最近的反倒没影
  const shown = markers && markers.length > 40 ? markers.slice(-40) : markers ?? [];
  const hidden = (markers?.length ?? 0) - shown.length;
  return (
    <Card
      size="small"
      className="monitor-card"
      style={{ ...monitorCardStyle }}
      title={<span style={{ ...gradientText, fontWeight: 600 }}>{title}</span>}
      extra={
        shown.length ? (
          <span style={{ fontSize: 12, color: "#999" }}>
            {shown.length} 处告警标注
            {markerSkewMs !== undefined &&
              ` · 定位误差 ≤ ${Math.round(markerSkewMs / 1000)} s`}
            {hidden > 0 && ` · 另有 ${hidden} 处未画`}
          </span>
        ) : null
      }
    >
      <ResponsiveContainer width="100%" height={200}>
        <AreaChart data={data}>
          <defs>
            <linearGradient id={gradientId} x1="0" y1="0" x2="0" y2="1">
              <stop offset="5%" stopColor={color} stopOpacity={0.8} />
              <stop offset="95%" stopColor={color} stopOpacity={0} />
            </linearGradient>
          </defs>
          <CartesianGrid strokeDasharray="3 3" />
          <XAxis dataKey="time" tick={{ fontSize: 12 }} minTickGap={24} />
          <YAxis domain={[0, yMax ?? "auto"]} tick={{ fontSize: 12 }} width={64} />
          <ChartTooltip formatter={(v) => unitFormatter(Number(v))} />
          <Area
            type="monotone"
            dataKey={dataKey}
            stroke={color}
            fill={`url(#${gradientId})`}
            isAnimationActive={false}
          />
          {/* 竖线只画在真实存在的那个采样点上：曲线在这里没数据就不画，不靠插值造位置 */}
          {shown.map((m) => (
            <ReferenceLine
              key={`${m.x}-${m.level}`}
              x={m.x}
              stroke={ALERT_LEVEL_COLORS[m.level]}
              strokeDasharray="4 3"
              label={{
                value: `${ALERT_LEVEL_LABELS[m.level]}${m.count > 1 ? `×${m.count}` : ""}`,
                position: "top",
                fill: ALERT_LEVEL_COLORS[m.level],
                fontSize: 10,
              }}
            />
          ))}
        </AreaChart>
      </ResponsiveContainer>
    </Card>
  );
}
