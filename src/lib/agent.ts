import type { AgentStatus, BackupConfig, ContainerConfig, RunLocation } from "./types";

/** 磁盘水位三档：控制机一块盘要扛所有备份包，界面必须把「还能不能写得下」说清楚。 */
export type DiskLevel = "ok" | "tight" | "low";

/**
 * 余量所处的档位。
 *
 * `floorBytes` 是 agent 拒绝任务的线（deploy-core 的 disk 模块算出来的水位线），
 * 所以「低于水位线」= 一定会失败，而「水位线到两倍之间」= 还能跑但今晚可能就撞上，
 * 需要提示用户先清理。取不到磁盘信息（total 为 0）时按 ok 处理，不编造告警。
 */
export function diskLevel(status: Pick<AgentStatus, "freeBytes" | "floorBytes" | "totalBytes">): DiskLevel {
  if (status.totalBytes === 0) return "ok";
  if (status.freeBytes < status.floorBytes) return "low";
  if (status.freeBytes < status.floorBytes * 2) return "tight";
  return "ok";
}

/** 按执行位把两类配置分开：控制机卡片要显示「几条归它管」。 */
export function splitByLocation<T extends { runLocation: RunLocation }>(
  items: T[],
): { local: T[]; remote: T[] } {
  const remote = items.filter((item) => item.runLocation === "remote");
  return { local: items.filter((item) => item.runLocation === "local"), remote };
}

/**
 * 下发之后还该不该再点一次「下发」。
 *
 * 判断口径只有一条：本机把执行位为远端的配置改了、删了，或加了新服务器，
 * 控制机上那份就落后了。状态里的计数是上次下发的结果，所以能直接比。
 */
export function syncNeeded(status: AgentStatus | null, configs: { backup: BackupConfig[]; container: ContainerConfig[]; servers: number }): boolean {
  if (!status) return false;
  const remote = splitByLocation(configs.backup).remote.length + splitByLocation(configs.container).remote.length;
  return (
    status.backupConfigs + status.containerConfigs !== remote ||
    status.servers !== configs.servers
  );
}
