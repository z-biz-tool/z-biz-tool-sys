import { useEffect, useState } from "react";
import type { MetricsSnapshot } from "../ipc_contract";
import { appendNetworkFrame, type NetworkSeries } from "../lib/networkSeries";

/**
 * per-interface 实时时序（T3-02）。
 *
 * 每一帧采集进来就并一次。注意 `snapshot` 是**按对象身份**触发的：
 * 后端停采时前端拿到的还是同一个对象引用，effect 不会重跑 ——
 * 于是曲线自然停在停采那一刻，**不会**被补出一段假的水平线。
 */
export function useNetworkSeries(snapshot: MetricsSnapshot | null): NetworkSeries {
  const [series, setSeries] = useState<NetworkSeries>(() => new Map());

  useEffect(() => {
    setSeries((prev) => appendNetworkFrame(prev, snapshot));
  }, [snapshot]);

  return series;
}
