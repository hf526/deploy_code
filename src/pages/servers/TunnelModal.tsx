import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Plus, RotateCw, Save, Trash2 } from "lucide-react";

import { Badge, Button, Checkbox, Field, Input, Modal } from "../../components/ui";
import { api } from "../../lib/api";
import { useApp } from "../../lib/store";
import {
  activeRuleCount,
  activeTunnelCount,
  draftToRule,
  emptyTunnelDraft,
  findTunnelStatus,
  tunnelEndpoint,
  tunnelTone,
  type TunnelDraft,
} from "../../lib/tunnel";
import type { ServerConfig, ServerTunnelStatus, TunnelRule } from "../../lib/types";

/** 四种整体状态的文案 key：在线 / 等重连 / 端口绑不上 / 没在跑。 */
const TONE_KEYS = {
  green: "servers.tunnelConnected",
  amber: "servers.tunnelRetrying",
  red: "servers.tunnelPortBlocked",
  gray: "servers.tunnelOff",
} as const;

/**
 * 一台服务器的 SSH 隧道弹窗：规则编辑 + 运行状态。
 *
 * 规则保存在服务器配置里，所以「保存」之后后端会立刻按新规则重建监听；
 * 应用下次启动时也会自动接上，不需要再点一次。
 */
export function TunnelModal({
  open,
  server,
  onClose,
  onSaved,
}: {
  open: boolean;
  server: ServerConfig;
  onClose: () => void;
  onSaved: (saved: ServerConfig) => void;
}) {
  const { t } = useTranslation();
  const toast = useApp((state) => state.toast);

  const [rules, setRules] = useState<TunnelRule[]>([]);
  const [draft, setDraft] = useState<TunnelDraft>(emptyTunnelDraft);
  const [status, setStatus] = useState<ServerTunnelStatus | null>(null);
  const [saving, setSaving] = useState(false);
  const [reconnecting, setReconnecting] = useState(false);
  const [refreshing, setRefreshing] = useState(false);

  // 请求序号 + 当前服务器：慢回包不能盖到另一台机器上。
  const seq = useRef(0);
  const serverIdRef = useRef(server.id);
  serverIdRef.current = server.id;

  const refreshStatus = useCallback(async () => {
    const request = ++seq.current;
    const target = serverIdRef.current;
    setRefreshing(true);
    try {
      const list = await api.listTunnelStatus();
      if (request === seq.current && target === serverIdRef.current) {
        setStatus(findTunnelStatus(list, target) ?? null);
      }
    } catch {
      // 状态读不到不打断编辑：规则本身在配置里，不依赖这次请求。
    } finally {
      if (request === seq.current) setRefreshing(false);
    }
  }, []);

  useEffect(() => {
    if (!open) return;
    setRules(server.tunnels.map((rule) => ({ ...rule })));
    setDraft(emptyTunnelDraft());
    void refreshStatus();
    // 依赖只认「打开」与「哪一台」：父组件刷新列表会换 tunnels 数组身份，
    // 把它列进来会在用户编辑中途清空未保存的草稿。
  }, [open, server.id, refreshStatus]);

  function addRule() {
    const { rule, errorKey } = draftToRule(draft, rules);
    if (!rule) {
      toast("error", t(errorKey ?? "servers.tunnelErrorPort", { port: draft.localPort }));
      return;
    }
    setRules((current) => [...current, rule]);
    setDraft({ ...emptyTunnelDraft(), remoteHost: rule.remoteHost });
  }

  async function save() {
    if (saving) return;
    setSaving(true);
    try {
      const saved = await api.saveServerTunnels(server.id, rules);
      // 后端补齐了规则 id，用它替换本地草稿，状态徽章才能对上号。
      setRules(saved.tunnels.map((rule) => ({ ...rule })));
      toast("success", t("servers.tunnelSaved"));
      onSaved(saved);
      await refreshStatus();
    } catch (error) {
      toast("error", String(error));
    } finally {
      setSaving(false);
    }
  }

  async function reconnect() {
    if (reconnecting) return;
    setReconnecting(true);
    try {
      await api.reconnectTunnels(server.id);
      // 重连是后台事：给一点建立时间再读状态，读不到就靠刷新按钮。
      await new Promise((resolve) => setTimeout(resolve, 800));
      await refreshStatus();
    } catch (error) {
      toast("error", String(error));
    } finally {
      setReconnecting(false);
    }
  }

  const enabledCount = activeTunnelCount(rules);
  const tone = tunnelTone(status, enabledCount);

  return (
    <Modal
      open={open}
      onClose={onClose}
      title={t("servers.tunnelTitle", { name: server.name })}
      subtitle={t("servers.tunnelSubtitle")}
      width="max-w-2xl"
      footer={
        <>
          <Button variant="secondary" disabled={saving || reconnecting} onClick={onClose}>
            {t("common.close")}
          </Button>
          <Button loading={saving} disabled={reconnecting} onClick={() => void save()}>
            <Save className="size-4" /> {t("common.save")}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-4">
        <section className="rounded-md border border-line bg-field px-3.5 py-3">
          <div className="flex flex-wrap items-center gap-2">
            <Badge kind={tone}>{t(TONE_KEYS[tone])}</Badge>
            <span className="text-[11.5px] text-ink-dim">
              {status?.connected
                ? t("servers.tunnelLive", {
                    ports: activeRuleCount(status),
                    count: status.forwarded,
                  })
                : status && status.retries > 0
                  ? t("servers.tunnelRetries", { count: status.retries })
                  : t("servers.tunnelIdle")}
            </span>
            {status?.connectedAt && (
              <span className="font-mono text-[11px] text-ink-faint">{status.connectedAt}</span>
            )}
            <div className="ml-auto flex items-center gap-1.5">
              <Button
                size="sm"
                variant="ghost"
                icon={<RotateCw className="size-3.5" />}
                loading={refreshing}
                disabled={reconnecting}
                onClick={() => void refreshStatus()}
              >
                {t("servers.tunnelRefresh")}
              </Button>
              <Button
                size="sm"
                variant="secondary"
                disabled={enabledCount === 0 || saving}
                loading={reconnecting}
                onClick={() => void reconnect()}
              >
                {t("servers.tunnelReconnect")}
              </Button>
            </div>
          </div>
          {status?.lastError && (
            <p className="mt-2 rounded-md border border-neg/30 bg-neg-soft px-2.5 py-1.5 text-[11.5px] break-all text-neg">
              {status.lastError}
            </p>
          )}
          <p className="mt-2 text-[11px] text-ink-faint">{t("servers.tunnelHint")}</p>
        </section>

        <section>
          <h4 className="mb-2 text-xs font-semibold text-ink">{t("servers.tunnelRules")}</h4>
          {rules.length === 0 ? (
            <p className="text-xs text-ink-faint">{t("servers.tunnelEmpty")}</p>
          ) : (
            <ul className="divide-y divide-line overflow-hidden rounded-md border border-line">
              {rules.map((rule, index) => {
                const item = status?.rules.find((entry) => entry.ruleId === rule.id);
                return (
                  <li key={rule.id || `${rule.localPort}-${index}`} className="flex items-center gap-3 px-3 py-2">
                    <Checkbox
                      checked={rule.enabled}
                      onChange={(checked) =>
                        setRules((current) =>
                          current.map((entry, position) =>
                            position === index ? { ...entry, enabled: checked } : entry,
                          ),
                        )
                      }
                    >
                      <span className="sr-only">{t("servers.tunnelEnabled")}</span>
                    </Checkbox>
                    <span className="min-w-0 flex-1 truncate font-mono text-xs text-ink">
                      {tunnelEndpoint(rule)}
                    </span>
                    {rule.enabled && item && !item.bound && (
                      <Badge kind="red">{t("servers.tunnelBindFailed")}</Badge>
                    )}
                    {rule.enabled && item?.bound && !item.active && <Badge kind="amber">{t("servers.tunnelWaiting")}</Badge>}
                    {rule.enabled && item?.active && <Badge kind="green">{t("servers.tunnelListening")}</Badge>}
                    {!rule.enabled && <Badge kind="gray">{t("servers.tunnelPaused")}</Badge>}
                    <Button
                      size="sm"
                      variant="ghost"
                      title={t("common.delete")}
                      className="text-neg hover:bg-neg-soft hover:text-neg"
                      icon={<Trash2 className="size-3.5" />}
                      onClick={() =>
                        setRules((current) => current.filter((_, position) => position !== index))
                      }
                    />
                  </li>
                );
              })}
            </ul>
          )}
        </section>

        <section className="rounded-md border border-line bg-field p-3.5">
          <div className="flex flex-wrap items-end gap-2">
            <Field label={t("servers.tunnelLocalPort")} className="w-28">
              <Input
                type="number"
                value={draft.localPort}
                placeholder="18080"
                onChange={(event) => setDraft({ ...draft, localPort: event.target.value })}
              />
            </Field>
            <Field label={t("servers.tunnelRemoteHost")} className="min-w-40 flex-1">
              <Input
                value={draft.remoteHost}
                placeholder="127.0.0.1"
                onChange={(event) => setDraft({ ...draft, remoteHost: event.target.value })}
              />
            </Field>
            <Field label={t("servers.tunnelRemotePort")} className="w-28">
              <Input
                type="number"
                value={draft.remotePort}
                placeholder="8080"
                onChange={(event) => setDraft({ ...draft, remotePort: event.target.value })}
              />
            </Field>
            <Button variant="secondary" icon={<Plus className="size-4" />} onClick={addRule}>
              {t("servers.tunnelAdd")}
            </Button>
          </div>
          <p className="mt-2 text-[11px] text-ink-faint">{t("servers.tunnelFormHint")}</p>
        </section>
      </div>
    </Modal>
  );
}
