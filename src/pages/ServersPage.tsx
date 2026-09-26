import { useRef, useState } from "react";
import {
  Ban,
  FolderOpen,
  KeyRound,
  Loader2,
  Lock,
  LogOut,
  Plus,
  RefreshCw,
  Server,
  ShieldCheck,
  Trash2,
  Wifi,
  X,
} from "lucide-react";
import { Trans, useTranslation } from "react-i18next";
import { open } from "@tauri-apps/plugin-dialog";

import {
  Badge,
  Button,
  Card,
  ConfirmModal,
  EmptyState,
  Field,
  Input,
  Modal,
  Page,
  Select,
} from "../components/ui";
import { api } from "../lib/api";
import i18n from "../lib/i18n";
import { useApp } from "../lib/store";
import type { OnlineSession, SecurityReport, ServerConfig, SecuritySetting } from "../lib/types";
import { authSummary, newServerTemplate } from "../lib/utils";

function firewallLabel(firewall: string): string {
  if (firewall === "none") return i18n.t("servers.firewallNone");
  if (firewall === "unknown") return i18n.t("servers.firewallUnknown");
  if (firewall === "ufw") return "UFW";
  return firewall;
}

function settingTone(setting: SecuritySetting): "green" | "red" | "amber" | "gray" {
  if (setting.key === "permitrootlogin") return setting.value.startsWith("yes") ? "red" : "green";
  if (setting.key === "passwordauthentication") {
    return setting.value.startsWith("yes") ? "amber" : "green";
  }
  return "gray";
}

export default function ServersPage() {
  const { t } = useTranslation();
  const servers = useApp((state) => state.servers);
  const refreshServers = useApp((state) => state.refreshServers);
  const refreshBackupConfigs = useApp((state) => state.refreshBackupConfigs);
  const setSettings = useApp((state) => state.setSettings);
  const toast = useApp((state) => state.toast);

  const [formOpen, setFormOpen] = useState(false);
  const [editing, setEditing] = useState<ServerConfig | null>(null);
  const [removing, setRemoving] = useState<ServerConfig | null>(null);
  const [busy, setBusy] = useState(false);
  const [testingId, setTestingId] = useState<string | null>(null);

  const [securing, setSecuring] = useState<ServerConfig | null>(null);
  const [report, setReport] = useState<SecurityReport | null>(null);
  const [scanning, setScanning] = useState(false);
  const [secError, setSecError] = useState<string | null>(null);
  const [ipBusy, setIpBusy] = useState<string | null>(null);
  const [kicking, setKicking] = useState<OnlineSession | null>(null);
  const [sessionBusy, setSessionBusy] = useState<string | null>(null);
  const [guardThreshold, setGuardThreshold] = useState(5);
  const [guardWindowMins, setGuardWindowMins] = useState(10);
  const [guardBusy, setGuardBusy] = useState(false);

  // 扫描请求序号 + 当前打开的服务器：切换/关闭后，旧服务器操作触发的重扫直接丢弃，
  // 避免把 A 的报告显示到 B 上，或对错误的服务器执行拉黑/解除。
  const scanSeq = useRef(0);
  const securingRef = useRef<ServerConfig | null>(null);

  async function scanSecurity(server: ServerConfig) {
    if (securingRef.current?.id !== server.id) return;
    const seq = ++scanSeq.current;
    setScanning(true);
    try {
      const result = await api.scanServerSecurity(server);
      if (seq !== scanSeq.current || securingRef.current?.id !== server.id) return;
      setReport(result);
      setGuardThreshold(result.guardThreshold || 5);
      setGuardWindowMins(result.guardWindowMins || 10);
      setSecError(null);
    } catch (error) {
      if (seq !== scanSeq.current || securingRef.current?.id !== server.id) return;
      setSecError(String(error));
    } finally {
      if (seq === scanSeq.current && securingRef.current?.id === server.id) setScanning(false);
    }
  }

  function openSecurity(server: ServerConfig) {
    securingRef.current = server;
    setSecuring(server);
    setReport(null);
    setSecError(null);
    setIpBusy(null);
    setSessionBusy(null);
    setKicking(null);
    void scanSecurity(server);
  }

  function closeSecurity() {
    securingRef.current = null;
    scanSeq.current++;
    setSecuring(null);
  }

  async function handleBlock(ip: string) {
    if (!securing) return;
    setIpBusy(ip);
    try {
      const message = await api.blockServerIp(securing, ip);
      toast("success", message);
      await scanSecurity(securing);
    } catch (error) {
      toast("error", String(error));
    } finally {
      setIpBusy(null);
    }
  }

  async function handleUnblock(ip: string) {
    if (!securing) return;
    setIpBusy(ip);
    try {
      const message = await api.unblockServerIp(securing, ip);
      toast("success", message);
      await scanSecurity(securing);
    } catch (error) {
      toast("error", String(error));
    } finally {
      setIpBusy(null);
    }
  }

  async function handleKick() {
    if (!kicking || !securing) return;
    setSessionBusy(kicking.tty);
    try {
      const message = await api.kickServerSession(securing, kicking.tty);
      toast("success", message);
      setKicking(null);
      await scanSecurity(securing);
    } catch (error) {
      toast("error", String(error));
    } finally {
      setSessionBusy(null);
    }
  }

  async function handleEnableGuard() {
    if (!securing) return;
    setGuardBusy(true);
    try {
      const message = await api.enableServerGuard(securing, guardThreshold, guardWindowMins);
      toast("success", message);
      await scanSecurity(securing);
    } catch (error) {
      toast("error", String(error));
    } finally {
      setGuardBusy(false);
    }
  }

  async function handleDisableGuard() {
    if (!securing) return;
    setGuardBusy(true);
    try {
      const message = await api.disableServerGuard(securing);
      toast("success", message);
      await scanSecurity(securing);
    } catch (error) {
      toast("error", String(error));
    } finally {
      setGuardBusy(false);
    }
  }

  function openForm(server: ServerConfig | null) {
    setEditing(server);
    setFormOpen(true);
  }

  async function handleDelete() {
    if (!removing) return;
    setBusy(true);
    try {
      await api.deleteServer(removing.id);
      toast("success", t("servers.deleted", { name: removing.name }));
      setRemoving(null);
      // 后端会一并删除该服务器的备份配置并清理定时备份引用，同步刷新避免残留悬空配置 / 选择。
      await Promise.all([refreshServers(), refreshBackupConfigs()]);
      void api
        .getSettings()
        .then(setSettings)
        .catch(() => undefined);
    } catch (error) {
      toast("error", String(error));
    } finally {
      setBusy(false);
    }
  }

  async function handleTest(server: ServerConfig) {
    if (testingId) return;
    setTestingId(server.id);
    try {
      const message = await api.testServer(server);
      toast("success", message);
    } catch (error) {
      toast("error", String(error));
    } finally {
      setTestingId(null);
    }
  }

  return (
    <Page
      title={t("nav.servers")}
      subtitle={t("servers.subtitle")}
      actions={
        <Button icon={<Plus className="size-4" />} onClick={() => openForm(null)}>
          {t("servers.add")}
        </Button>
      }
    >
      {servers.length === 0 ? (
        <EmptyState
          icon={<Server className="size-4.5" />}
          title={t("servers.emptyTitle")}
          description={t("servers.emptyDescription")}
          action={
            <Button icon={<Plus className="size-4" />} onClick={() => openForm(null)}>
              {t("servers.add")}
            </Button>
          }
        />
      ) : (
        <Card className="overflow-hidden">
          <ul className="divide-y divide-line">
            {servers.map((server) => (
              <li
                key={server.id}
                className="group flex items-center gap-4 px-4 py-3 transition-colors hover:bg-hover"
              >
                <span className="grid size-9 shrink-0 place-items-center rounded-md border border-line bg-field">
                  {server.auth.type === "password" ? (
                    <Lock className="size-4 text-ink-dim" />
                  ) : (
                    <KeyRound className="size-4 text-ink-dim" />
                  )}
                </span>
                <div className="min-w-0 flex-1">
                  <div className="flex items-center gap-2">
                    <p className="truncate text-[13px] font-semibold tracking-tight text-ink">
                      {server.name}
                    </p>
                    <Badge kind="gray">{authSummary(server)}</Badge>
                  </div>
                  <p className="mt-0.5 truncate font-mono text-[11px] text-ink-dim">
                    {server.username}@{server.host}:{server.port}
                  </p>
                </div>
                <div className="hidden w-56 shrink-0 truncate md:block" title={server.defaultTargetDir}>
                  {server.defaultTargetDir ? (
                    <p className="truncate font-mono text-[11px] text-ink-faint">
                      {server.defaultTargetDir}
                    </p>
                  ) : (
                    <p className="text-[11px] text-ink-faint">{t("servers.noDefaultDir")}</p>
                  )}
                </div>
                <div className="flex shrink-0 items-center gap-1">
                  <Button
                    size="sm"
                    variant="ghost"
                    title={t("servers.testConnection")}
                    loading={testingId === server.id}
                    disabled={testingId !== null && testingId !== server.id}
                    icon={<Wifi className="size-3.5" />}
                    onClick={() => void handleTest(server)}
                  />
                  <Button
                    size="sm"
                    variant="secondary"
                    title={t("servers.securityTitle")}
                    icon={<ShieldCheck className="size-3.5" />}
                    onClick={() => openSecurity(server)}
                  >
                    {t("servers.securityButton")}
                  </Button>
                  <Button size="sm" variant="secondary" onClick={() => openForm(server)}>
                    {t("servers.configure")}
                  </Button>
                  <Button
                    size="sm"
                    variant="ghost"
                    title={t("common.delete")}
                    className="text-neg hover:bg-neg-soft hover:text-neg"
                    icon={<Trash2 className="size-3.5" />}
                    onClick={() => setRemoving(server)}
                  />
                </div>
              </li>
            ))}
          </ul>
        </Card>
      )}

      <ServerFormModal
        open={formOpen}
        initial={editing}
        onClose={() => setFormOpen(false)}
        onSaved={() => {
          setFormOpen(false);
          void refreshServers();
        }}
      />

      <Modal
        open={!!securing}
        onClose={closeSecurity}
        title={t("servers.securityTitle")}
        subtitle={
          securing
            ? `${securing.name} · ${securing.username}@${securing.host}:${securing.port}`
            : undefined
        }
        width="max-w-3xl"
        footer={
          <>
            <Button variant="secondary" onClick={closeSecurity}>
              {t("common.close")}
            </Button>
            <Button
              variant="secondary"
              loading={scanning}
              icon={<RefreshCw className="size-4" />}
              onClick={() => securing && void scanSecurity(securing)}
            >
              {t("servers.rescan")}
            </Button>
          </>
        }
      >
        {scanning && !report ? (
          <p className="flex items-center justify-center gap-2 py-12 text-xs text-ink-dim">
            <Loader2 className="size-4 animate-spin" /> {t("servers.scanning")}
          </p>
        ) : secError ? (
          <p className="rounded-md border border-neg/30 bg-neg-soft px-4 py-3 text-xs break-all text-neg">
            {secError}
          </p>
        ) : report ? (
          <div className="flex flex-col gap-5">
            <div className="flex flex-wrap items-center gap-2">
              <Badge
                kind={
                  report.firewall === "none" ? "red" : report.firewall === "unknown" ? "amber" : "brand"
                }
              >
                {t("servers.firewall", { value: firewallLabel(report.firewall) })}
              </Badge>
              <Badge kind={report.isRoot || report.hasSudo ? "green" : "amber"}>
                {report.isRoot
                  ? t("servers.privilegeRoot")
                  : report.hasSudo
                    ? t("servers.privilegeSudo")
                    : t("servers.privilegeLimited")}
              </Badge>
            </div>

            {report.notes.length > 0 && (
              <div className="rounded-md border border-warn/30 bg-warn-soft px-3.5 py-2.5 text-[11.5px] leading-relaxed text-warn">
                {report.notes.map((note) => (
                  <p key={note}>{note}</p>
                ))}
              </div>
            )}

            <div className="flex flex-wrap items-center gap-2 text-[11px]">
              <span className="rounded-md border border-line bg-field px-2.5 py-1 text-ink-dim">
                {t("servers.onlineSessions")}{" "}
                <b className="ml-1 text-ink">{report.sessions.length}</b>
              </span>
              <span className="rounded-md border border-line bg-field px-2.5 py-1 text-ink-dim">
                {t("servers.lastScan")}{" "}
                <b className="ml-1 text-ink">{report.scannedAt || "-"}</b>
              </span>
            </div>

            <section>
              <h4 className="mb-2 text-xs font-semibold text-ink">
                {t("servers.failedLogins", { count: report.failed.length })}
              </h4>
              {report.failed.length === 0 ? (
                <p className="text-xs text-ink-faint">{t("servers.noFailedLogins")}</p>
              ) : (
                <ul className="divide-y divide-line overflow-hidden rounded-md border border-line">
                  {report.failed.map((item) => (
                    <li
                      key={`${item.user}-${item.ip}`}
                      className="flex items-center gap-3 px-3 py-1.5 transition-colors hover:bg-hover"
                    >
                      <span
                        className="w-40 shrink-0 truncate font-mono text-xs text-ink"
                        title={item.user}
                      >
                        {item.user}
                      </span>
                      <span className="min-w-0 flex-1 truncate font-mono text-xs text-ink-dim">
                        {item.ip}
                      </span>
                      <Badge kind="red">{t("servers.times", { count: item.count })}</Badge>
                      {report.blocked.includes(item.ip) ? (
                        <Button
                          size="sm"
                          variant="secondary"
                          loading={ipBusy === item.ip}
                          disabled={ipBusy !== null && ipBusy !== item.ip}
                          icon={<X className="size-3.5" />}
                          onClick={() => void handleUnblock(item.ip)}
                        >
                          {t("servers.unblock")}
                        </Button>
                      ) : (
                        <Button
                          size="sm"
                          variant="secondary"
                          loading={ipBusy === item.ip}
                          disabled={ipBusy !== null && ipBusy !== item.ip}
                          icon={<Ban className="size-3.5" />}
                          onClick={() => void handleBlock(item.ip)}
                        >
                          {t("servers.block")}
                        </Button>
                      )}
                    </li>
                  ))}
                </ul>
              )}
            </section>

            <section>
              <h4 className="mb-2 text-xs font-semibold text-ink">{t("servers.recentSuccess")}</h4>
              {report.success.length === 0 ? (
                <p className="text-xs text-ink-faint">{t("servers.noSuccess")}</p>
              ) : (
                <ul className="divide-y divide-line overflow-hidden rounded-md border border-line">
                  {report.success.map((item, index) => (
                    <li
                      key={`${item.user}-${item.ip}-${index}`}
                      className="flex items-center gap-3 px-3 py-1.5"
                    >
                      <span className="w-28 shrink-0 truncate font-mono text-xs text-ink">
                        {item.user}
                      </span>
                      <span className="w-40 shrink-0 truncate font-mono text-xs text-ink-dim">
                        {item.ip}
                      </span>
                      <span
                        className="min-w-0 flex-1 truncate text-[11px] text-ink-faint"
                        title={item.detail}
                      >
                        {item.detail}
                      </span>
                    </li>
                  ))}
                </ul>
              )}
            </section>

            <section>
              <h4 className="mb-2 text-xs font-semibold text-ink">
                {t("servers.sessionsTitle", { count: report.sessions.length })}
              </h4>
              {report.sessions.length === 0 ? (
                <p className="text-xs text-ink-faint">{t("servers.noSessions")}</p>
              ) : (
                <ul className="divide-y divide-line overflow-hidden rounded-md border border-line">
                  {report.sessions.map((session) => (
                    <li
                      key={`${session.user}-${session.tty}`}
                      className="flex items-center gap-3 px-3 py-1.5 transition-colors hover:bg-hover"
                    >
                      <span
                        className="w-28 shrink-0 truncate font-mono text-xs text-ink"
                        title={session.user}
                      >
                        {session.user}
                      </span>
                      <span className="w-20 shrink-0 truncate font-mono text-xs text-ink-dim">
                        {session.tty}
                      </span>
                      <span
                        className="min-w-0 flex-1 truncate text-[11px] text-ink-faint"
                        title={`${session.loginAt}${session.from ? ` · ${session.from}` : ""}`}
                      >
                        {session.loginAt}
                        {session.from ? ` · ${session.from}` : ""}
                      </span>
                      <Button
                        size="sm"
                        variant="secondary"
                        loading={sessionBusy === session.tty}
                        disabled={sessionBusy !== null && sessionBusy !== session.tty}
                        icon={<LogOut className="size-3.5" />}
                        onClick={() => setKicking(session)}
                      >
                        {t("servers.kick")}
                      </Button>
                    </li>
                  ))}
                </ul>
              )}
            </section>

            <section>
              <h4 className="mb-2 text-xs font-semibold text-ink">{t("servers.blockedIps")}</h4>
              {report.blocked.length === 0 ? (
                <p className="text-xs text-ink-faint">{t("servers.noBlockedIps")}</p>
              ) : (
                <div className="flex flex-wrap gap-2">
                  {report.blocked.map((ip) => (
                    <span
                      key={ip}
                      className="inline-flex items-center gap-1.5 rounded-md border border-line bg-field py-0.5 pr-1 pl-2 font-mono text-[11px] text-ink"
                    >
                      {ip}
                      <button
                        type="button"
                        disabled={ipBusy !== null}
                        onClick={() => void handleUnblock(ip)}
                        className="rounded px-1.5 py-0.5 text-[10px] text-ink-faint hover:bg-hover hover:text-ink disabled:opacity-50"
                      >
                        {ipBusy === ip ? t("servers.unblocking") : t("servers.unblock")}
                      </button>
                    </span>
                  ))}
                </div>
              )}
            </section>

            <section>
              <h4 className="mb-2 text-xs font-semibold text-ink">{t("servers.guardTitle")}</h4>
              <div className="rounded-md border border-line bg-field p-3.5">
                <div className="flex flex-wrap items-center gap-2">
                  <Badge kind={report.guardEnabled ? "green" : "gray"}>
                    {report.guardEnabled ? t("servers.guardEnabled") : t("servers.guardDisabled")}
                  </Badge>
                  <span className="text-[11.5px] text-ink-dim">{t("servers.guardDescription")}</span>
                </div>
                <div className="mt-3 flex flex-wrap items-end gap-2">
                  <Field label={t("servers.guardThreshold")} className="w-28">
                    <Input
                      type="number"
                      value={guardThreshold}
                      onChange={(event) => setGuardThreshold(Number(event.target.value))}
                    />
                  </Field>
                  <Field label={t("servers.guardWindow")} className="w-32">
                    <Input
                      type="number"
                      value={guardWindowMins}
                      onChange={(event) => setGuardWindowMins(Number(event.target.value))}
                    />
                  </Field>
                  <Button
                    variant="secondary"
                    loading={guardBusy}
                    icon={<ShieldCheck className="size-3.5" />}
                    onClick={() => void handleEnableGuard()}
                  >
                    {report.guardEnabled ? t("servers.guardUpdate") : t("servers.guardEnable")}
                  </Button>
                  {report.guardEnabled && (
                    <Button
                      variant="ghost"
                      className="text-neg hover:bg-neg-soft hover:text-neg"
                      disabled={guardBusy}
                      onClick={() => void handleDisableGuard()}
                    >
                      {t("servers.guardDisable")}
                    </Button>
                  )}
                </div>
                {!report.isRoot && !report.hasSudo && (
                  <p className="mt-2 text-[11px] text-ink-faint">{t("servers.guardNeedRoot")}</p>
                )}
              </div>
            </section>

            <section>
              <h4 className="mb-2 text-xs font-semibold text-ink">{t("servers.sshdTitle")}</h4>
              {report.sshd.length === 0 ? (
                <p className="text-xs text-ink-faint">{t("servers.noSshd")}</p>
              ) : (
                <div className="grid grid-cols-1 gap-2 sm:grid-cols-2">
                  {report.sshd.map((setting) => (
                    <div
                      key={setting.key}
                      className="flex items-center justify-between gap-2 rounded-md border border-line bg-field px-3 py-1.5"
                    >
                      <span className="truncate font-mono text-[11px] text-ink-dim">
                        {setting.key}
                      </span>
                      <Badge kind={settingTone(setting)}>{setting.value}</Badge>
                    </div>
                  ))}
                </div>
              )}
            </section>
          </div>
        ) : null}
      </Modal>

      <ConfirmModal
        open={!!removing}
        danger
        loading={busy}
        title={t("servers.deleteTitle")}
        confirmText={t("common.delete")}
        description={
          <Trans
            i18nKey="servers.deleteDescription"
            values={{ name: removing?.name }}
            components={{ b: <b className="text-ink" /> }}
          />
        }
        onCancel={() => setRemoving(null)}
        onConfirm={() => void handleDelete()}
      />

      <ConfirmModal
        open={!!kicking}
        danger
        loading={sessionBusy !== null}
        title={t("servers.kickTitle")}
        confirmText={t("servers.kick")}
        description={
          <Trans
            i18nKey={kicking?.from ? "servers.kickDescriptionWithFrom" : "servers.kickDescription"}
            values={{ user: kicking?.user, tty: kicking?.tty, from: kicking?.from }}
            components={{ b: <b className="text-ink" /> }}
          />
        }
        onCancel={() => setKicking(null)}
        onConfirm={() => void handleKick()}
      />
    </Page>
  );
}

function ServerFormModal({
  open: isOpen,
  initial,
  onClose,
  onSaved,
}: {
  open: boolean;
  initial: ServerConfig | null;
  onClose: () => void;
  onSaved: () => void | Promise<void>;
}) {
  const { t } = useTranslation();
  const toast = useApp((state) => state.toast);
  const [draft, setDraft] = useState<ServerConfig>(newServerTemplate);
  const [busy, setBusy] = useState(false);
  const [testing, setTesting] = useState(false);
  const [initialized, setInitialized] = useState(false);

  if (isOpen && !initialized) {
    setDraft(initial ? { ...initial, auth: { ...initial.auth } } : newServerTemplate());
    setInitialized(true);
  }
  if (!isOpen && initialized) {
    setInitialized(false);
  }

  async function browseKey() {
    const selected = await open({ multiple: false, title: t("servers.keyBrowseTitle") });
    if (typeof selected === "string") {
      setDraft((current) =>
        current.auth.type === "privateKey"
          ? { ...current, auth: { ...current.auth, keyPath: selected } }
          : current,
      );
    }
  }

  async function save() {
    if (busy || testing) return;
    if (!draft.name.trim()) return toast("error", t("servers.errorName"));
    if (!draft.host.trim()) return toast("error", t("servers.errorHost"));
    if (!draft.username.trim()) return toast("error", t("servers.errorUsername"));
    if (draft.auth.type === "password" && !draft.auth.password) {
      return toast("error", t("servers.errorPassword"));
    }
    if (draft.auth.type === "privateKey" && !draft.auth.keyPath.trim()) {
      return toast("error", t("servers.errorKey"));
    }

    setBusy(true);
    try {
      const saved = await api.saveServer(draft);
      toast("success", initial ? t("servers.updated") : t("servers.added"));
      setDraft(saved);
      await onSaved();
    } catch (error) {
      toast("error", String(error));
    } finally {
      setBusy(false);
    }
  }

  async function test() {
    if (busy || testing) return;
    setTesting(true);
    try {
      const message = await api.testServer(draft);
      toast("success", message);
    } catch (error) {
      toast("error", String(error));
    } finally {
      setTesting(false);
    }
  }

  /** 保存 / 测试进行中禁止关闭，避免异步结果落到另一个服务器的表单上。 */
  function requestClose() {
    if (busy || testing) return;
    onClose();
  }

  return (
    <Modal
      open={isOpen}
      onClose={requestClose}
      title={initial ? t("servers.editTitle", { name: initial.name }) : t("servers.addTitle")}
      subtitle={t("servers.formSubtitle")}
      onEnter={() => {
        if (!busy && !testing) void save();
      }}
      footer={
        <>
          <Button
            variant="secondary"
            loading={testing}
            disabled={busy}
            icon={<Wifi className="size-4" />}
            onClick={() => void test()}
          >
            {t("servers.testConnection")}
          </Button>
          <Button variant="secondary" disabled={busy || testing} onClick={requestClose}>
            {t("common.cancel")}
          </Button>
          <Button loading={busy} disabled={testing} onClick={() => void save()}>
            {t("common.save")}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-4">
        <div className="grid grid-cols-2 gap-4">
          <Field label={t("servers.name")} required>
            <Input
              value={draft.name}
              onChange={(event) => setDraft({ ...draft, name: event.target.value })}
              placeholder={t("servers.namePlaceholder")}
            />
          </Field>
          <Field label={t("servers.host")} required>
            <Input
              value={draft.host}
              onChange={(event) => setDraft({ ...draft, host: event.target.value })}
              placeholder={t("servers.hostPlaceholder")}
            />
          </Field>
        </div>

        <div className="grid grid-cols-2 gap-4">
          <Field label={t("servers.port")}>
            <Input
              type="number"
              value={draft.port}
              onChange={(event) => setDraft({ ...draft, port: Number(event.target.value) || 22 })}
            />
          </Field>
          <Field label={t("servers.username")} required>
            <Input
              value={draft.username}
              onChange={(event) => setDraft({ ...draft, username: event.target.value })}
              placeholder="root"
            />
          </Field>
        </div>

        <Field label={t("servers.authType")}>
          <Select
            value={draft.auth.type}
            onChange={(event) => {
              const type = event.target.value as "password" | "privateKey";
              if (type === draft.auth.type) return;
              setDraft({
                ...draft,
                auth:
                  type === "password"
                    ? { type: "password", password: "" }
                    : { type: "privateKey", keyPath: "", passphrase: null },
              });
            }}
          >
            <option value="password">{t("servers.passwordAuth")}</option>
            <option value="privateKey">{t("servers.keyAuth")}</option>
          </Select>
        </Field>

        {draft.auth.type === "password" ? (
          <Field label={t("servers.password")} required>
            <Input
              type="password"
              value={draft.auth.password}
              onChange={(event) =>
                setDraft({ ...draft, auth: { type: "password", password: event.target.value } })
              }
              placeholder={t("servers.passwordPlaceholder")}
            />
          </Field>
        ) : (
          <>
            <Field label={t("servers.keyPath")} required>
              <div className="flex gap-2">
                <Input
                  value={draft.auth.keyPath}
                  onChange={(event) =>
                    setDraft({
                      ...draft,
                      auth: {
                        type: "privateKey",
                        keyPath: event.target.value,
                        passphrase: draft.auth.type === "privateKey" ? draft.auth.passphrase : null,
                      },
                    })
                  }
                  placeholder="C:\Users\you\.ssh\id_rsa"
                />
                <Button
                  variant="secondary"
                  icon={<FolderOpen className="size-4" />}
                  onClick={() => void browseKey()}
                >
                  {t("servers.browse")}
                </Button>
              </div>
            </Field>
            <Field label={t("servers.keyPassphrase")} hint={t("servers.keyPassphraseHint")}>
              <Input
                type="password"
                value={draft.auth.type === "privateKey" ? (draft.auth.passphrase ?? "") : ""}
                onChange={(event) =>
                  setDraft({
                    ...draft,
                    auth: {
                      type: "privateKey",
                      keyPath: draft.auth.type === "privateKey" ? draft.auth.keyPath : "",
                      passphrase: event.target.value || null,
                    },
                  })
                }
              />
            </Field>
          </>
        )}

        <Field label={t("servers.defaultTargetDir")} hint={t("servers.defaultTargetDirHint")}>
          <Input
            value={draft.defaultTargetDir}
            onChange={(event) => setDraft({ ...draft, defaultTargetDir: event.target.value })}
            placeholder="/opt/apps/my-app"
          />
        </Field>
      </div>
    </Modal>
  );
}
