import { describe, expect, it } from "vitest";

import { WHITELIST_MAX, addWhitelistEntry, blockGuardReason, isSingleIp, normalizeIpEntry } from "./security";

describe("isSingleIp", () => {
  it("接受单个 IPv4 与 IPv6", () => {
    for (const value of ["1.2.3.4", "255.255.255.255", "2001:db8::1", "[2001:db8::1]", "fe80::1%eth0"]) {
      expect(isSingleIp(value)).toBe(true);
    }
  });

  it("拒绝网段、主机名与残缺地址", () => {
    for (const value of ["10.0.0.0/8", "example.com", "1.2.3", "999.1.1.1", "01.2.3.4", "1:2", ""]) {
      expect(isSingleIp(value)).toBe(false);
    }
  });

  it("拒绝全零地址与 IPv4 上挂 zone id", () => {
    // 全零在防火墙语义里是"任意地址"，收进白名单等于对所有来源免封。
    expect(isSingleIp("0.0.0.0")).toBe(false);
    expect(isSingleIp("::")).toBe(false);
    expect(isSingleIp("1.2.3.4%eth0")).toBe(false);
  });

  it("接受内嵌 IPv4 的 IPv6 写法", () => {
    expect(isSingleIp("::ffff:1.2.3.4")).toBe(true);
  });
});

describe("addWhitelistEntry", () => {
  it("去空白后追加，并按规范化结果去重", () => {
    const added = addWhitelistEntry(["1.2.3.4"], " 5.6.7.8 ");
    expect(added).toEqual({ ok: true, entries: ["1.2.3.4", "5.6.7.8"] });
    expect(addWhitelistEntry(["2001:db8::1"], "[2001:0DB8::1]").ok).toBe(false);
    expect(addWhitelistEntry(["fe80::1"], "fe80::1%eth0")).toEqual({ ok: false, reason: "duplicate" });
  });

  it("空输入与非法输入都算 invalid", () => {
    expect(addWhitelistEntry([], "")).toEqual({ ok: false, reason: "invalid" });
    expect(addWhitelistEntry([], "10.0.0.0/8")).toEqual({ ok: false, reason: "invalid" });
  });

  it("超过上限时拒绝新增", () => {
    const full = Array.from({ length: WHITELIST_MAX }, (_, i) => `10.0.${Math.floor(i / 256)}.${i % 256}`);
    expect(addWhitelistEntry(full, "1.2.3.4")).toEqual({ ok: false, reason: "full" });
  });
});

describe("blockGuardReason", () => {
  it("标出当前连接来源与白名单地址", () => {
    expect(blockGuardReason([], "1.2.3.4", "1.2.3.4")).toBe("self");
    expect(blockGuardReason(["5.6.7.8"], "", "5.6.7.8")).toBe("whitelisted");
    expect(blockGuardReason(["5.6.7.8"], "1.2.3.4", "9.9.9.9")).toBeNull();
  });

  it("来源未知时不拿空串当保护理由", () => {
    expect(blockGuardReason([], "", "")).toBeNull();
  });
});

describe("normalizeIpEntry", () => {
  it("剥掉方括号与 zone id、统一小写并去掉每组前导零", () => {
    expect(normalizeIpEntry(" [2001:0DB8::1] ")).toBe("2001:db8::1");
    expect(normalizeIpEntry("FE80::1%eth0")).toBe("fe80::1");
    expect(normalizeIpEntry("0.0.0.0")).toBe("0.0.0.0");
  });
});
