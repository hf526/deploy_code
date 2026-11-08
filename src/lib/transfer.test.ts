import { describe, expect, it } from "vitest";

import {
  REMOTE_DIR_STORAGE_KEY,
  applyTransferEvent,
  entriesFromInfos,
  findEntry,
  isValidRemotePath,
  joinRemoteDir,
  mergeSeededEntries,
  readStoredRemoteDirs,
  transferPercent,
  type TransferEntry,
} from "./transfer";
import type { TransferInfo } from "./types";

function info(id: string, overrides: Partial<TransferInfo> = {}): TransferInfo {
  return {
    id,
    serverId: `s-${id}`,
    serverName: "深圳",
    direction: "upload",
    fileName: "app.tar.gz",
    remotePath: "/opt/app/app.tar.gz",
    ...overrides,
  };
}

function entry(id: string, overrides: Partial<TransferEntry> = {}): TransferEntry {
  return { ...info(id), done: 0, total: 0, ...overrides };
}

describe("entriesFromInfos", () => {
  it("对账回来的登记信息按「还没收到回调」画，进度从 0 开始", () => {
    expect(entriesFromInfos([info("a")])).toEqual([entry("a")]);
  });

  it("空列表还是空列表", () => {
    expect(entriesFromInfos([])).toEqual([]);
  });
});

describe("mergeSeededEntries：挂载对账", () => {
  it("只做加法：快照不许抹掉用户刚自己发起的那条", () => {
    const current = [entry("mine", { done: 5, total: 10 })];
    const next = mergeSeededEntries(current, [info("backend")], new Set());
    expect(next.map((item) => item.id)).toEqual(["mine", "backend"]);
    // 已有那条的进度原样留着，不被快照里的 0 盖回去。
    expect(next[0].done).toBe(5);
  });

  it("同一个 id 不重复画，且没有新增时交回原数组", () => {
    const current = [entry("a")];
    expect(mergeSeededEntries(current, [info("a")], new Set())).toBe(current);
    expect(mergeSeededEntries(current, [], new Set())).toBe(current);
  });

  it("已经收过尾的 id 不许被快照复活", () => {
    // 快照拍下之后、invoke 落地之前那条传输正好结束：finished 先到（列表里还没有它），
    // 快照后到。只按 id 去重的加法会把它重新画出来，而且再没有事件来摘它。
    const settled = new Set(["gone"]);
    const next = mergeSeededEntries([], [info("gone"), info("alive")], settled);
    expect(next.map((item) => item.id)).toEqual(["alive"]);
  });

  it("收过尾的 id 即使当前还在列表里也不受影响（去重那一支先接住）", () => {
    const current = [entry("a", { done: 9, total: 10 })];
    const next = mergeSeededEntries(current, [info("a")], new Set(["a"]));
    expect(next).toBe(current);
  });
});

describe("applyTransferEvent：started", () => {
  // 命令返回值与 started 事件带的是同一次传输，两边的落地顺序不保证。
  it("同一个 id 来两次只留一条", () => {
    const first = applyTransferEvent([], {
      type: "started",
      ...info("a"),
    });
    const second = applyTransferEvent(first, { type: "started", ...info("a") });
    expect(second).toHaveLength(1);
    expect(second).toBe(first);
  });

  it("新 id 排在已有那条后面，不动已经有的那些", () => {
    const one = applyTransferEvent([], { type: "started", ...info("a", { fileName: "one.txt" }) });
    const two = applyTransferEvent(one, {
      type: "started",
      ...info("b", { direction: "download", fileName: "two.txt" }),
    });
    expect(two.map((item) => item.id)).toEqual(["a", "b"]);
    expect(two[0].fileName).toBe("one.txt");
    expect(two[1].direction).toBe("download");
  });
});

describe("applyTransferEvent：progress", () => {
  it("未知 id 直接忽略，且返回的是同一个数组", () => {
    // 一秒好几条进度，而每次 setState 都要过一遍 React；已经收尾那条的迟到回调也不能
    // 把条带画回来，所以「没变化」必须是数组身份没变，而不只是内容相等。
    const list = [entry("a", { done: 3, total: 10 })];
    expect(applyTransferEvent(list, { type: "progress", id: "zzz", done: 1, total: 9 })).toBe(list);
  });

  it("数字没变时同样返回原数组", () => {
    const list = [entry("a", { done: 3, total: 10 })];
    expect(applyTransferEvent(list, { type: "progress", id: "a", done: 3, total: 10 })).toBe(list);
  });

  it("只改 done/total，其余字段原样留着", () => {
    const list = [entry("a", { done: 3, total: 10 })];
    const next = applyTransferEvent(list, { type: "progress", id: "a", done: 7, total: 20 });
    expect(next).not.toBe(list);
    expect(next[0]).toEqual(entry("a", { done: 7, total: 20 }));
    expect(next[0].remotePath).toBe("/opt/app/app.tar.gz");
    expect(list[0].done).toBe(3);
  });

  it("总量仍然未知时照收 0（界面按「还不知道」画，而不是当错误）", () => {
    const next = applyTransferEvent([entry("a", { done: 1, total: 0 })], {
      type: "progress",
      id: "a",
      done: 4096,
      total: 0,
    });
    expect(next[0].done).toBe(4096);
    expect(next[0].total).toBe(0);
  });
});

describe("applyTransferEvent：收尾三条", () => {
  it("finished 把那一条摘掉", () => {
    const next = applyTransferEvent([entry("a"), entry("b")], { type: "finished", id: "a" });
    expect(next.map((item) => item.id)).toEqual(["b"]);
  });

  // 取消不是失败：那条文件根本没落地。
  it("cancelled 把那一条摘掉", () => {
    const next = applyTransferEvent([entry("a"), entry("b")], { type: "cancelled", id: "a" });
    expect(next.map((item) => item.id)).toEqual(["b"]);
  });

  it("failed 把那一条摘掉", () => {
    const next = applyTransferEvent([entry("a"), entry("b")], {
      type: "failed",
      id: "a",
      reason: "远端目录不存在",
    });
    expect(next.map((item) => item.id)).toEqual(["b"]);
  });

  it("收尾事件指向一个已经不存在的 id 时返回原数组", () => {
    const list = [entry("a")];
    expect(applyTransferEvent(list, { type: "finished", id: "zzz" })).toBe(list);
    expect(applyTransferEvent(list, { type: "cancelled", id: "zzz" })).toBe(list);
    expect(applyTransferEvent(list, { type: "failed", id: "zzz", reason: "x" })).toBe(list);
  });

  it("摘掉之后同一条的迟到进度不会把条带画回来", () => {
    const afterFinish = applyTransferEvent([entry("a")], { type: "finished", id: "a" });
    expect(applyTransferEvent(afterFinish, { type: "progress", id: "a", done: 5, total: 5 })).toBe(
      afterFinish,
    );
    expect(afterFinish).toEqual([]);
  });
});

describe("findEntry", () => {
  it("按 id 取那一条，取不到就是 undefined", () => {
    const list = [entry("a", { done: 1, total: 2 })];
    expect(findEntry(list, "a")?.done).toBe(1);
    expect(findEntry(list, "zzz")).toBeUndefined();
  });
});

describe("transferPercent", () => {
  it("总量还不知道时返回 null，而不是画一个 0%", () => {
    expect(transferPercent(entry("a", { done: 0, total: 0 }))).toBeNull();
    expect(transferPercent(entry("a", { done: 8192, total: 0 }))).toBeNull();
    expect(transferPercent(entry("a", { done: 1, total: -1 }))).toBeNull();
  });

  it("四舍五入成整数", () => {
    expect(transferPercent(entry("a", { done: 5, total: 10 }))).toBe(50);
    expect(transferPercent(entry("a", { done: 1, total: 3 }))).toBe(33);
    expect(transferPercent(entry("a", { done: 2, total: 3 }))).toBe(67);
  });

  // 进度回调与文件大小之间差几个字节是常事（远端把目录也算进去过一次），越界要夹住，
  // 否则条带画出 103% 而那个宽度是不合法的。
  it("超过 100 与负数都夹在两端", () => {
    expect(transferPercent(entry("a", { done: 200, total: 100 }))).toBe(100);
    expect(transferPercent(entry("a", { done: -5, total: 100 }))).toBe(0);
  });
});

describe("isValidRemotePath", () => {
  it("只收以 / 开头的绝对路径", () => {
    expect(isValidRemotePath("/a/b")).toBe(true);
    expect(isValidRemotePath("/opt/app/")).toBe(true);
    expect(isValidRemotePath("  /opt/app  ")).toBe(true);
  });

  it("~ 与相对路径都不行（~ 是字面量，相对路径按会话当前目录解释）", () => {
    expect(isValidRemotePath("~/x")).toBe(false);
    expect(isValidRemotePath("~")).toBe(false);
    expect(isValidRemotePath("x/y")).toBe(false);
    expect(isValidRemotePath("./x")).toBe(false);
    expect(isValidRemotePath("")).toBe(false);
    expect(isValidRemotePath("   ")).toBe(false);
  });

  it("带 .. 段的一律拒绝", () => {
    expect(isValidRemotePath("/a/../b")).toBe(false);
    expect(isValidRemotePath("/a/..")).toBe(false);
    // `..abc` 是文件名的一部分，不是上一级。
    expect(isValidRemotePath("/a/..abc")).toBe(true);
  });
});

describe("joinRemoteDir", () => {
  it("补上末尾那一个斜杠，重复的收成一个", () => {
    expect(joinRemoteDir("/opt/app")).toBe("/opt/app/");
    expect(joinRemoteDir("/opt/app/")).toBe("/opt/app/");
    expect(joinRemoteDir("/opt/app///")).toBe("/opt/app/");
    expect(joinRemoteDir("  /opt/app  ")).toBe("/opt/app/");
  });

  // 空串兜出一个 `/` 等于把「没填」变成「传到根目录」，那是静默兜底。
  it("空值仍然是空值", () => {
    expect(joinRemoteDir("")).toBe("");
    expect(joinRemoteDir("   ")).toBe("");
  });
});

describe("readStoredRemoteDirs", () => {
  it("认得出的是这份按服务器 id 分的表", () => {
    expect(readStoredRemoteDirs('{"s1":"/opt/app/","s2":"/data/"}')).toEqual({
      s1: "/opt/app/",
      s2: "/data/",
    });
    expect(REMOTE_DIR_STORAGE_KEY).toBe("deploycode:transfer.remoteDir");
  });

  it("没存过、不是 JSON、不是对象都当没存过，不抛出去", () => {
    expect(readStoredRemoteDirs(null)).toEqual({});
    expect(readStoredRemoteDirs("")).toEqual({});
    expect(readStoredRemoteDirs("not json")).toEqual({});
    expect(readStoredRemoteDirs("[]")).toEqual({});
    expect(readStoredRemoteDirs('"字符串"')).toEqual({});
    expect(readStoredRemoteDirs("null")).toEqual({});
  });

  // 别的版本（或手改过的盘）可能把值写成数字 / 对象 / 空串：喂进输入框比空着更糟。
  it("非字符串与空串的值丢掉，其余留着", () => {
    expect(readStoredRemoteDirs('{"s1":123}')).toEqual({});
    expect(readStoredRemoteDirs('{"s1":123,"s2":"/opt/"}')).toEqual({ s2: "/opt/" });
    expect(readStoredRemoteDirs('{"s1":"","s2":"/opt/"}')).toEqual({ s2: "/opt/" });
    expect(readStoredRemoteDirs('{"s1":null,"s2":{}}')).toEqual({});
  });
});
