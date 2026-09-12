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
import { useApp } from "../lib/store";
import type { OnlineSession, SecurityReport, ServerConfig, SecuritySetting } from "../lib/types";
import { authSummary, newServerTemplate } from "../lib/utils";

const FIREWALL_LABEL: Record<string, string> = {
  ufw: "UFW",
  firewalld: "firewalld",
  iptables: "iptables",
  none: "未检测到防火墙",
  unknown: "防火墙未知",
};

function settingTone(setting: SecuritySetting): "green" | "red" | "amber" | "gray" {
  if (setting.key === "permitrootlogin") return setting.value.startsWith("yes") ? "red" : "green";
  if (setting.key === "passwordauthentication") {
    return setting.value.startsWith("yes") ? "amber" : "green";
  }
  return "gray";
}

export default function ServersPage() {
  const servers = useApp((state) => state.servers);
  const refreshServers = useApp((state) => state.refreshServers);
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
      toast("success", `已删除服务器 ${removing.name}`);
      setRemoving(null);
      await refreshServers();
    } catch (error) {
      toast("error", String(error));
    } finally {
      setBusy(false);
    }
  }

  async function handleTest(server: ServerConfig) {
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
      title="服务器"
      subtitle="部署服务器：SSH 连接信息与默认目录"
      actions={
        <Button icon={<Plus className="size-4" />} onClick={() => openForm(null)}>
          添加服务器
        </Button>
      }
    >
      {servers.length === 0 ? (
        <EmptyState
          icon={<Server className="size-4.5" />}
          title="还没有部署服务器"
          description="添加一台服务器并测试连接，即可在部署页面选择它。"
          action={
            <Button icon={<Plus className="size-4" />} onClick={() => openForm(null)}>
              添加服务器
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
                    <p className="text-[11px] text-ink-faint">未设置默认目录</p>
                  )}
                </div>
                <div className="flex shrink-0 items-center gap-1">
                  <Button
                    size="sm"
                    variant="ghost"
                    title="测试连接"
                    loading={testingId === server.id}
                    icon={<Wifi className="size-3.5" />}
                    onClick={() => void handleTest(server)}
                  />
                  <Button
                    size="sm"
                    variant="secondary"
                    title="安全检查"
                    icon={<ShieldCheck className="size-3.5" />}
                    onClick={() => openSecurity(server)}
                  >
                    安全
                  </Button>
                  <Button size="sm" variant="secondary" onClick={() => openForm(server)}>
                    配置
                  </Button>
                  <Button
                    size="sm"
                    variant="ghost"
                    title="删除"
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
        title="安全检查"
        subtitle={
          securing
            ? `${securing.name} · ${securing.username}@${securing.host}:${securing.port}`
            : undefined
        }
        width="max-w-3xl"
        footer={
          <>
            <Button variant="secondary" onClick={closeSecurity}>
              关闭
            </Button>
            <Button
              variant="secondary"
              loading={scanning}
              icon={<RefreshCw className="size-4" />}
              onClick={() => securing && void scanSecurity(securing)}
            >
              重新扫描
            </Button>
          </>
        }
      >
        {scanning && !report ? (
          <p className="flex items-center justify-center gap-2 py-12 text-xs text-ink-dim">
            <Loader2 className="size-4 animate-spin" /> 正在收集登录日志与防火墙信息 ...
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
                防火墙：{FIREWALL_LABEL[report.firewall] ?? report.firewall}
              </Badge>
              <Badge kind={report.isRoot || report.hasSudo ? "green" : "amber"}>
                {report.isRoot ? "root 权限" : report.hasSudo ? "免密 sudo 可用" : "权限受限"}
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
                在线会话 <b className="ml-1 text-ink">{report.sessions.length}</b>
              </span>
              <span className="rounded-md border border-line bg-field px-2.5 py-1 text-ink-dim">
                上次扫描 <b className="ml-1 text-ink">{report.scannedAt || "-"}</b>
              </span>
            </div>

            <section>
              <h4 className="mb-2 text-xs font-semibold text-ink">
                登录失败（按账号 / IP 汇总，共 {report.failed.length} 组）
              </h4>
              {report.failed.length === 0 ? (
                <p className="text-xs text-ink-faint">未发现失败登录记录。</p>
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
                      <Badge kind="red">{item.count} 次</Badge>
                      {report.blocked.includes(item.ip) ? (
                        <Button
                          size="sm"
                          variant="secondary"
                          loading={ipBusy === item.ip}
                          disabled={ipBusy !== null && ipBusy !== item.ip}
                          icon={<X className="size-3.5" />}
                          onClick={() => void handleUnblock(item.ip)}
                        >
                          解除
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
                          拉黑
                        </Button>
                      )}
                    </li>
                  ))}
                </ul>
              )}
            </section>

            <section>
              <h4 className="mb-2 text-xs font-semibold text-ink">最近成功登录</h4>
              {report.success.length === 0 ? (
                <p className="text-xs text-ink-faint">没有读取到登录记录。</p>
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
                当前在线会话（{report.sessions.length}）
              </h4>
              {report.sessions.length === 0 ? (
                <p className="text-xs text-ink-faint">当前没有在线登录会话。</p>
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
                        踢出
                      </Button>
                    </li>
                  ))}
                </ul>
              )}
            </section>

            <section>
              <h4 className="mb-2 text-xs font-semibold text-ink">防火墙拦截（DENY / DROP）</h4>
              {report.blocked.length === 0 ? (
                <p className="text-xs text-ink-faint">当前没有拦截中的 IP。</p>
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
                        {ipBusy === ip ? "解除中..." : "解除"}
                      </button>
                    </span>
                  ))}
                </div>
              )}
            </section>

            <section>
              <h4 className="mb-2 text-xs font-semibold text-ink">自动防护（服务器端）</h4>
              <div className="rounded-md border border-line bg-field p-3.5">
                <div className="flex flex-wrap items-center gap-2">
                  <Badge kind={report.guardEnabled ? "green" : "gray"}>
                    {report.guardEnabled ? "已启用" : "未启用"}
                  </Badge>
                  <span className="text-[11.5px] text-ink-dim">
                    服务器每分钟检查一次，统计窗口内同一 IP 登录失败达到阈值后自动拉黑（不依赖本应用运行）
                  </span>
                </div>
                <div className="mt-3 flex flex-wrap items-end gap-2">
                  <Field label="失败次数阈值" className="w-28">
                    <Input
                      type="number"
                      value={guardThreshold}
                      onChange={(event) => setGuardThreshold(Number(event.target.value))}
                    />
                  </Field>
                  <Field label="统计窗口（分钟）" className="w-32">
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
                    {report.guardEnabled ? "更新配置" : "启用防护"}
                  </Button>
                  {report.guardEnabled && (
                    <Button
                      variant="ghost"
                      className="text-neg hover:bg-neg-soft hover:text-neg"
                      disabled={guardBusy}
                      onClick={() => void handleDisableGuard()}
                    >
                      停用
                    </Button>
                  )}
                </div>
                {!report.isRoot && !report.hasSudo && (
                  <p className="mt-2 text-[11px] text-ink-faint">
                    需要 root 或免密 sudo 权限才能在服务器上安装防护脚本。
                  </p>
                )}
              </div>
            </section>

            <section>
              <h4 className="mb-2 text-xs font-semibold text-ink">sshd 关键配置</h4>
              {report.sshd.length === 0 ? (
                <p className="text-xs text-ink-faint">未读取到 sshd 配置。</p>
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
        title="删除服务器"
        confirmText="删除"
        description={
          <span>
            确定删除服务器 <b className="text-ink">{removing?.name}</b> 吗？
            使用该服务器的部署记录不会被删除。
          </span>
        }
        onCancel={() => setRemoving(null)}
        onConfirm={() => void handleDelete()}
      />

      <ConfirmModal
        open={!!kicking}
        danger
        loading={sessionBusy !== null}
        title="踢出在线会话"
        confirmText="踢出"
        description={
          <span>
            确定强制断开 <b className="text-ink">{kicking?.user}</b> 在{" "}
            <b className="text-ink">{kicking?.tty}</b>
            {kicking?.from ? `（来源 ${kicking.from}）` : ""} 的登录会话吗？
          </span>
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
    const selected = await open({ multiple: false, title: "选择 SSH 私钥文件" });
    if (typeof selected === "string") {
      setDraft((current) =>
        current.auth.type === "privateKey"
          ? { ...current, auth: { ...current.auth, keyPath: selected } }
          : current,
      );
    }
  }

  async function save() {
    if (!draft.name.trim()) return toast("error", "请填写服务器名称");
    if (!draft.host.trim()) return toast("error", "请填写主机地址");
    if (!draft.username.trim()) return toast("error", "请填写 SSH 用户名");
    if (draft.auth.type === "password" && !draft.auth.password) {
      return toast("error", "请填写登录密码");
    }
    if (draft.auth.type === "privateKey" && !draft.auth.keyPath.trim()) {
      return toast("error", "请选择私钥文件");
    }

    setBusy(true);
    try {
      const saved = await api.saveServer(draft);
      toast("success", initial ? "服务器已更新" : "服务器已添加");
      setDraft(saved);
      await onSaved();
    } catch (error) {
      toast("error", String(error));
    } finally {
      setBusy(false);
    }
  }

  async function test() {
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

  return (
    <Modal
      open={isOpen}
      onClose={onClose}
      title={initial ? `编辑服务器 · ${initial.name}` : "添加服务器"}
      subtitle="通过 SSH 将代码上传到该服务器"
      footer={
        <>
          <Button
            variant="secondary"
            loading={testing}
            icon={<Wifi className="size-4" />}
            onClick={() => void test()}
          >
            测试连接
          </Button>
          <Button variant="secondary" onClick={onClose}>
            取消
          </Button>
          <Button loading={busy} onClick={() => void save()}>
            保存
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-4">
        <div className="grid grid-cols-2 gap-4">
          <Field label="名称" required>
            <Input
              value={draft.name}
              onChange={(event) => setDraft({ ...draft, name: event.target.value })}
              placeholder="生产服务器"
            />
          </Field>
          <Field label="主机地址" required>
            <Input
              value={draft.host}
              onChange={(event) => setDraft({ ...draft, host: event.target.value })}
              placeholder="192.168.1.10 或 example.com"
            />
          </Field>
        </div>

        <div className="grid grid-cols-2 gap-4">
          <Field label="SSH 端口">
            <Input
              type="number"
              value={draft.port}
              onChange={(event) => setDraft({ ...draft, port: Number(event.target.value) || 22 })}
            />
          </Field>
          <Field label="用户名" required>
            <Input
              value={draft.username}
              onChange={(event) => setDraft({ ...draft, username: event.target.value })}
              placeholder="root"
            />
          </Field>
        </div>

        <Field label="认证方式">
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
            <option value="password">密码认证</option>
            <option value="privateKey">私钥认证</option>
          </Select>
        </Field>

        {draft.auth.type === "password" ? (
          <Field label="登录密码" required>
            <Input
              type="password"
              value={draft.auth.password}
              onChange={(event) =>
                setDraft({ ...draft, auth: { type: "password", password: event.target.value } })
              }
              placeholder="仅保存在本机配置文件"
            />
          </Field>
        ) : (
          <>
            <Field label="私钥文件" required>
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
                  浏览
                </Button>
              </div>
            </Field>
            <Field label="私钥口令" hint="没有留空">
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

        <Field label="默认部署目录" hint="创建部署任务时自动填充">
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
