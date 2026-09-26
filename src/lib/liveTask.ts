import type { DeployStatus, LiveTask, LogLevel } from "./types";

/** 三类任务记录都带 id 与 status：id 用于重载后对账，status 用于收敛最终状态。 */
export interface TaskRecord {
  id: string;
  status: DeployStatus;
}

/** 部署 / 备份 / Pages 三类长任务共用的事件载荷。 */
export type TaskEvent<TRecord extends TaskRecord> =
  | { type: "started"; recordId: string }
  | { type: "log"; level: LogLevel; message: string }
  | { type: "progress"; percent: number; message: string }
  | { type: "finished"; record: TRecord };

/** 事件归约需要的最小副作用集合（由 store 提供）。 */
export interface TaskEventSink<TRecord extends TaskRecord> {
  getLive: () => LiveTask<TRecord> | null;
  setLive: (live: LiveTask<TRecord>) => void;
  /** 
   * 重载后事件先到时，从持久化记录里找正在运行的任务 id 兜底。
   * ⚠️ 注意：理论上同一时间只有一个 running 任务（抢占机制），所以直接返回第一个匹配的即可
   */
  findRunningId: () => string;
  /** 任务结束：刷新对应记录列表并弹出结果提示。 */
  announce: (record: TRecord) => void;
}

/** 单任务保留的日志行上限（超出后丢弃最早的日志）。 */
export const MAX_LIVE_LINES = 6000;

function runningTask<TRecord>(recordId: string): LiveTask<TRecord> {
  return {
    recordId,
    lines: [],
    progress: 0,
    progressMessage: "",
    status: "running",
    record: null,
  };
}

/** 三类长任务共用的事件归约：维护日志 / 进度 / 状态，避免三份重复实现逐渐分叉。 */
export function applyTaskEvent<TRecord extends TaskRecord>(
  event: TaskEvent<TRecord>,
  sink: TaskEventSink<TRecord>,
): void {
  const { getLive, setLive, findRunningId, announce } = sink;
  const live = getLive();

  if (!live) {
    // 刷新/重载后 live 为空，但后台任务仍在跑：按事件补建 live 状态，后续日志/进度才能继续接收。
    // ⚠️ 注意：此时 event 可能还没携带完整 record，所以 log/progress 事件用 findRunningId() 兜底
    if (event.type === "started") {
      setLive(runningTask(event.recordId));
      return;
    }
    if (event.type === "log") {
      setLive({
        ...runningTask(findRunningId()),
        lines: [{ level: event.level, message: event.message }],
      });
      return;
    }
    if (event.type === "progress") {
      setLive({
        ...runningTask(findRunningId()),
        progress: event.percent,
        progressMessage: event.message,
      });
      return;
    }
    announce(event.record);
    return;
  }

  switch (event.type) {
    case "started":
      // 新一轮任务开始：清空上一轮的日志 / 进度与结果状态。
      setLive(runningTask(event.recordId));
      break;
    case "log": {
      const lines = [...live.lines, { level: event.level, message: event.message }];
      if (lines.length > MAX_LIVE_LINES) lines.splice(0, lines.length - MAX_LIVE_LINES);
      setLive({ ...live, lines });
      break;
    }
    case "progress":
      setLive({ ...live, progress: event.percent, progressMessage: event.message });
      break;
    case "finished":
      setLive({ ...live, status: event.record.status, record: event.record, progress: 100 });
      announce(event.record);
      break;
  }
}

/**
 * 重载后对账：用持久化记录收敛 live 状态，避免错过 finished/started 事件后永久卡在 running。
 * `records` 为 null 表示列表接口失败（保持现状）；返回 undefined 表示无需更新。
 */
export function reconcileLiveTask<TRecord extends TaskRecord>(
  current: LiveTask<TRecord> | null,
  records: TRecord[] | null,
): LiveTask<TRecord> | null | undefined {
  if (!current || current.status !== "running" || !records) return undefined;
  const record = current.recordId
    ? records.find((item) => item.id === current.recordId)
    : records.find((item) => item.status === "running");
  if (!record) return current.recordId ? undefined : null;
  return {
    ...current,
    // 补回 recordId，否则「停止任务」按钮会一直禁用。
    recordId: current.recordId || record.id,
    ...(record.status !== "running" ? { status: record.status, record, progress: 100 } : {}),
  };
}
