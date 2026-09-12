import { useEffect, useMemo, useRef, useState } from "react";
import {
  AlertTriangle,
  CheckCircle2,
  CircleDot,
  GitBranch,
  History,
  Rocket,
  Terminal,
  Timer,
  XCircle,
} from "lucide-react";
import { useNavigate, useSearchParams } from "react-router-dom";

import { LogConsole } from "../components/LogConsole";
import { Badge, Button, Card, Field, Input, Page, Select } from "../components/ui";
import { api } from "../lib/api";
import { useApp } from "../lib/store";
import type { Branch, DeployRecord, ResolvedRev } from "../lib/types";
import { cn, deployStatusClass, deployStatusLabel, formatDuration, shortPath } from "../lib/utils";

export default function DeployPage() {
  const [searchParams] = useSearchParams();
  const navigate = useNavigate();

  const toast = useApp((state) => state.toast);
  const repos = useApp((state) => state.repos);
  const servers = useApp((state) => state.servers);
  const history = useApp((state) => state.history);
  const settings = useApp((state) => state.settings);
  const live = useApp((state) => state.live);
  const startDeploy = useApp((state) => state.startDeploy);
  const redeploy = useApp((state) => state.redeploy);

  const [repoId, setRepoId] = useState(() => searchParams.get("repo") ?? "");
  const [rev, setRev] = useState(() => searchParams.get("rev") ?? "");
  const [customRev, setCustomRev] = useState(() => !!searchParams.get("rev"));
  const [serverId, setServerId] = useState("");
  const [targetDir, setTargetDir] = useState("");
  const [runScripts, setRunScripts] = useState(settings.runScripts);
  const [scriptDir, setScriptDir] = useState(settings.scriptDir);
  const [script, setScript] = useState("");

  const [branches, setBranches] = useState<Branch[]>([]);
  const [resolved, setResolved] = useState<ResolvedRev | null>(null);
  const [resolveError, setResolveError] = useState<string | null>(null);
  const [resolving, setResolving] = useState(false);
  const [submitting, setSubmitting] = useState(false);

  const appliedQuery = useRef("");
  const appliedRepo = useRef("");
  const pendingRev = useRef<{ repoId: string; rev: string } | null>(null);
  const repoIdRef = useRef(repoId);
  repoIdRef.current = repoId;

  useEffect(() => {
    const key = `${searchParams.get("repo") ?? ""}|${searchParams.get("rev") ?? ""}`;
    if (key === appliedQuery.current) return;
    appliedQuery.current = key;
    const queryRepo = searchParams.get("repo");
    const queryRev = searchParams.get("rev");
    if (queryRepo) setRepoId(queryRepo);
    if (queryRev) {
      // 版本绑定到所属仓库，避免切换仓库时把旧仓库的版本套用过去。
      pendingRev.current = { repoId: queryRepo ?? repoIdRef.current, rev: queryRev };
      setRev(queryRev);
      setCustomRev(true);
    }
  }, [searchParams]);

  useEffect(() => {
    if (!repoId && repos.length > 0) setRepoId(repos[0].id);
  }, [repos, repoId]);

  useEffect(() => {
    if (!repoId) return;
    let cancelled = false;
    void (async () => {
      try {
        const list = await api.listBranches(repoId, false);
        if (!cancelled) setBranches(list);
      } catch (error) {
        if (!cancelled) toast("error", String(error));
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [repoId, toast]);

  useEffect(() => {
    if (!repoId || appliedRepo.current === repoId) return;
    const repo = repos.find((item) => item.id === repoId);
    // 仓库列表尚未加载完成时先不标记，等加载后再补默认值。
    if (!repo) return;
    appliedRepo.current = repoId;
    const pending = pendingRev.current;
    pendingRev.current = null;
    if (pending && pending.repoId === repoId) {
      // 深链带入的版本优先于仓库默认分支，否则会被默认值重置覆盖。
      setRev(pending.rev);
      setCustomRev(true);
    } else {
      setRev(repo.currentBranch ?? "");
      setCustomRev(false);
    }
    setTargetDir(repo.defaultTargetDir ?? "");
    setServerId(repo.defaultServerId ?? "");
  }, [repoId, repos]);

  useEffect(() => {
    setRunScripts(settings.runScripts);
    setScriptDir(settings.scriptDir);
  }, [settings]);

  useEffect(() => {
    if (!repoId || !rev.trim()) {
      setResolved(null);
      setResolveError(null);
      setResolving(false);
      return;
    }
    let cancelled = false;
    setResolving(true);
    setResolveError(null);
    setResolved(null);
    const timer = window.setTimeout(() => {
      void (async () => {
        try {
          const result = await api.resolveRev(repoId, rev.trim());
          if (!cancelled) {
            setResolved(result);
            setResolveError(null);
          }
        } catch (error) {
          if (!cancelled) {
            setResolved(null);
            setResolveError(String(error));
          }
        } finally {
          if (!cancelled) setResolving(false);
        }
      })();
    }, 300);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [repoId, rev]);

  const selectedRepo = repos.find((item) => item.id === repoId);
  const selectedServer = servers.find((item) => item.id === serverId);

  const recentDeploys = useMemo(
    () => history.filter((record) => record.repoId === repoId).slice(0, 6),
    [history, repoId],
  );

  async function handleDeploy() {
    if (!repoId) return toast("error", "请选择仓库");
    if (!rev.trim()) return toast("error", "请选择或填写要部署的版本");
    if (!serverId) return toast("error", "请选择部署服务器");
    if (!targetDir.trim()) return toast("error", "请填写部署目录");
    if (resolveError) return toast("error", "版本解析失败，请检查版本号");

    setSubmitting(true);
    try {
      await startDeploy({
        repoId,
        rev: rev.trim(),
        serverId,
        targetDir: targetDir.trim(),
        runScripts,
        scriptDir: scriptDir.trim() || settings.scriptDir,
        script: script.trim() || null,
      });
    } catch {
      // store 已提示错误
    } finally {
      setSubmitting(false);
    }
  }

  const running = live?.status === "running";

  return (
    <Page
      title="部署"
      subtitle="打包当前版本并上传到服务器执行部署脚本"
      actions={
        <Button
          variant="secondary"
          icon={<History className="size-4" />}
          onClick={() => navigate("/history")}
        >
          部署记录
        </Button>
      }
    >
      <div className="grid grid-cols-1 gap-5 xl:grid-cols-[400px_minmax(0,1fr)]">
        <div className="flex flex-col gap-5">
          <Card className="p-5">
            <h2 className="mb-4 flex items-center gap-2.5 text-sm font-semibold tracking-tight text-ink">
              <span className="grid size-7 place-items-center rounded-md border border-brand-line bg-brand-soft">
                <Rocket className="size-3.5 text-brand" />
              </span>
              部署配置
            </h2>

            <div className="flex flex-col gap-4">
              <Field label="仓库" required>
                <Select
                  value={repoId}
                  onChange={(event) => setRepoId(event.target.value)}
                  disabled={running}
                >
                  <option value="">请选择仓库</option>
                  {repos.map((repo) => (
                    <option key={repo.id} value={repo.id}>
                      {repo.name}
                    </option>
                  ))}
                </Select>
              </Field>

              <Field
                label="版本"
                required
                hint={resolving ? "解析中 ..." : resolved?.short}
              >
                {customRev ? (
                  <div className="flex gap-2">
                    <Input
                      value={rev}
                      onChange={(event) => setRev(event.target.value)}
                      placeholder="分支名 / 提交号 / 标签"
                      disabled={running}
                    />
                    <Button
                      variant="secondary"
                      onClick={() => {
                        setCustomRev(false);
                        setRev(selectedRepo?.currentBranch ?? "");
                      }}
                    >
                      选择分支
                    </Button>
                  </div>
                ) : (
                  <div className="flex gap-2">
                    <Select
                      value={rev}
                      onChange={(event) => setRev(event.target.value)}
                      disabled={running}
                    >
                      <option value="">请选择分支</option>
                      {branches.map((branch) => (
                        <option key={branch.name} value={branch.name}>
                          {branch.name}
                          {branch.isCurrent ? "（当前）" : ""}
                        </option>
                      ))}
                    </Select>
                    <Button variant="secondary" onClick={() => setCustomRev(true)}>
                      指定提交
                    </Button>
                  </div>
                )}
                {resolved && !resolveError && (
                  <p className="mt-2 flex items-start gap-1.5 text-[11px] text-ink-dim">
                    <CircleDot className="mt-0.5 size-3 shrink-0 text-pos" />
                    <span className="min-w-0">
                      <span className="text-ink-dim">{resolved.short}</span> {resolved.subject}
                      <span className="text-ink-faint">
                        {" "}
                        · {resolved.author} · {resolved.date}
                      </span>
                    </span>
                  </p>
                )}
                {resolveError && (
                  <p className="mt-2 flex items-start gap-1.5 text-[11px] text-neg">
                    <AlertTriangle className="mt-0.5 size-3 shrink-0" />
                    {resolveError}
                  </p>
                )}
              </Field>

              <Field label="服务器" required>
                <Select
                  value={serverId}
                  onChange={(event) => {
                    const next = event.target.value;
                    setServerId(next);
                    const server = servers.find((item) => item.id === next);
                    if (server && !targetDir.trim()) setTargetDir(server.defaultTargetDir);
                  }}
                  disabled={running}
                >
                  <option value="">请选择服务器</option>
                  {servers.map((server) => (
                    <option key={server.id} value={server.id}>
                      {server.name} ({server.username}@{server.host})
                    </option>
                  ))}
                </Select>
                {servers.length === 0 && (
                  <p className="mt-2 text-[11px] text-ink-faint">
                    还没有服务器，去
                    <button
                      type="button"
                      className="mx-0.5 text-brand hover:underline"
                      onClick={() => navigate("/servers")}
                    >
                      服务器页面
                    </button>
                    添加一台
                  </p>
                )}
              </Field>

              <Field label="部署目录" required hint="服务器上的绝对路径">
                <Input
                  value={targetDir}
                  onChange={(event) => setTargetDir(event.target.value)}
                  placeholder="/opt/apps/my-app"
                  disabled={running}
                />
              </Field>

              <div className="rounded-md border border-line bg-field p-3.5">
                <label className="flex cursor-pointer items-center gap-2.5 text-xs text-ink">
                  <input
                    type="checkbox"
                    checked={runScripts}
                    onChange={(event) => setRunScripts(event.target.checked)}
                    disabled={running}
                    className="size-3.5 accent-primary"
                  />
                  上传解压后执行项目脚本
                </label>
                {runScripts && (
                  <div className="mt-3 grid grid-cols-2 gap-3">
                    <Field label="脚本目录">
                      <Input
                        value={scriptDir}
                        onChange={(event) => setScriptDir(event.target.value)}
                        placeholder="docker"
                        disabled={running}
                      />
                    </Field>
                    <Field label="指定脚本" hint="留空自动执行全部">
                      <Input
                        value={script}
                        onChange={(event) => setScript(event.target.value)}
                        placeholder="deploy.sh"
                        disabled={running}
                      />
                    </Field>
                  </div>
                )}
              </div>

              <Button
                size="lg"
                loading={submitting || running}
                disabled={!!resolveError}
                icon={<Rocket className="size-4" />}
                onClick={() => void handleDeploy()}
              >
                {running ? "部署进行中 ..." : "开始部署"}
              </Button>

              {selectedRepo && (
                <p className="truncate text-center text-[11px] text-ink-faint" title={selectedRepo.path}>
                  {selectedServer ? `${selectedServer.name} → ` : ""}
                  {targetDir || "未填写部署目录"}
                </p>
              )}
            </div>
          </Card>

          {recentDeploys.length > 0 && (
            <Card className="overflow-hidden">
              <div className="border-b border-line px-5 py-3">
                <h2 className="text-xs font-semibold text-ink">该仓库最近部署</h2>
              </div>
              <ul className="divide-y divide-line">
                {recentDeploys.map((record) => (
                  <RecentDeployRow
                    key={record.id}
                    record={record}
                    disabled={running}
                    onRedeploy={() => {
                      void redeploy(record.id).catch(() => {
                        // store 已提示错误
                      });
                    }}
                  />
                ))}
              </ul>
            </Card>
          )}
        </div>

        <div className="flex min-h-0 flex-col gap-5">
          <Card className="p-5">
            <div className="flex items-start justify-between gap-4">
              <div className="min-w-0">
                <h2 className="flex items-center gap-2.5 text-sm font-semibold tracking-tight text-ink">
                  <span className="grid size-7 place-items-center rounded-md border border-brand-line bg-brand-soft">
                    <Terminal className="size-3.5 text-brand" />
                  </span>
                  部署状态
                </h2>
                <p className="mt-1 text-xs text-ink-dim">
                  {live
                    ? live.status === "running"
                      ? "正在部署，日志实时输出中 ..."
                      : live.status === "success"
                        ? "上次部署成功"
                        : "上次部署失败"
                    : "尚未发起部署"}
                </p>
              </div>
              {live && (
                <Badge className={cn("shrink-0 border", deployStatusClass(live.status))}>
                  {live.status === "running" && (
                    <span className="size-1.5 animate-pulse rounded-full bg-current" />
                  )}
                  {deployStatusLabel(live.status)}
                </Badge>
              )}
            </div>

            {running && (
              <div className="mt-4">
                <div className="h-1.5 overflow-hidden rounded-full bg-hover">
                  <div
                    className="h-full rounded-full bg-linear-to-r from-[var(--btn-primary-top)] to-[var(--btn-primary-bottom)] transition-all duration-300"
                    style={{ width: `${Math.max(live?.progress ?? 0, 3)}%` }}
                  />
                </div>
                <p className="mt-2 text-right text-[11px] text-ink-dim">{live?.progress ?? 0}%</p>
              </div>
            )}

            {live?.record && (
              <div
                className={cn(
                  "mt-4 flex flex-wrap items-center gap-3 rounded-md border px-4 py-3 text-xs",
                  live.record.status === "success"
                    ? "border-pos/30 bg-pos-soft text-pos"
                    : "border-neg/30 bg-neg-soft text-neg",
                )}
              >
                {live.record.status === "success" ? (
                  <CheckCircle2 className="size-4 shrink-0" />
                ) : (
                  <XCircle className="size-4 shrink-0" />
                )}
                <span className="min-w-0 flex-1 truncate">
                  {live.record.repoName} · {live.record.commitShort} · {live.record.serverName} ·{" "}
                  {formatDuration(live.record.durationMs)}
                </span>
                <Button
                  size="sm"
                  variant="secondary"
                  onClick={() => navigate(`/history?record=${live.record?.id ?? ""}`)}
                >
                  查看记录
                </Button>
              </div>
            )}

            {selectedRepo?.remote && (
              <p className="mt-4 flex items-center gap-1.5 text-[11px] text-ink-faint">
                <GitBranch className="size-3" />
                {shortPath(selectedRepo.remote, 72)}
              </p>
            )}
          </Card>

          <LogConsole
            lines={live?.lines ?? []}
            className="h-[460px] xl:h-[520px]"
            emptyText="点击「开始部署」后，这里会实时显示打包、上传与脚本执行日志。"
          />
        </div>
      </div>
    </Page>
  );
}

function RecentDeployRow({
  record,
  disabled,
  onRedeploy,
}: {
  record: DeployRecord;
  disabled: boolean;
  onRedeploy: () => void;
}) {
  return (
    <li className="flex items-center gap-3 px-4 py-2.5">
      <span
        className={cn(
          "size-1.5 shrink-0 rounded-full",
          record.status === "success"
            ? "bg-pos"
            : record.status === "failed"
              ? "bg-neg"
              : "bg-warn",
        )}
      />
      <div className="min-w-0 flex-1">
        <p className="truncate text-[11px] text-ink">
          {record.branch} · {record.commitShort}
        </p>
        <p className="mt-0.5 flex items-center gap-1 text-[10px] text-ink-faint">
          <Timer className="size-2.5" />
          {record.startedAt} · {record.serverName}
        </p>
      </div>
      <Button
        size="sm"
        variant="ghost"
        disabled={disabled || record.status === "running"}
        onClick={onRedeploy}
      >
        重新部署
      </Button>
    </li>
  );
}
