import { describe, expect, it } from "vitest";

import { applyTaskEvent, MAX_LIVE_LINES, reconcileLiveTask, type TaskRecord } from "./liveTask";
import type { LiveTask, LogLevel } from "./types";

function makeLive(overrides: Partial<LiveTask<TaskRecord>> = {}): LiveTask<TaskRecord> {
  return {
    recordId: "",
    lines: [],
    progress: 0,
    progressMessage: "",
    status: "running",
    record: null,
    ...overrides,
  };
}

function makeRecord(id: string, status: TaskRecord["status"] = "running"): TaskRecord {
  return { id, status };
}

function makeSink(initial: LiveTask<TaskRecord> | null, runningId = "") {
  const state = { live: initial, announced: [] as TaskRecord[] };
  return {
    state,
    sink: {
      getLive: () => state.live,
      setLive: (next: LiveTask<TaskRecord>) => {
        state.live = next;
      },
      findRunningId: () => runningId,
      announce: (record: TaskRecord) => {
        state.announced.push(record);
      },
    },
  };
}

const log = (message: string, level: LogLevel = "info") => ({ type: "log" as const, level, message });

describe("applyTaskEvent", () => {
  it("无 live 时 started 补建运行状态", () => {
    const { state, sink } = makeSink(null);
    applyTaskEvent({ type: "started", recordId: "r1" }, sink);
    expect(state.live).toEqual({
      recordId: "r1",
      lines: [],
      progress: 0,
      progressMessage: "",
      status: "running",
      record: null,
      aborted: null,
    });
  });

  it("批量部署：下一台的 started 保留上一台的批次行", () => {
    // 引擎在每台开始时先写「[批次 i/N] 开始部署到 X」再发 started，
    // 上一台结束时带着记录 —— 这时候清行就会把批次行自己抹掉。
    const { state, sink } = makeSink(
      makeLive({
        recordId: "r1",
        status: "success",
        record: makeRecord("r1", "success"),
        lines: [
          { level: "info", message: "[批次 1/3] 开始部署到 A" },
          { level: "info", message: "[批次 2/3] 开始部署到 B" },
        ],
      }),
    );
    applyTaskEvent({ type: "started", recordId: "r2" }, sink);
    expect(state.live?.lines.map((line) => line.message)).toEqual([
      "[批次 1/3] 开始部署到 A",
      "[批次 2/3] 开始部署到 B",
    ]);
    expect(state.live?.recordId).toBe("r2");
    expect(state.live?.status).toBe("running");
    expect(state.live?.record).toBeNull();
  });

  it("batchAborted 把界面从上一台的绿色成功改成失败", () => {
    const { state, sink } = makeSink(
      makeLive({
        recordId: "r1",
        status: "success",
        record: makeRecord("r1", "success"),
        lines: [{ level: "info", message: "[批次 1/2] 开始部署到 A" }],
      }),
    );
    applyTaskEvent({ type: "batchAborted", succeeded: 1, total: 2, reason: "服务器已被删除" }, sink);
    expect(state.live?.status).toBe("failed");
    expect(state.live?.aborted).toEqual({ succeeded: 1, total: 2, reason: "服务器已被删除" });
    // 已经跑过的那台记录留着：它确实是成功的，批次结论另说。
    expect(state.live?.record).not.toBeNull();
  });

  it("无 live 时 batchAborted 也补建成失败状态（重载后事件先到）", () => {
    const { state, sink } = makeSink(null, "r7");
    applyTaskEvent({ type: "batchAborted", succeeded: 1, total: 3, reason: "准备失败" }, sink);
    expect(state.live?.recordId).toBe("r7");
    expect(state.live?.status).toBe("failed");
    expect(state.live?.aborted).toEqual({ succeeded: 1, total: 3, reason: "准备失败" });
  });

  it("无 live 时 log 用记录列表里的运行 id 兜底补建", () => {
    const { state, sink } = makeSink(null, "r9");
    applyTaskEvent(log("hello", "command"), sink);
    expect(state.live?.recordId).toBe("r9");
    expect(state.live?.lines).toEqual([{ level: "command", message: "hello" }]);
  });

  it("无 live 时 progress 补建并带上百分比与文案", () => {
    const { state, sink } = makeSink(null, "r9");
    applyTaskEvent({ type: "progress", percent: 42, message: "上传压缩包" }, sink);
    expect(state.live).toEqual({
      recordId: "r9",
      lines: [],
      progress: 42,
      progressMessage: "上传压缩包",
      status: "running",
      record: null,
      aborted: null,
    });
  });

  it("无 live 时 finished 只通知、不补建状态", () => {
    const record = makeRecord("r1", "success");
    const { state, sink } = makeSink(null);
    applyTaskEvent({ type: "finished", record }, sink);
    expect(state.live).toBeNull();
    expect(state.announced).toEqual([record]);
  });

  it("started 清空上一轮日志与结果（store 发起新任务时不带记录）", () => {
    // 界面上点「部署 / 重新部署」时 store 会把 live 重置成没有记录的形状，所以这一轮必须清空；
    // 带着记录的那条路径是批量部署的下一台，见上面「保留批次行」的用例。
    const { state, sink } = makeSink(
      makeLive({ recordId: "old", lines: [{ level: "info", message: "old" }], record: null }),
    );
    applyTaskEvent({ type: "started", recordId: "new" }, sink);
    expect(state.live).toEqual({
      recordId: "new",
      lines: [],
      progress: 0,
      progressMessage: "",
      status: "running",
      record: null,
      aborted: null,
    });
  });

  it("log 追加日志行", () => {
    const { state, sink } = makeSink(makeLive({ recordId: "r1" }));
    applyTaskEvent(log("a"), sink);
    applyTaskEvent(log("b", "error"), sink);
    expect(state.live?.lines).toEqual([
      { level: "info", message: "a" },
      { level: "error", message: "b" },
    ]);
  });

  it("日志行数超过上限时丢弃最早的", () => {
    const lines = Array.from({ length: MAX_LIVE_LINES }, (_, index) => ({
      level: "info" as LogLevel,
      message: `m${index}`,
    }));
    const { state, sink } = makeSink(makeLive({ recordId: "r1", lines }));
    applyTaskEvent(log("last"), sink);
    expect(state.live?.lines).toHaveLength(MAX_LIVE_LINES);
    expect(state.live?.lines[0].message).toBe("m1");
    expect(state.live?.lines[MAX_LIVE_LINES - 1].message).toBe("last");
  });

  it("progress 更新百分比与后端带来的步骤文案", () => {
    const { state, sink } = makeSink(makeLive({ recordId: "r1", progress: 10 }));
    applyTaskEvent({ type: "progress", percent: 66, message: "解压到 releases" }, sink);
    expect(state.live?.progress).toBe(66);
    // 文案曾经在这里被丢掉：界面只剩一个百分比，用户看不到卡在哪一步。
    expect(state.live?.progressMessage).toBe("解压到 releases");
  });

  it("finished 收敛状态、进度并通知", () => {
    const record = makeRecord("r1", "failed");
    const { state, sink } = makeSink(makeLive({ recordId: "r1", progress: 30 }));
    applyTaskEvent({ type: "finished", record }, sink);
    expect(state.live).toEqual({
      recordId: "r1",
      lines: [],
      progress: 100,
      progressMessage: "",
      status: "failed",
      record,
    });
    expect(state.announced).toEqual([record]);
  });
});

describe("reconcileLiveTask", () => {
  it("记录仍在运行：补回 recordId 并保持运行状态", () => {
    const next = reconcileLiveTask(makeLive(), [makeRecord("r1")]);
    expect(next?.recordId).toBe("r1");
    expect(next?.status).toBe("running");
  });

  it("记录已结束：收敛状态与结果，进度置满", () => {
    const record = makeRecord("r1", "success");
    const next = reconcileLiveTask(makeLive({ recordId: "r1", progress: 40 }), [record]);
    expect(next).toEqual({
      recordId: "r1",
      lines: [],
      progress: 100,
      progressMessage: "",
      status: "success",
      record,
    });
  });

  it("有 recordId 但记录不存在：保持现状（可能列表还没刷新到）", () => {
    expect(reconcileLiveTask(makeLive({ recordId: "r1" }), [makeRecord("other")])).toBeUndefined();
  });

  it("无 recordId 且没有运行中的记录：清空 live", () => {
    expect(reconcileLiveTask(makeLive(), [makeRecord("r1", "success")])).toBeNull();
  });

  it("列表接口失败（records 为 null）：不更新", () => {
    expect(reconcileLiveTask(makeLive({ recordId: "r1" }), null)).toBeUndefined();
  });

  it("live 不是运行中（或无 live）：不更新", () => {
    expect(reconcileLiveTask(makeLive({ status: "success" }), [makeRecord("r1")])).toBeUndefined();
    expect(reconcileLiveTask(null, [makeRecord("r1")])).toBeUndefined();
  });
});
