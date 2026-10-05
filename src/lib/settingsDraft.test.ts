import { describe, expect, it, vi } from "vitest";

import type { Settings } from "./types";

vi.mock("./i18n", () => ({
  normalizeLanguagePreference: (value: string) => (value === "en-US" ? "en-US" : "zh-CN"),
}));

const { enqueueSettingsSave, mergedGroup, normalizeSettings } = await import("./settingsDraft");

function settings(overrides: Partial<Settings> = {}): Settings {
  return {
    scriptDir: "docker",
    runScripts: true,
    connectTimeoutSecs: 15,
    scriptTimeoutSecs: 1800,
    keepRemoteArchive: false,
    historyLimit: 500,
    supabaseUrl: "",
    defaultBackupTargetId: null,
    backupHistoryLimit: 200,
    backupTimeoutSecs: 3600,
    cloudflareApiToken: "",
    cloudflareAccountId: "",
    masterPasswordHash: null,
    githubToken: "",
    cronjobApiKey: "",
    pagesHistoryLimit: 200,
    containerHistoryLimit: 200,
    containerTimeoutSecs: 7200,
    containerBundleKeep: 2,
    dbBundleKeep: 0,
    language: "zh-CN",
    atomicRelease: false,
    releaseKeep: 5,
    scheduledBackupEnabled: false,
    scheduledBackupTime: "03:00",
    scheduledBackupConfigId: null,
    scheduledBackupLastRun: "",
    scheduledShutdownEnabled: false,
    scheduledShutdownTime: "04:00",
    scheduledShutdownLastRun: "",
    scheduledContainerEnabled: false,
    scheduledContainerTime: "03:30",
    scheduledContainerConfigIds: [],
    scheduledContainerLastRun: "",
    agentServerId: "",
    ...overrides,
  };
}

const DEPLOY_GROUP = [
  "scriptDir",
  "runScripts",
  "connectTimeoutSecs",
  "scriptTimeoutSecs",
  "keepRemoteArchive",
  "historyLimit",
  "atomicRelease",
  "releaseKeep",
] as const satisfies readonly (keyof Settings)[];

describe("normalizeSettings", () => {
  it("保留 0 这个「不自动清理」的有效值", () => {
    expect(normalizeSettings(settings({ containerBundleKeep: 0 })).containerBundleKeep).toBe(0);
  });

  it("数值字段越界时夹到边界，填不进数字时回落默认", () => {
    const clamped = normalizeSettings(
      settings({ connectTimeoutSecs: 1, historyLimit: 5, releaseKeep: 999 }),
    );
    expect(clamped.connectTimeoutSecs).toBe(3);
    expect(clamped.historyLimit).toBe(20);
    expect(clamped.releaseKeep).toBe(50);
    // NaN 这类填不进数字的输入不能把设置改成空值。
    const fallback = normalizeSettings(
      settings({ scriptTimeoutSecs: Number("x"), scheduledBackupTime: "  " }),
    );
    expect(fallback.scriptTimeoutSecs).toBe(1800);
    expect(fallback.scheduledBackupTime).toBe("03:00");
  });
});

describe("mergedGroup", () => {
  it("只提交本组字段，别的板块正在编辑的内容不会被这次保存带下去", () => {
    const saved = settings({ githubToken: "已落盘的旧 token", cloudflareApiToken: "" });
    // 草稿里同时装着部署板块和 GitHub token 的未提交编辑（整份 Settings 传进来的那种旧写法）。
    const draft = settings({
      ...saved,
      scriptDir: "compose",
      historyLimit: 100,
      githubToken: "还没点保存的新 token",
    });

    const body = mergedGroup(saved, draft, DEPLOY_GROUP);

    expect(body.scriptDir).toBe("compose");
    expect(body.historyLimit).toBe(100);
    expect(body.githubToken).toBe("已落盘的旧 token");
  });

  it("本组字段照样过规范化，其余字段原样保留（含未镜像到界面的那些）", () => {
    const saved = settings({
      containerBundleKeep: 0,
      agentServerId: "srv-agent",
      scheduledContainerConfigIds: ["c1", "c2"],
      masterPasswordHash: "hash",
    });

    const body = mergedGroup(saved, { ...saved, connectTimeoutSecs: 1, releaseKeep: 3 }, DEPLOY_GROUP);

    expect(body.connectTimeoutSecs).toBe(3);
    expect(body.releaseKeep).toBe(3);
    expect(body.containerBundleKeep).toBe(0);
    expect(body.agentServerId).toBe("srv-agent");
    expect(body.scheduledContainerConfigIds).toEqual(["c1", "c2"]);
    expect(body.masterPasswordHash).toBe("hash");
  });
});

describe("enqueueSettingsSave：整表回写只许一条车道", () => {
  /**
   * 模拟后端：body 在任务开工那一刻就定稿（真实代码是 mergedGroup 取快照），
   * 落盘要等一会儿（invoke + 写 config.json），期间别的任务可能也在写。
   */
  function fakeStore(initial: Settings) {
    let saved = { ...initial };
    const tick = () => new Promise((resolve) => setTimeout(resolve, 0));
    return {
      current: () => ({ ...saved }),
      submit: (patch: Partial<Settings>) => {
        const body = { ...saved, ...patch };
        return tick().then(() => {
          saved = body;
          return { ...saved };
        });
      },
    };
  }

  it("后一次提交带着前一次刚落盘的结果，而不是自己开工前的快照", async () => {
    const store = fakeStore(settings());

    await Promise.all([
      enqueueSettingsSave(() => store.submit({ language: "en-US" })),
      enqueueSettingsSave(() => store.submit({ githubToken: "gh-new" })),
    ]);

    expect(store.current().language).toBe("en-US");
    expect(store.current().githubToken).toBe("gh-new");
  });

  it("前一次失败不堵车道，后一次照样能提交", async () => {
    const store = fakeStore(settings());
    const failing = enqueueSettingsSave(async () => {
      throw new Error("存不上");
    });
    const ok = enqueueSettingsSave(() => store.submit({ releaseKeep: 9 }));

    await expect(failing).rejects.toThrow("存不上");
    await expect(ok).resolves.toMatchObject({ releaseKeep: 9 });
    expect(store.current().releaseKeep).toBe(9);
  });
});
