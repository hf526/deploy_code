import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  AlertTriangle,
  ArrowLeftRight,
  Boxes,
  Container as ContainerIcon,
  Download,
  FolderOpen,
  RefreshCw,
  RotateCcw,
  Square,
} from "lucide-react";
import { useTranslation } from "react-i18next";

import { LogConsole } from "../components/LogConsole";
import { ExpandableRecordRow, RecordLog } from "../components/RecordRows";
import {
  Badge,
  Button,
  Card,
  Checkbox,
  ConfirmModal,
  EmptyState,
  Field,
  Input,
  Page,
  SectionTitle,
  Select,
} from "../components/ui";
import { api } from "../lib/api";
import { containerKindLabel, containerSummary } from "../lib/container";
import { useApp } from "../lib/store";
import type { ComposeStack, ComposeStackDetail, ContainerRequest } from "../lib/types";
import { cn, deployStatusLabel, humanSize, statusBadgeKind } from "../lib/utils";
import { RestoreBundleModal } from "./containers/RestoreBundleModal";
import { StackDetail } from "./containers/StackDetail";

/** 迁到另一台机器时才有意义：同一台机器上 compose 项目名不能重复。 */
function targetOptions(
  servers: { id: string; name: string; host: string }[],
  sourceId: string,
) {
  return servers.filter((server) => server.id !== sourceId);
}

export default function ContainersPage() {
  const { t } = useTranslation();
  const servers = useApp((state) => state.servers);
  const records = useApp((state) => state.containerRecords);
  const live = useApp((state) => state.liveContainer);
  const startTransfer = useApp((state) => state.startContainerTransfer);
  const cancelContainer = useApp((state) => state.cancelContainer);
  const refreshRecords = useApp((state) => state.refreshContainerRecords);
  const toast = useApp((state) => state.toast);

  const [serverId, setServerId] = useState("");
  const [stacks, setStacks] = useState<ComposeStack[]>([]);
  const [project, setProject] = useState("");
  const [detail, setDetail] = useState<ComposeStackDetail | null>(null);
  const [loadingStacks, setLoadingStacks] = useState(false);
  const [loadingDetail, setLoadingDetail] = useState(false);

  const [includeVolumes, setIncludeVolumes] = useState(true);
  const [includeImages, setIncludeImages] = useState(true);
  const [pauseSource, setPauseSource] = useState(false);

  const [targetServerId, setTargetServerId] = useState("");
  const [targetDir, setTargetDir] = useState("");
  const [startServices, setStartServices] = useState(true);

  const [confirming, setConfirming] = useState<ContainerRequest | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const [restoring, setRestoring] = useState<{
    bundlePath: string;
    bundleSize: number;
    project: string;
  } | null>(null);
  const [expanded, setExpanded] = useState<string | null>(null);
  const [confirmClear, setConfirmClear] = useState(false);
  const [bundleDir, setBundleDir] = useState("");

  // 查询序号：切换服务器/项目后，旧请求返回时直接丢弃，避免串数据。
  // 两类查询各用一个序号：共用一个时，扫描途中点某一行详情会把在途扫描判为过期，
  // 而 loadingStacks 只在「自己的序号仍是最新」时才复位，扫描按钮就永远转圈。
  const stacksSeq = useRef(0);
  const detailSeq = useRef(0);

  useEffect(() => {
    void api
      .getContainerBackupDir()
      .then(setBundleDir)
      .catch(() => undefined);
  }, []);

  useEffect(() => {
    if (servers.length === 0) {
      setServerId("");
      return;
    }
    if (!servers.some((server) => server.id === serverId)) setServerId(servers[0].id);
  }, [servers, serverId]);

  const targets = useMemo(() => targetOptions(servers, serverId), [servers, serverId]);

  useEffect(() => {
    if (targetServerId && !targets.some((server) => server.id === targetServerId)) {
      setTargetServerId("");
    }
  }, [targets, targetServerId]);

  const loadDetail = useCallback(
    async (targetServer: string, targetProject: string) => {
      const seq = ++detailSeq.current;
      setLoadingDetail(true);
      try {
        const result = await api.inspectComposeStack(targetServer, targetProject);
        if (seq !== detailSeq.current) return;
        setDetail(result);
        // 目标目录默认照抄源机项目目录：compose 项目与路径无关，同名项目换机即可跑。
        setTargetDir((current) => (current.trim() ? current : result.stack.workingDir));
      } catch (error) {
        if (seq !== detailSeq.current) return;
        setDetail(null);
        toast("error", String(error));
      } finally {
        if (seq === detailSeq.current) setLoadingDetail(false);
      }
    },
    [toast],
  );

  const loadStacks = useCallback(
    async (targetServer: string): Promise<void> => {
      const seq = ++stacksSeq.current;
      setLoadingStacks(true);
      try {
        const list = await api.listComposeStacks(targetServer);
        if (seq !== stacksSeq.current) return;
        setStacks(list);
        // 保留仍然存在的选中项，否则清空详情等待用户选。
        setProject((current) =>
          list.some((item) => item.name === current) ? current : "",
        );
      } catch (error) {
        if (seq !== stacksSeq.current) return;
        setStacks([]);
        setProject("");
        toast("error", String(error));
      } finally {
        if (seq === stacksSeq.current) setLoadingStacks(false);
      }
    },
    [toast],
  );

  // 切换服务器：作废在途查询，重新扫描项目。
  useEffect(() => {
    stacksSeq.current += 1;
    detailSeq.current += 1;
    setStacks([]);
    setProject("");
    setDetail(null);
    if (serverId) void loadStacks(serverId);
  }, [serverId, loadStacks]);

  useEffect(() => {
    if (!serverId || !project) {
      setDetail(null);
      return;
    }
    void loadDetail(serverId, project);
  }, [serverId, project, loadDetail]);

  const running = live?.status === "running";
  const selected = stacks.find((item) => item.name === project) ?? null;

  function buildRequest(target: boolean): ContainerRequest | null {
    if (!serverId || !project) {
      toast("error", t("containers.pickProject"));
      return null;
    }
    if (!includeVolumes && !includeImages) {
      toast("error", t("containers.emptyBundle"));
      return null;
    }
    if (!target) return { serverId, project, pauseSource, includeVolumes, includeImages, target: null };
    if (!targetServerId) {
      toast("error", t("containers.pickTarget"));
      return null;
    }
    const dir = targetDir.trim();
    if (!dir.startsWith("/")) {
      toast("error", t("containers.targetDirRequired"));
      return null;
    }
    return {
      serverId,
      project,
      pauseSource,
      includeVolumes,
      includeImages,
      target: { serverId: targetServerId, targetDir: dir, startServices },
    };
  }

  async function submit(request: ContainerRequest) {
    if (submitting) return;
    setSubmitting(true);
    try {
      await startTransfer(request);
      setConfirming(null);
    } catch {
      // store 已弹出错误提示。
    } finally {
      setSubmitting(false);
    }
  }

  /** 迁移要改两台机器，落一次确认；备份只写本机，直接开跑。 */
  function handleStart(target: boolean) {
    const request = buildRequest(target);
    if (!request) return;
    if (target) {
      setConfirming(request);
      return;
    }
    void submit(request);
  }

  async function handleClear() {
    try {
      await api.clearContainerRecords();
      await refreshRecords();
      toast("success", t("containers.cleared"));
    } catch (error) {
      toast("error", String(error));
    } finally {
      setConfirmClear(false);
    }
  }

  async function handleDeleteRecord(recordId: string) {
    try {
      await api.deleteContainerRecord(recordId);
      await refreshRecords();
    } catch (error) {
      toast("error", String(error));
    }
  }

  // 项目目录（compose 文件与 env）总是打包，只有卷和镜像受开关影响；
  // 远端预检按「项目+卷+镜像」再留 30% 余量要空间，漏掉项目目录就会界面说够、开跑报空间不足。
  const estimate = detail
    ? detail.projectBytes +
      (includeVolumes ? detail.volumeBytes : 0) +
      (includeImages ? detail.imageBytes : 0)
    : 0;

  return (
    <Page
      title={t("nav.containers")}
      subtitle={t("containers.subtitle")}
      actions={
        <Button
          variant="secondary"
          size="sm"
          icon={<RefreshCw className="size-3.5" />}
          loading={loadingStacks}
          disabled={!serverId}
          onClick={() => void loadStacks(serverId)}
        >
          {t("containers.rescan")}
        </Button>
      }
    >
      <div className="grid grid-cols-1 gap-6 xl:grid-cols-[minmax(0,1fr)_400px]">
        <div className="flex min-w-0 flex-col gap-6">
          <section>
            <SectionTitle
              title={t("containers.sourceTitle")}
              description={t("containers.sourceDescription")}
            />
            <Card className="flex flex-col gap-4 p-5">
              <Field label={t("containers.server")} required>
                <Select
                  value={serverId}
                  disabled={servers.length === 0 || loadingStacks}
                  onChange={(event) => {
                    stacksSeq.current += 1;
                    detailSeq.current += 1;
                    setServerId(event.target.value);
                  }}
                >
                  {servers.length === 0 && <option value="">{t("containers.noServers")}</option>}
                  {servers.map((server) => (
                    <option key={server.id} value={server.id}>
                      {server.name} · {server.host}
                    </option>
                  ))}
                </Select>
              </Field>

              {servers.length === 0 ? (
                <EmptyState
                  icon={<ContainerIcon className="size-4.5" />}
                  title={t("containers.noServers")}
                  description={t("containers.noServersDescription")}
                />
              ) : stacks.length === 0 && !loadingStacks ? (
                <EmptyState
                  icon={<Boxes className="size-4.5" />}
                  title={t("containers.noStacks")}
                  description={t("containers.noStacksDescription")}
                />
              ) : (
                <div className="divide-y divide-line overflow-hidden rounded-md border border-line">
                  {stacks.map((stack) => {
                    const active = stack.name === project;
                    return (
                      <button
                        key={stack.name}
                        type="button"
                        disabled={running}
                        onClick={() => setProject(active ? "" : stack.name)}
                        className={cn(
                          "flex w-full items-center gap-3 px-4 py-3 text-left transition-colors hover:bg-hover",
                          active && "bg-brand-soft/60",
                        )}
                      >
                        <span
                          className={cn(
                            "grid size-8 shrink-0 place-items-center rounded-md border",
                            active
                              ? "border-brand-line bg-brand-soft text-brand"
                              : "border-line bg-field text-ink-faint",
                          )}
                        >
                          <Boxes className="size-4" />
                        </span>
                        <span className="min-w-0 flex-1">
                          <span className="block truncate text-[13px] font-medium text-ink">
                            {stack.name}
                          </span>
                          <span className="mt-0.5 block truncate text-[11px] text-ink-faint">
                            {stack.services} {t("containers.servicesUnit")} ·{" "}
                            {stack.running}/{stack.services} {t("containers.running")} ·{" "}
                            {stack.workingDir}
                          </span>
                        </span>
                        {active && <Badge kind="brand">{t("containers.selected")}</Badge>}
                      </button>
                    );
                  })}
                </div>
              )}
            </Card>
          </section>

          {(detail || loadingDetail) && (
            <section>
              <SectionTitle
                title={t("containers.bundleTitle")}
                description={t("containers.bundleDescription")}
              />
              <Card className="flex flex-col gap-4 p-5">
                {detail && (
                  <>
                    <StackDetail detail={detail} />
                    <div className="flex flex-col gap-2.5 border-t border-line pt-4">
                      <Checkbox
                        checked={includeVolumes}
                        disabled={running}
                        onChange={setIncludeVolumes}
                      >
                        {t("containers.includeVolumes")}
                      </Checkbox>
                      <Checkbox
                        checked={includeImages}
                        disabled={running}
                        onChange={setIncludeImages}
                      >
                        {t("containers.includeImages")}
                      </Checkbox>
                      <Checkbox checked={pauseSource} disabled={running} onChange={setPauseSource}>
                        {t("containers.pauseSource")}
                      </Checkbox>
                      <p className="text-[11px] leading-relaxed text-ink-faint">
                        {t("containers.pauseSourceHint")}
                      </p>
                    </div>
                  </>
                )}
              </Card>
            </section>
          )}

          <section>
            <SectionTitle
              title={t("containers.targetTitle")}
              description={t("containers.targetDescription")}
            />
            <Card className="flex flex-col gap-4 p-5">
              <div className="grid grid-cols-1 gap-4 md:grid-cols-2">
                <Field label={t("containers.targetServer")} hint={t("containers.targetServerHint")}>
                  <Select
                    value={targetServerId}
                    disabled={running || targets.length === 0}
                    onChange={(event) => setTargetServerId(event.target.value)}
                  >
                    <option value="">{t("containers.pickTarget")}</option>
                    {targets.map((server) => (
                      <option key={server.id} value={server.id}>
                        {server.name} · {server.host}
                      </option>
                    ))}
                  </Select>
                </Field>
                <Field label={t("containers.targetDir")} hint={t("containers.targetDirHint")}>
                  <Input
                    value={targetDir}
                    disabled={running}
                    placeholder="/opt/blog"
                    onChange={(event) => setTargetDir(event.target.value)}
                  />
                </Field>
              </div>
              <Checkbox checked={startServices} disabled={running || !targetServerId} onChange={setStartServices}>
                {t("containers.startServices")}
              </Checkbox>

              <div className="flex flex-wrap items-center gap-2 border-t border-line pt-4">
                <Button
                  icon={<Download className="size-4" />}
                  disabled={running || !selected || loadingDetail}
                  onClick={() => handleStart(false)}
                >
                  {t("containers.backupOnly")}
                </Button>
                <Button
                  variant="secondary"
                  icon={<ArrowLeftRight className="size-4" />}
                  disabled={running || !selected || loadingDetail}
                  onClick={() => handleStart(true)}
                >
                  {t("containers.migrate")}
                </Button>
                {running && (
                  <Button
                    variant="danger"
                    icon={<Square className="size-3.5" />}
                    loading={submitting}
                    onClick={() => void cancelContainer()}
                  >
                    {t("containers.stop")}
                  </Button>
                )}
                <p className="ml-auto text-[11px] text-ink-faint">
                  {estimate > 0
                    ? t("containers.estimate", { size: humanSize(estimate) })
                    : t("containers.estimateUnknown")}
                </p>
              </div>

              <div className="flex items-center gap-2">
                <p
                  className="min-w-0 flex-1 truncate font-mono text-[11px] text-ink-faint"
                  title={bundleDir}
                >
                  {t("containers.bundleLocation")}
                  <span className="ml-1 text-ink-dim">{bundleDir || t("common.loading")}</span>
                </p>
                <Button
                  variant="ghost"
                  size="sm"
                  icon={<FolderOpen className="size-3.5" />}
                  disabled={!bundleDir}
                  onClick={() => void api.revealPath(bundleDir).catch((e) => toast("error", String(e)))}
                >
                  {t("containers.openBundleDir")}
                </Button>
              </div>
            </Card>
          </section>
        </div>

        <div className="flex min-w-0 flex-col gap-6">
          <section className="flex min-h-[320px] flex-col">
            <SectionTitle
              title={t("containers.liveLog")}
              description={running ? t("containers.liveLogRunning") : t("containers.liveLogIdle")}
            />
            {live && (
              <div className="mb-2 h-1 overflow-hidden rounded-full bg-line">
                <div
                  className="h-full rounded-full bg-brand transition-all"
                  style={{ width: `${live.progress}%` }}
                />
              </div>
            )}
            {live?.status === "running" && live.progressMessage && (
              <p className="-mt-1 mb-2 truncate text-[11px] text-ink-faint">
                {live.progressMessage}
              </p>
            )}
            <LogConsole
              className="min-h-[280px] flex-1"
              title={t("containers.logTitle")}
              emptyText={t("containers.logEmpty")}
              lines={live?.lines ?? []}
            />
          </section>

          <section>
            <SectionTitle
              title={t("containers.records")}
              description={t("containers.recordsCount", { count: records.length })}
              actions={
                records.length > 0 ? (
                  <Button variant="ghost" size="sm" onClick={() => setConfirmClear(true)}>
                    {t("containers.clearRecords")}
                  </Button>
                ) : undefined
              }
            />
            {records.length === 0 ? (
              <EmptyState
                icon={<Boxes className="size-4.5" />}
                title={t("containers.noRecords")}
                description={t("containers.noRecordsDescription")}
              />
            ) : (
              <Card className="divide-y divide-line overflow-hidden">
                {records.map((record) => (
                  <ExpandableRecordRow
                    key={record.id}
                    expanded={expanded === record.id}
                    onToggle={() => setExpanded(expanded === record.id ? null : record.id)}
                    badge={
                      <Badge kind={statusBadgeKind(record.status)}>
                        {deployStatusLabel(record.status)}
                      </Badge>
                    }
                    deleteTitle={t("containers.deleteRecord")}
                    onDelete={() => void handleDeleteRecord(record.id)}
                    actions={
                      record.status !== "running" && record.bundlePath ? (
                        <Button
                          variant="secondary"
                          size="sm"
                          title={t("containers.restoreAgain")}
                          disabled={running}
                          onClick={() =>
                            setRestoring({
                              bundlePath: record.bundlePath,
                              bundleSize: record.bundleSize,
                              project: record.project,
                            })
                          }
                        >
                          <RotateCcw className="size-3.5" />
                        </Button>
                      ) : undefined
                    }
                    title={
                      <>
                        <span className="mr-2 text-[11px] font-normal text-ink-faint">
                          {containerKindLabel(record.kind)}
                        </span>
                        {containerSummary(record)}
                      </>
                    }
                    subtitle={
                      <>
                        {record.startedAt} · {humanSize(record.bundleSize)}
                        {record.targetDir ? ` · ${record.targetDir}` : ""} ·{" "}
                        {record.volumes.length} {t("containers.volumes")} ·{" "}
                        {record.images.length} {t("containers.images")}
                      </>
                    }
                    log={
                      <RecordLog
                        error={record.error}
                        errorPrefix={(error) => t("containers.errorPrefix", { error })}
                        log={record.log}
                        emptyText={t("containers.noLog")}
                      />
                    }
                  />
                ))}
              </Card>
            )}
          </section>
        </div>
      </div>

      <ConfirmModal
        open={confirming !== null}
        danger
        loading={submitting}
        title={t("containers.migrateConfirmTitle")}
        confirmText={t("containers.migrate")}
        description={
          confirming && (
            <div className="flex flex-col gap-2">
              <p>{t("containers.migrateConfirmDescription", { project: confirming.project })}</p>
              <ul className="flex flex-col gap-1 text-[12px] text-ink-faint">
                <li>{t("containers.migrateConfirmSource")}: {serverName(servers, confirming.serverId)}</li>
                <li>
                  {t("containers.migrateConfirmTarget")}:{" "}
                  {serverName(servers, confirming.target?.serverId ?? "")} · {confirming.target?.targetDir}
                </li>
                <li className="flex items-center gap-1.5 text-warn">
                  <AlertTriangle className="size-3.5 shrink-0" />
                  {t("containers.migrateConfirmKeepSource")}
                </li>
              </ul>
            </div>
          )
        }
        onCancel={() => setConfirming(null)}
        onConfirm={() => confirming && void submit(confirming)}
      />

      {restoring && (
        <RestoreBundleModal
          bundlePath={restoring.bundlePath}
          bundleSize={restoring.bundleSize}
          project={restoring.project}
          busy={running}
          onClose={() => setRestoring(null)}
          onSubmitted={() => setRestoring(null)}
        />
      )}

      <ConfirmModal
        open={confirmClear}
        danger
        title={t("containers.clearTitle")}
        confirmText={t("common.clear")}
        description={t("containers.clearDescription")}
        onCancel={() => setConfirmClear(false)}
        onConfirm={() => void handleClear()}
      />
    </Page>
  );
}

function serverName(servers: { id: string; name: string }[], id: string): string {
  return servers.find((server) => server.id === id)?.name ?? id;
}
