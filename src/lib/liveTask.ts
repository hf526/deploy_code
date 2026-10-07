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
  | { type: "finished"; record: TRecord }
  /** 只有批量部署会有：某台连准备都没过，剩下的机器不再部署，而那台没有自己的记录。 */
  | { type: "batchAborted"; succeeded: number; total: number; reason: string };

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
    aborted: null,
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
    if (event.type === "batchAborted") {
      // 重载之后中止事件先到（那台没有记录可以对账）：照样把这一批定性成失败，
      // 否则界面会停在上一台的绿色「成功」上。
      setLive({
        ...runningTask(findRunningId()),
        status: "failed",
        progress: 100,
        aborted: { succeeded: event.succeeded, total: event.total, reason: event.reason },
      });
      return;
    }
    if (event.type === "log") {
      const recordId = findRunningId();
      // 没有正在跑的任务可挂靠（命令已经失败、这次批次连记录都没写）：忽略。
      // 硬要补建，就会造出一个 recordId 为空、再也不会收到事件的 running 状态，界面永久卡在「进行中」。
      if (!recordId) return;
      setLive({
        ...runningTask(recordId),
        lines: [{ level: event.level, message: event.message }],
      });
      return;
    }
    if (event.type === "progress") {
      const recordId = findRunningId();
      if (!recordId) return;
      setLive({
        ...runningTask(recordId),
        progress: event.percent,
        progressMessage: event.message,
      });
      return;
    }
    announce(event.record);
    return;
  }

  switch (event.type) {
    case "started": {
      // 新一轮任务开始：清掉上一轮的进度与结果状态，但**保留已有日志行**。
      // 单次任务由 store 在发起时把整块 live 重置，到这里本来就没有旧日志；
      // 批量部署每台各发一条 started，而引擎先把「[批次 i/N] 开始部署到 X」写进日志再发 started ——
      // 首台那行来不及带记录，按「有记录才保留」的判断会把它连同后续批次行一起清掉（N 台丢 N 行）。
      const carried = live.lines;
      setLive({ ...runningTask(event.recordId), lines: carried });
      break;
    }
    case "batchAborted":
      // 中止那台没有自己的记录：上一条 finished 已把界面定成成功，批次性质只能由这条事件纠正回来。
      setLive({
        ...live,
        status: "failed",
        progress: 100,
        aborted: { succeeded: event.succeeded, total: event.total, reason: event.reason },
      });
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
  // 有 recordId 却查不到记录：多半是这次列表还没刷新到（对账只在 loadAll 里跑、
  // 不轮询），所以保持原状而不是判定成失败 —— 判错会把正在看的任务状态抹掉。
  // 「记录被真的删掉」那条路已经由后端堵死（Store 的四组删除方法在对应任务锁被
  // 持有时直接拒绝），所以这里不存在「永久卡 running」：删不掉，也就对得上账。
  if (!record) return current.recordId ? undefined : null;
  return {
    ...current,
    // 补回 recordId，否则「停止任务」按钮会一直禁用。
    recordId: current.recordId || record.id,
    ...(record.status !== "running" ? { status: record.status, record, progress: 100 } : {}),
  };
}
