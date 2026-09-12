import { useEffect, useMemo, useState } from "react";
import {
  ChevronDown,
  ChevronRight,
  Database,
  Play,
  RefreshCw,
  Save,
  ShieldCheck,
  Trash2,
} from "lucide-react";

import { LogConsole } from "../components/LogConsole";
import {
  Badge,
  Button,
  Card,
  ConfirmModal,
  EmptyState,
  Field,
  Input,
  Page,
  SectionTitle,
  Select,
} from "../components/ui";
import { api } from "../lib/api";
import { useApp } from "../lib/store";
import type { BackupRecord, BackupRequest, DbBackupSource, ServerConfig } from "../lib/types";

function defaultSource(): DbBackupSource {
  return {
    mode: "docker",
    container: "postgres",
    database: "",
    username: "postgres",
    password: "",
    schema: "public",
  };
}

function sourceOf(server: ServerConfig | null): DbBackupSource {
  return server?.dbBackup ?? defaultSource();
}

function normalized(source: DbBackupSource): DbBackupSource {
  return {
    ...source,
    container: source.container.trim(),
    database: source.database.trim(),
    username: source.username.trim(),
    schema: source.schema.trim() || "public",
  };
}

function statusBadge(record: BackupRecord) {
  if (record.status === "success") return <Badge kind="green">成功</Badge>;
  if (record.status === "failed") return <Badge kind="red">失败</Badge>;
  return <Badge kind="amber">进行中</Badge>;
}

function humanSize(bytes: number): string {
  if (!bytes) return "-";
  const units = ["B", "KB", "MB", "GB"];
  let value = bytes;
  let index = 0;
  while (value >= 1024 && index < units.length - 1) {
    value /= 1024;
    index += 1;
  }
  return `${value.toFixed(index === 0 ? 0 : 1)} ${units[index]}`;
}

export default function BackupsPage() {
  const servers = useApp((state) => state.servers);
  const backups = useApp((state) => state.backups);
  const backupTargets = useApp((state) => state.backupTargets);
  const settings = useApp((state) => state.settings);
  const liveBackup = useApp((state) => state.liveBackup);
  const startBackup = useApp((state) => state.startBackup);
  const refreshBackups = useApp((state) => state.refreshBackups);
  const refreshServers = useApp((state) => state.refreshServers);
  const clearLiveBackup = useApp((state) => state.clearLiveBackup);
  const toast = useApp((state) => state.toast);

  const [serverId, setServerId] = useState("");
  const [draft, setDraft] = useState<DbBackupSource>(defaultSource());
  const [targetId, setTargetId] = useState("");
  const [overrideUrl, setOverrideUrl] = useState("");
  const [saving, setSaving] = useState(false);
  const [testing, setTesting] = useState(false);
  const [starting, setStarting] = useState(false);
  const [expanded, setExpanded] = useState<string | null>(null);
  const [confirmClear, setConfirmClear] = useState(false);

  const selected = useMemo(
    () => servers.find((server) => server.id === serverId) ?? null,
    [servers, serverId],
  );

  useEffect(() => {
    if (!serverId && servers.length > 0) {
      setServerId(servers[0].id);
    }
  }, [servers, serverId]);

  useEffect(() => {
    setDraft(sourceOf(selected));
    setTargetId(selected?.backupTargetId ?? "");
    setOverrideUrl(selected?.supabaseUrl ?? "");
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selected?.id]);

  const running = liveBackup?.status === "running";

  const targetById = (id: string | null | undefined) =>
    backupTargets.find((target) => target.id === id) ?? null;
  // 服务器可能绑定了已被删除的目标：悬空 id 一律按未绑定处理，避免后端报"目标不存在"。
  const validTargetId = backupTargets.some((target) => target.id === targetId)
    ? targetId
    : "";
  const effectiveTarget = useMemo(() => {
    if (overrideUrl.trim()) {
      return { name: "自定义连接串", url: overrideUrl.trim() };
    }
    const bound = targetById(validTargetId);
    if (bound) return { name: bound.name, url: bound.url };
    if (selected?.supabaseUrl?.trim()) {
      return { name: "服务器自定义连接串", url: selected.supabaseUrl.trim() };
    }
    const global = targetById(settings.defaultBackupTargetId);
    if (global) return { name: global.name, url: global.url };
    if (settings.supabaseUrl.trim()) {
      return { name: "旧版连接串", url: settings.supabaseUrl.trim() };
    }
    return null;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [overrideUrl, validTargetId, selected, settings, backupTargets]);

  /** 以服务端最新配置为基准合并表单，避免整对象回写覆盖 CLI / 其他窗口的修改。 */
  async function buildServer(): Promise<ServerConfig | null> {
    if (!selected) return null;
    let base = selected;
    try {
      const list = await api.listServers();
      base = list.find((server) => server.id === selected.id) ?? selected;
    } catch {
      // 读取失败时退回当前副本
    }
    return {
      ...base,
      dbBackup: normalized(draft),
      backupTargetId: validTargetId || null,
      supabaseUrl: overrideUrl.trim() ? overrideUrl.trim() : null,
    };
  }

  /** 用当前表单构造备份请求，测试与实际备份使用同一份来源与目标。 */
  function buildRequest(server: ServerConfig): BackupRequest {
    return {
      serverId: server.id,
      targetId: validTargetId || null,
      supabaseUrl: overrideUrl.trim() || null,
      database: server.dbBackup?.database || null,
      schema: server.dbBackup?.schema || null,
    };
  }

  async function handleSave() {
    const server = await buildServer();
    if (!server) return;
    setSaving(true);
    try {
      const saved = await api.saveServer(server);
      await refreshServers();
      toast("success", `已保存 ${saved.name} 的备份配置`);
    } catch (error) {
      toast("error", String(error));
    } finally {
      setSaving(false);
    }
  }

  async function handleTest() {
    if (testing || starting) return;
    const server = await buildServer();
    if (!server) return;
    setTesting(true);
    try {
      const message = await api.testBackup(buildRequest(server));
      toast("success", message || "环境检查通过");
    } catch (error) {
      toast("error", String(error));
    } finally {
      setTesting(false);
    }
  }

  async function handleStart() {
    if (starting || testing) return;
    const server = await buildServer();
    if (!server) return;
    if (!server.dbBackup?.database) {
      toast("error", "请先填写数据库名");
      return;
    }
    setStarting(true);
    try {
      // 先把当前表单保存到服务器配置，保证 CLI/后续备份使用同一份参数。
      await api.saveServer(server);
      await refreshServers();
      clearLiveBackup();
      await startBackup(buildRequest(server));
    } catch (error) {
      toast("error", String(error));
    } finally {
      setStarting(false);
    }
  }

  async function handleDelete(recordId: string) {
    try {
      await api.deleteBackup(recordId);
      await refreshBackups();
    } catch (error) {
      toast("error", String(error));
    }
  }

  async function handleClear() {
    try {
      await api.clearBackups();
      await refreshBackups();
      toast("success", "已清空备份记录");
    } catch (error) {
      toast("error", String(error));
    } finally {
      setConfirmClear(false);
    }
  }

  return (
    <Page
      title="数据库备份"
      subtitle="把服务器上的 PostgreSQL schema 全量同步到 Supabase"
      actions={
        <Button
          icon={<RefreshCw className="size-4" />}
          variant="secondary"
          onClick={() => void refreshBackups()}
        >
          刷新记录
        </Button>
      }
    >
      <div className="grid grid-cols-1 gap-6 xl:grid-cols-[400px_minmax(0,1fr)]">
        <div className="flex flex-col gap-6">
          <section>
            <SectionTitle title="数据来源" description="服务器上的 PostgreSQL" />
            <Card className="flex flex-col gap-4 p-5">
              <Field label="服务器" required>
                <Select
                  value={serverId}
                  onChange={(event) => setServerId(event.target.value)}
                  disabled={servers.length === 0}
                >
                  {servers.length === 0 && <option value="">暂无服务器</option>}
                  {servers.map((server) => (
                    <option key={server.id} value={server.id}>
                      {server.name}（{server.username}@{server.host}）
                    </option>
                  ))}
                </Select>
              </Field>

              <div className="grid grid-cols-2 gap-4">
                <Field label="采集方式">
                  <Select
                    value={draft.mode}
                    onChange={(event) =>
                      setDraft({ ...draft, mode: event.target.value as DbBackupSource["mode"] })
                    }
                  >
                    <option value="docker">Docker 容器</option>
                    <option value="system">服务器本机</option>
                  </Select>
                </Field>
                <Field label="Schema">
                  <Input
                    value={draft.schema}
                    onChange={(event) => setDraft({ ...draft, schema: event.target.value })}
                    placeholder="public"
                  />
                </Field>
              </div>

              {draft.mode === "docker" && (
                <Field label="容器名" hint="如 postgres" required>
                  <Input
                    value={draft.container}
                    onChange={(event) => setDraft({ ...draft, container: event.target.value })}
                    placeholder="postgres"
                  />
                </Field>
              )}

              <div className="grid grid-cols-2 gap-4">
                <Field label="数据库名" required>
                  <Input
                    value={draft.database}
                    onChange={(event) => setDraft({ ...draft, database: event.target.value })}
                    placeholder="app"
                  />
                </Field>
                <Field label="用户名" required>
                  <Input
                    value={draft.username}
                    onChange={(event) => setDraft({ ...draft, username: event.target.value })}
                    placeholder="postgres"
                  />
                </Field>
              </div>

              <Field label="密码" hint="容器内本地认证可留空">
                <Input
                  type="password"
                  value={draft.password}
                  onChange={(event) => setDraft({ ...draft, password: event.target.value })}
                  placeholder="数据库密码"
                />
              </Field>

              <div className="flex justify-end gap-2 border-t border-line pt-4">
                <Button
                  variant="secondary"
                  loading={testing}
                  disabled={!selected || starting}
                  onClick={() => void handleTest()}
                >
                  <ShieldCheck className="size-4" />
                  测试环境
                </Button>
                <Button
                  variant="secondary"
                  loading={saving}
                  disabled={!selected || testing || starting}
                  onClick={() => void handleSave()}
                >
                  <Save className="size-4" />
                  保存
                </Button>
              </div>
            </Card>
          </section>

          <section>
            <SectionTitle title="备份目标" description="全量覆盖：清空目标 schema 后重新导入" />
            <Card className="flex flex-col gap-4 p-5">
              <Field label="使用目标" hint="在设置页可添加 Supabase / Aiven / Neon 等">
                <Select
                  value={validTargetId}
                  onChange={(event) => setTargetId(event.target.value)}
                >
                  <option value="">
                    全局默认
                    {targetById(settings.defaultBackupTargetId)
                      ? `（${targetById(settings.defaultBackupTargetId)?.name}）`
                      : ""}
                  </option>
                  {backupTargets.map((target) => (
                    <option key={target.id} value={target.id}>
                      {target.name}
                    </option>
                  ))}
                </Select>
              </Field>
              <Field label="自定义连接串" hint="可选，优先级最高">
                <Input
                  value={overrideUrl}
                  onChange={(event) => setOverrideUrl(event.target.value)}
                  placeholder="postgresql://user:password@host:5432/postgres"
                />
              </Field>
              <p className="text-[11px] leading-relaxed text-ink-faint">
                {effectiveTarget ? (
                  <>
                    将写入：
                    <span className="ml-1 font-medium text-ink-dim">{effectiveTarget.name}</span>
                    <span className="ml-1 font-mono text-ink-faint">
                      {effectiveTarget.url.replace(/:[^:@/]+@/, ":***@")}
                    </span>
                  </>
                ) : (
                  <span className="text-warn">
                    尚未配置备份目标，请到设置页添加，或在上方填写自定义连接串。
                  </span>
                )}
              </p>
              <Button
                size="lg"
                loading={running || starting}
                disabled={!selected || !effectiveTarget || testing}
                onClick={() => void handleStart()}
              >
                <Play className="size-4" />
                {running ? "备份进行中 ..." : "开始备份"}
              </Button>
            </Card>
          </section>
        </div>

        <div className="flex min-w-0 flex-col gap-6">
          <section className="flex min-h-[320px] flex-col">
            <SectionTitle
              title="实时日志"
              description={running ? "备份执行中，请勿关闭应用" : "最近一次备份的实时输出"}
            />
            {liveBackup && (
              <div className="mb-2 h-1 overflow-hidden rounded-full bg-line">
                <div
                  className="h-full rounded-full bg-brand transition-all"
                  style={{ width: `${liveBackup.progress}%` }}
                />
              </div>
            )}
            <LogConsole
              className="min-h-[280px] flex-1"
              title="备份日志"
              emptyText="点击「开始备份」后这里会显示实时日志 ..."
              lines={liveBackup?.lines ?? []}
            />
          </section>

          <section>
            <SectionTitle
              title="备份记录"
              description={`共 ${backups.length} 条`}
              actions={
                backups.length > 0 ? (
                  <Button variant="ghost" size="sm" onClick={() => setConfirmClear(true)}>
                    清空记录
                  </Button>
                ) : undefined
              }
            />
            {backups.length === 0 ? (
              <EmptyState
                icon={<Database className="size-4.5" />}
                title="暂无备份记录"
                description="配置好数据来源和 Supabase 目标后，点击「开始备份」即可。"
              />
            ) : (
              <Card className="divide-y divide-line overflow-hidden">
                {backups.map((record) => (
                  <div key={record.id}>
                    <div className="flex items-center gap-3 px-4 py-3">
                      <button
                        className="flex min-w-0 flex-1 items-center gap-3 text-left"
                        onClick={() => setExpanded(expanded === record.id ? null : record.id)}
                      >
                        {expanded === record.id ? (
                          <ChevronDown className="size-3.5 shrink-0 text-ink-faint" />
                        ) : (
                          <ChevronRight className="size-3.5 shrink-0 text-ink-faint" />
                        )}
                        <div className="min-w-0 flex-1">
                          <p className="truncate text-[13px] font-medium text-ink">
                            {record.serverName} · {record.database}
                            <span className="ml-2 text-[11px] font-normal text-ink-faint">
                              schema {record.schema}
                            </span>
                          </p>
                          <p className="mt-0.5 truncate text-[11px] text-ink-faint">
                            {record.startedAt} · {humanSize(record.dumpSize)} ·{" "}
                            {record.targetName || "目标"} · {record.target}
                          </p>
                        </div>
                        {statusBadge(record)}
                      </button>
                      <Button
                        variant="ghost"
                        size="sm"
                        onClick={() => void handleDelete(record.id)}
                        title="删除记录"
                      >
                        <Trash2 className="size-3.5" />
                      </Button>
                    </div>
                    {expanded === record.id && (
                      <pre className="max-h-80 overflow-auto border-t border-line bg-sunken px-4 py-3 font-mono text-[11px] leading-relaxed whitespace-pre-wrap text-ink-dim">
                        {record.error ? `错误: ${record.error}\n\n` : ""}
                        {record.log || "（无日志）"}
                      </pre>
                    )}
                  </div>
                ))}
              </Card>
            )}
          </section>
        </div>
      </div>

      <ConfirmModal
        open={confirmClear}
        danger
        title="清空备份记录"
        confirmText="清空"
        description="将删除全部备份记录（不会影响 Supabase 中的数据），此操作不可恢复。"
        onCancel={() => setConfirmClear(false)}
        onConfirm={() => void handleClear()}
      />
    </Page>
  );
}
