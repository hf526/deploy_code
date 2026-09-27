import { describe, expect, it } from "vitest";

import {
  DEFAULT_TUNNEL_HOST,
  activeRuleCount,
  activeTunnelCount,
  draftToRule,
  emptyTunnelDraft,
  findTunnelStatus,
  tunnelEndpoint,
  tunnelTone,
} from "./tunnel";
import type { ServerTunnelStatus, TunnelRule } from "./types";

function rule(patch: Partial<TunnelRule> = {}): TunnelRule {
  return {
    id: "r1",
    localPort: 18080,
    remoteHost: "127.0.0.1",
    remotePort: 8080,
    enabled: true,
    ...patch,
  };
}

function status(patch: Partial<ServerTunnelStatus> = {}): ServerTunnelStatus {
  return {
    serverId: "s1",
    serverName: "prod-a",
    connected: true,
    retries: 0,
    lastError: null,
    connectedAt: "2026-09-27 10:00:00",
    forwarded: 3,
    rules: [{ ruleId: "r1", localPort: 18080, remoteLabel: "127.0.0.1:8080", bound: true, active: true, error: null }],
    ...patch,
  };
}

describe("draftToRule", () => {
  it("生成启用状态的规则，id 留给后端补", () => {
    const draft = { localPort: "18080", remoteHost: " 127.0.0.1 ", remotePort: "8080" };
    const { rule: made, errorKey } = draftToRule(draft, []);
    expect(errorKey).toBeNull();
    expect(made).toEqual({
      id: "",
      localPort: 18080,
      remoteHost: "127.0.0.1",
      remotePort: 8080,
      enabled: true,
    });
  });

  it("拒绝越界与非整数端口", () => {
    for (const bad of ["0", "65536", "80.5", "", "abc"]) {
      expect(draftToRule({ localPort: bad, remoteHost: "127.0.0.1", remotePort: "8080" }, []).errorKey).toBe(
        "servers.tunnelErrorPort",
      );
      expect(draftToRule({ localPort: "18080", remoteHost: "127.0.0.1", remotePort: bad }, []).errorKey).toBe(
        "servers.tunnelErrorPort",
      );
    }
  });

  it("拒绝空的远端地址", () => {
    expect(draftToRule({ localPort: "18080", remoteHost: "   ", remotePort: "8080" }, []).errorKey).toBe(
      "servers.tunnelErrorHost",
    );
  });

  it("本机端口与已有启用规则相撞时拒绝", () => {
    const existing = [rule({ localPort: 18080 })];
    expect(draftToRule({ localPort: "18080", remoteHost: "127.0.0.1", remotePort: "9000" }, existing).errorKey).toBe(
      "servers.tunnelErrorDuplicate",
    );
  });

  it("禁用的旧规则不占端口", () => {
    const existing = [rule({ localPort: 18080, enabled: false })];
    expect(draftToRule({ localPort: "18080", remoteHost: "127.0.0.1", remotePort: "9000" }, existing).rule).not.toBeNull();
  });
});

describe("emptyTunnelDraft", () => {
  it("远端地址预置为本机回环", () => {
    expect(emptyTunnelDraft().remoteHost).toBe(DEFAULT_TUNNEL_HOST);
    expect(emptyTunnelDraft().localPort).toBe("");
  });
});

describe("展示辅助", () => {
  it("endpoint 文本带两侧地址", () => {
    expect(tunnelEndpoint(rule())).toBe("localhost:18080 → 127.0.0.1:8080");
  });

  it("只数启用的规则", () => {
    expect(activeTunnelCount([rule(), rule({ id: "r2", localPort: 15432, enabled: false })])).toBe(1);
  });

  it("按服务器 id 找状态", () => {
    const list = [status({ serverId: "s2" })];
    expect(findTunnelStatus(list, "s2")?.serverName).toBe("prod-a");
    expect(findTunnelStatus(list, "s1")).toBeUndefined();
  });

  it("活跃条数只算 active 的规则", () => {
    expect(activeRuleCount(status())).toBe(1);
    expect(activeRuleCount(status({ rules: [{ ruleId: "r1", localPort: 18080, remoteLabel: "x", bound: true, active: false, error: null }] }))).toBe(
      0,
    );
    expect(activeRuleCount(undefined)).toBe(0);
  });
});

describe("tunnelTone", () => {
  it("没有启用规则或没有状态时是灰", () => {
    expect(tunnelTone(status(), 0)).toBe("gray");
    expect(tunnelTone(undefined, 2)).toBe("gray");
  });

  it("在线是绿，端口绑不上是红，其余等重连算琥珀", () => {
    expect(tunnelTone(status(), 1)).toBe("green");
    expect(
      tunnelTone(status({ connected: false, rules: [{ ruleId: "r1", localPort: 18080, remoteLabel: "x", bound: false, active: false, error: "占用" }] }), 1),
    ).toBe("red");
    expect(tunnelTone(status({ connected: false }), 1)).toBe("amber");
  });
});
