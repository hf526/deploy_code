import { beforeEach, describe, expect, it, vi } from "vitest";

import type { Settings } from "./types";

/** 捕获即时保存真正提交出去的那份 body（`saveSettingsGroup` 的测试要用）。 */
const sent: { body: Settings | null } = { body: null };

vi.mock("./i18n", () => ({
  normalizeLanguagePreference: (value: string) => (value === "en-US" ? "en-US" : "zh-CN"),
}));

vi.mock("./api", () => ({
  api: {
    saveSettings: (body: Settings) => {
      sent.body = body;
      return Promise.resolve(body);
    },
  },
}));

/** 只替掉 store 的读/写两个口，`saveSettingsGroup` 依赖 `useApp.getState().settings`。 */
let committed: Settings;
vi.mock("./store", () => ({
  useApp: {
    getState: () => ({ settings: committed }),
    setState: () => undefined,
  },
}));

const {
  SETTINGS_FIELD_GROUPS,
  enqueueSettingsSave,
  mergedGroup,
  normalizeSettings,
  saveSettingsGroup,
} = await import("./settingsDraft");

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
    // 保留数的兜底是后端同一个默认值 2；非法值不能悄悄多留一个包（曾经回落成 3）。
    expect(normalizeSettings(settings({ containerBundleKeep: Number("x") })).containerBundleKeep).toBe(2);
    expect(normalizeSettings(settings({ containerBundleKeep: -1 })).containerBundleKeep).toBe(2);
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
      scheduledContainerConfigIds: ["c1", "c2"],
      masterPasswordHash: "hash",
    });

    const body = mergedGroup(saved, { ...saved, connectTimeoutSecs: 1, releaseKeep: 3 }, DEPLOY_GROUP);

    expect(body.connectTimeoutSecs).toBe(3);
    expect(body.releaseKeep).toBe(3);
    expect(body.containerBundleKeep).toBe(0);
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

describe("saveSettingsGroup：模块页的即时保存也只提交本组", () => {
  // 定时备份三个字段就是 BackupsPage 那一组的全部内容。
  const SCHEDULE_KEYS = [
    "scheduledBackupEnabled",
    "scheduledBackupTime",
    "scheduledBackupConfigId",
  ] as const satisfies readonly (keyof Settings)[];

  beforeEach(() => {
    sent.body = null;
    committed = settings();
  });

  it("只提交本组字段，别组已落盘的值原样带上", async () => {
    committed = settings({ connectTimeoutSecs: 42, githubToken: "已落盘的 token" });

    await saveSettingsGroup(SCHEDULE_KEYS, { scheduledBackupEnabled: true });

    expect(sent.body?.scheduledBackupEnabled).toBe(true);
    expect(sent.body?.connectTimeoutSecs).toBe(42);
    expect(sent.body?.githubToken).toBe("已落盘的 token");
    // 没碰过的调度字段保持落盘值，不会被抹成默认。
    expect(sent.body?.scheduledBackupTime).toBe("03:00");
  });

  it("本组字段照样过规范化：越界的数值被夹回边界", async () => {
    await saveSettingsGroup(SCHEDULE_KEYS, { scheduledBackupTime: "99:99" });

    // 非法时间回落默认值（这是「时间和正则」那三条重复控件共同的兜底）。
    expect(sent.body?.scheduledBackupTime).toBe("03:00");
  });

  it("提交的是排队时刻的快照，不是调用时刻的旧快照", async () => {
    committed = settings({ githubToken: "旧" });

    // 第一次提交还没跑完就把 store 里的值换成新的（模拟别处刚落盘的一份）。
    const first = saveSettingsGroup(SCHEDULE_KEYS, { scheduledBackupEnabled: true });
    committed = settings({ githubToken: "新" });
    await first;

    await saveSettingsGroup(SCHEDULE_KEYS, { scheduledBackupTime: "05:00" });

    // 第二次不能拿着「旧」把刚落盘的「新」顶回去。
    expect(sent.body?.githubToken).toBe("新");
    expect(sent.body?.scheduledBackupTime).toBe("05:00");
  });
});

describe("SETTINGS_FIELD_GROUPS：每个 Settings 字段都要有归属", () => {
  it("字段名与 fixture 逐个对齐（漏登记或多登记都会红）", () => {
    // fixture 的字段集就是 Settings 的全部字段；TS 侧另外还有一层编译期保险
    // （`Record<keyof Settings, SettingsGroupName>`），这条负责挡住拼错名字那种情况。
    expect(Object.keys(SETTINGS_FIELD_GROUPS).sort()).toEqual(Object.keys(settings()).sort());
  });

  it("没有字段被漏掉 normalizeSettings 之外的兜底（数值字段都有限幅）", () => {
    // 哨兵：把每个数值字段塞成越界值，normalizeSettings 后不该留下原值。
    const wild = normalizeSettings(
      settings({
        connectTimeoutSecs: 0,
        scriptTimeoutSecs: 0,
        historyLimit: 0,
        pagesHistoryLimit: 0,
        backupHistoryLimit: 0,
        backupTimeoutSecs: 0,
        containerHistoryLimit: 0,
        containerTimeoutSecs: 0,
        releaseKeep: 0,
      }),
    );
    expect(wild.connectTimeoutSecs).toBeGreaterThanOrEqual(3);
    expect(wild.scriptTimeoutSecs).toBeGreaterThanOrEqual(10);
    expect(wild.historyLimit).toBeGreaterThanOrEqual(20);
    expect(wild.pagesHistoryLimit).toBeGreaterThanOrEqual(10);
    expect(wild.backupHistoryLimit).toBeGreaterThanOrEqual(10);
    expect(wild.backupTimeoutSecs).toBeGreaterThanOrEqual(60);
    expect(wild.containerHistoryLimit).toBeGreaterThanOrEqual(10);
    expect(wild.containerTimeoutSecs).toBeGreaterThanOrEqual(300);
    expect(wild.releaseKeep).toBeGreaterThanOrEqual(1);
  });
});
