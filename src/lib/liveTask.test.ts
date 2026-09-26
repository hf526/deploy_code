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
    });
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
    });
  });

  it("无 live 时 finished 只通知、不补建状态", () => {
    const record = makeRecord("r1", "success");
    const { state, sink } = makeSink(null);
    applyTaskEvent({ type: "finished", record }, sink);
    expect(state.live).toBeNull();
    expect(state.announced).toEqual([record]);
  });

  it("started 清空上一轮日志与结果", () => {
    const { state, sink } = makeSink(
      makeLive({ recordId: "old", lines: [{ level: "info", message: "old" }], record: makeRecord("old") }),
    );
    applyTaskEvent({ type: "started", recordId: "new" }, sink);
    expect(state.live).toEqual({
      recordId: "new",
      lines: [],
      progress: 0,
      progressMessage: "",
      status: "running",
      record: null,
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
