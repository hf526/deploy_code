import { useEffect, useState } from "react";

import { api } from "./api";
import type { AgentStaleness } from "./types";

/**
 * 控制机上那份配置与本机当前设置是否已经不一致（后端纯读盘，不连服务器）。
 *
 * 不进全局 store、也不只在进页面时取一次：这一栏说的就是「你刚改的那一下有没有下发」，
 * 用快照会一直显示改动之前的结论。`watch` 传调用方 relevant 的那份设置即可 ——
 * 设置每次保存都是新对象，改完自然重取。
 */
export function useAgentStaleness(watch: unknown): AgentStaleness | null {
  const [staleness, setStaleness] = useState<AgentStaleness | null>(null);

  useEffect(() => {
    let cancelled = false;
    api
      .agentStaleness()
      .then((result) => {
        if (!cancelled) setStaleness(result);
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
    };
  }, [watch]);

  return staleness;
}
