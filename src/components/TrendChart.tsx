import { Card } from "antd";
import {
  Area,
  AreaChart,
  CartesianGrid,
  ResponsiveContainer,
  Tooltip as ChartTooltip,
  XAxis,
  YAxis,
} from "recharts";
import { gradientText, monitorCardStyle } from "../lib/ui";

export function TrendChart({
  title,
  data,
  dataKey,
  color,
  gradientId,
  yMax,
  unitFormatter,
}: {
  title: string;
  data: Array<Record<string, number | string>>;
  dataKey: string;
  color: string;
  gradientId: string;
  yMax?: number | "auto";
  unitFormatter: (v: number) => string;
}) {
  return (
    <Card
      size="small"
      className="monitor-card"
      style={{ ...monitorCardStyle }}
      title={<span style={{ ...gradientText, fontWeight: 600 }}>{title}</span>}
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
        </AreaChart>
      </ResponsiveContainer>
    </Card>
  );
}
