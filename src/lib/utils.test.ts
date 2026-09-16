import { describe, expect, it, vi } from "vitest";

// utils 里只有文案类函数依赖 i18n，测试统一用直返 key 的桩。
vi.mock("./i18n", () => ({ default: { t: (key: string) => key } }));

import {
  cn,
  deployStatusClass,
  formatDuration,
  githubTarget,
  inferEnvRemotePath,
  maskUrlPassword,
  shortPath,
  statusBadgeKind,
} from "./utils";

describe("formatDuration", () => {
  it("0 显示占位符，毫秒级不换算", () => {
    expect(formatDuration(0)).toBe("-");
    expect(formatDuration(500)).toBe("500ms");
  });

  it("按秒四舍五入，跨分钟用 m/s 组合", () => {
    expect(formatDuration(1500)).toBe("1.5s");
    expect(formatDuration(59_999)).toBe("1m0s");
    expect(formatDuration(61_000)).toBe("1m1s");
    expect(formatDuration(90_000)).toBe("1m30s");
  });
});

describe("inferEnvRemotePath", () => {
  it("仓库内文件保留相对子目录", () => {
    expect(inferEnvRemotePath("E:/proj/lf_resume/backend/.env", "E:\\proj\\lf_resume")).toBe(
      "backend/.env",
    );
    expect(inferEnvRemotePath("/home/dev/app/docker/app.env", "/home/dev/app")).toBe(
      "docker/app.env",
    );
  });

  it("仓库根目录下的文件只取文件名", () => {
    expect(inferEnvRemotePath("E:/proj/app/.env", "E:/proj/app")).toBe(".env");
  });

  it("仓库外文件退回文件名", () => {
    expect(inferEnvRemotePath("D:/secrets/prod.env", "E:/proj/app")).toBe("prod.env");
    expect(inferEnvRemotePath("D:/secrets/prod.env", null)).toBe("prod.env");
  });

  it("盘符与目录大小写不敏感", () => {
    expect(inferEnvRemotePath("e:/Proj/App/backend/.env", "E:/proj/app")).toBe("backend/.env");
  });

  it("同名前缀目录不误判（app2 不命中 app）", () => {
    expect(inferEnvRemotePath("E:/proj/app2/.env", "E:/proj/app")).toBe(".env");
  });
});

describe("maskUrlPassword", () => {
  it("遮蔽 userinfo 中的密码", () => {
    expect(maskUrlPassword("postgresql://user:secret@host:5432/db")).toBe(
      "postgresql://user:***@host:5432/db",
    );
    expect(maskUrlPassword("postgres://u:p%40ss%3Aword@h/db")).toBe("postgres://u:***@h/db");
  });

  it("密码含未编码 / 时按最后一个 @ 分界", () => {
    expect(maskUrlPassword("postgresql://admin:888888/password@host/db")).toBe(
      "postgresql://admin:***@host/db",
    );
  });

  it("遮蔽查询参数中的 password（保留片段）", () => {
    expect(maskUrlPassword("postgresql://u@h/db?Password=secret")).toBe(
      "postgresql://u@h/db?Password=***",
    );
    expect(maskUrlPassword("postgresql://u:p@h/db?a=1&password=x#frag")).toBe(
      "postgresql://u:***@h/db?a=1&password=***#frag",
    );
  });

  it("无 scheme 时原样返回", () => {
    expect(maskUrlPassword("not-a-url")).toBe("not-a-url");
  });
});

describe("githubTarget", () => {
  it("scp 与 URL 形式都能解析并推导 Pages 地址", () => {
    expect(githubTarget("git@github.com:owner/repo.git")).toEqual({
      owner: "owner",
      repo: "repo",
      url: "https://owner.github.io/repo/",
    });
    expect(githubTarget("https://github.com/owner/repo.git#main")).toEqual({
      owner: "owner",
      repo: "repo",
      url: "https://owner.github.io/repo/",
    });
  });

  it("用户主页仓库（owner.github.io）使用根地址", () => {
    expect(githubTarget("https://github.com/owner/owner.github.io")).toEqual({
      owner: "owner",
      repo: "owner.github.io",
      url: "https://owner.github.io/",
    });
  });

  it("非 GitHub 或信息不足时返回 null", () => {
    expect(githubTarget(null)).toBeNull();
    expect(githubTarget("https://gitlab.com/owner/repo.git")).toBeNull();
    expect(githubTarget("https://github.com/owner")).toBeNull();
  });
});

describe("工具函数", () => {
  it("shortPath 超长时保留尾部并加省略号", () => {
    expect(shortPath("/a/b", 10)).toBe("/a/b");
    const long = "/very/long/path/to/some/file.txt";
    const short = shortPath(long, 10);
    expect(short).toHaveLength(10);
    expect(short.startsWith("…")).toBe(true);
    expect(long.endsWith(short.slice(1))).toBe(true);
  });

  it("cn 过滤空值并用空格连接", () => {
    expect(cn("a", false, undefined, "b", null)).toBe("a b");
  });

  it("deployStatusClass 覆盖三种状态", () => {
    expect(deployStatusClass("success")).toContain("text-pos");
    expect(deployStatusClass("failed")).toContain("text-neg");
    expect(deployStatusClass("running")).toContain("text-warn");
  });

  it("statusBadgeKind 覆盖三种状态", () => {
    expect(statusBadgeKind("success")).toBe("green");
    expect(statusBadgeKind("failed")).toBe("red");
    expect(statusBadgeKind("running")).toBe("amber");
  });
});
