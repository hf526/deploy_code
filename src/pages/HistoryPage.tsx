import { useEffect, useMemo, useState, type ReactNode } from "react";
import { Eraser, History, RefreshCw, RotateCcw, ScrollText, Trash2 } from "lucide-react";
import { useNavigate, useSearchParams } from "react-router-dom";

import {
  Badge,
  Button,
  Card,
  ConfirmModal,
  EmptyState,
  Modal,
  Page,
  Select,
} from "../components/ui";
import { api } from "../lib/api";
import { useApp } from "../lib/store";
import type { DeployRecord } from "../lib/types";
import {
  cn,
  deployStatusClass,
  deployStatusLabel,
  formatDuration,
  shortPath,
} from "../lib/utils";

export default function HistoryPage() {
  const history = useApp((state) => state.history);
  const repos = useApp((state) => state.repos);
  const refreshHistory = useApp((state) => state.refreshHistory);
  const refreshRepos = useApp((state) => state.refreshRepos);
  const redeploy = useApp((state) => state.redeploy);
  const toast = useApp((state) => state.toast);
  const navigate = useNavigate();
  const [searchParams] = useSearchParams();

  const [repoFilter, setRepoFilter] = useState("");
  const [viewing, setViewing] = useState<DeployRecord | null>(null);
  const [removing, setRemoving] = useState<DeployRecord | null>(null);
  const [rollingBack, setRollingBack] = useState<DeployRecord | null>(null);
  const [clearing, setClearing] = useState(false);
  const [busy, setBusy] = useState(false);

  const live = useApp((state) => state.live);
  const running = live?.status === "running";

  // 过滤放在前端做，避免筛选结果与后台刷新（部署完成）互相覆盖。
  const filtered = useMemo(
    () => (repoFilter ? history.filter((record) => record.repoId === repoFilter) : history),
    [history, repoFilter],
  );

  useEffect(() => {
    void refreshHistory();
  }, [refreshHistory]);

  useEffect(() => {
    const recordId = searchParams.get("record");
    if (!recordId) return;
    let cancelled = false;
    void api
      .getRecord(recordId)
      .then((record) => {
        if (!cancelled) setViewing(record);
      })
      .catch((error) => {
        if (!cancelled) toast("error", String(error));
      });
    return () => {
      cancelled = true;
    };
  }, [searchParams, toast]);

  async function handleRedeploy(record: DeployRecord) {
    try {
      await redeploy(record.id);
      navigate("/deploy");
    } catch {
      // store 已提示
    }
  }

  async function handleRollback() {
    if (!rollingBack) return;
    setBusy(true);
    try {
      const message = await api.resetHard(rollingBack.repoId, rollingBack.commit);
      toast("success", message.trim() || "已回退");
      setRollingBack(null);
      await refreshRepos();
    } catch (error) {
      toast("error", String(error));
    } finally {
      setBusy(false);
    }
  }

  async function handleDelete() {
    if (!removing) return;
    setBusy(true);
    try {
      await api.deleteRecord(removing.id);
      toast("success", "记录已删除");
      setRemoving(null);
      await refreshHistory();
    } catch (error) {
      toast("error", String(error));
    } finally {
      setBusy(false);
    }
  }

  async function handleClear() {
    setBusy(true);
    try {
      await api.clearHistory();
      toast("success", "已清空部署记录");
      setClearing(false);
      await refreshHistory();
    } catch (error) {
      toast("error", String(error));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Page
      title="部署记录"
      subtitle="每次部署的版本、服务器与执行结果"
      actions={
        <>
          <Select
            value={repoFilter}
            onChange={(event) => setRepoFilter(event.target.value)}
            className="w-48"
          >
            <option value="">全部仓库</option>
            {repos.map((repo) => (
              <option key={repo.id} value={repo.id}>
                {repo.name}
              </option>
            ))}
          </Select>
          <Button
            variant="secondary"
            icon={<RefreshCw className="size-4" />}
            onClick={() => void refreshHistory()}
          >
            刷新
          </Button>
          <Button
            variant="secondary"
            icon={<Eraser className="size-4" />}
            disabled={history.length === 0}
            onClick={() => setClearing(true)}
          >
            清空
          </Button>
        </>
      }
    >
      {filtered.length === 0 ? (
        <EmptyState
          icon={<History className="size-5" />}
          title="暂无部署记录"
          description="在「部署」页面发起一次部署后，这里会记录版本、服务器与完整日志，可随时重新部署。"
          action={
            <Button onClick={() => navigate("/deploy")} icon={<History className="size-4" />}>
              去部署
            </Button>
          }
        />
      ) : (
        <Card className="overflow-hidden">
          <table className="w-full text-left text-xs">
            <thead>
              <tr className="border-b border-line bg-field/60 text-[11px] uppercase tracking-wide text-ink-faint">
                <th className="px-4 py-2.5 font-medium">状态</th>
                <th className="px-4 py-2.5 font-medium">时间</th>
                <th className="px-4 py-2.5 font-medium">仓库</th>
                <th className="px-4 py-2.5 font-medium">版本</th>
                <th className="px-4 py-2.5 font-medium">服务器 / 目录</th>
                <th className="px-4 py-2.5 font-medium">耗时</th>
                <th className="px-4 py-2.5 text-right font-medium">操作</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-line">
              {filtered.map((record) => (
                <tr key={record.id} className="group transition-colors hover:bg-field">
                  <td className="px-4 py-2.5">
                    <Badge className={cn("border", deployStatusClass(record.status))}>
                      {deployStatusLabel(record.status)}
                    </Badge>
                  </td>
                  <td className="whitespace-nowrap px-4 py-2.5 text-ink-dim tabular-nums">
                    {record.startedAt}
                  </td>
                  <td className="px-4 py-2.5 text-ink">{record.repoName}</td>
                  <td className="px-4 py-2.5">
                    <div className="text-ink">{record.branch}</div>
                    <div className="mt-0.5 font-mono text-[10px] text-ink-faint">
                      {record.commitShort} · {shortPath(record.commitSubject, 28)}
                    </div>
                  </td>
                  <td className="px-4 py-2.5">
                    <div className="text-ink">{record.serverName}</div>
                    <div className="mt-0.5 max-w-56 truncate font-mono text-[10px] text-ink-faint" title={record.targetDir}>
                      {record.targetDir}
                    </div>
                  </td>
                  <td className="whitespace-nowrap px-4 py-2.5 text-ink-dim tabular-nums">
                    {record.status === "running" ? "-" : formatDuration(record.durationMs)}
                  </td>
                  <td className="px-4 py-2.5">
                    <div className="flex items-center justify-end gap-1">
                      {record.error && (
                        <span
                          className="mr-1 max-w-40 truncate text-[10px] text-neg"
                          title={record.error}
                        >
                          {record.error}
                        </span>
                      )}
                      <Button
                        size="sm"
                        variant="ghost"
                        icon={<ScrollText className="size-3.5" />}
                        onClick={() => setViewing(record)}
                      >
                        日志
                      </Button>
                      <Button
                        size="sm"
                        variant="ghost"
                        icon={<RefreshCw className="size-3.5" />}
                        disabled={record.status === "running" || running}
                        onClick={() => void handleRedeploy(record)}
                      >
                        重新部署
                      </Button>
                      {record.status === "success" && (
                        <Button
                          size="sm"
                          variant="ghost"
                          icon={<RotateCcw className="size-3.5" />}
                          title="将当前分支回退到此部署版本 (git reset --hard)"
                          onClick={() => setRollingBack(record)}
                        >
                          回滚
                        </Button>
                      )}
                      <Button
                        size="sm"
                        variant="ghost"
                        className="text-neg hover:bg-neg-soft hover:text-neg"
                        icon={<Trash2 className="size-3.5" />}
                        onClick={() => setRemoving(record)}
                      />
                    </div>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </Card>
      )}

      <Modal
        open={!!viewing}
        onClose={() => setViewing(null)}
        title="部署日志"
        subtitle={
          viewing
            ? `${viewing.repoName} · ${viewing.branch} · ${viewing.commitShort}`
            : undefined
        }
        width="max-w-4xl"
        footer={
          <>
            <Button variant="secondary" onClick={() => setViewing(null)}>
              关闭
            </Button>
            {viewing && (
              <Button
                icon={<RefreshCw className="size-4" />}
                disabled={running || viewing.status === "running"}
                onClick={() => {
                  const record = viewing;
                  setViewing(null);
                  void handleRedeploy(record);
                }}
              >
                重新部署此版本
              </Button>
            )}
          </>
        }
      >
        {viewing && (
          <div className="flex flex-col gap-4">
            <div className="grid grid-cols-2 gap-3 text-xs md:grid-cols-4">
              <InfoCell label="状态">
                <Badge className={cn("border", deployStatusClass(viewing.status))}>
                  {deployStatusLabel(viewing.status)}
                </Badge>
              </InfoCell>
              <InfoCell label="服务器">{viewing.serverName}</InfoCell>
              <InfoCell label="目录">{viewing.targetDir}</InfoCell>
              <InfoCell label="耗时">
                {viewing.status === "running" ? "-" : formatDuration(viewing.durationMs)}
              </InfoCell>
            </div>
            {viewing.error && (
              <div className="rounded-md border border-neg/30 bg-neg-soft px-4 py-2.5 text-xs text-neg">
                {viewing.error}
              </div>
            )}
            <pre className="max-h-[46vh] overflow-y-auto rounded-md border border-line bg-sunken px-4 py-2.5 font-mono text-[11.5px] leading-[1.7] text-ink-dim">
              {viewing.log || "（没有日志）"}
            </pre>
          </div>
        )}
      </Modal>

      <ConfirmModal
        open={!!removing}
        danger
        loading={busy}
        title="删除部署记录"
        confirmText="删除"
        description={
          <span>
            确定删除 <b className="text-ink">{removing?.repoName}</b> 在{" "}
            {removing?.startedAt} 的部署记录吗？
          </span>
        }
        onCancel={() => setRemoving(null)}
        onConfirm={() => void handleDelete()}
      />

      <ConfirmModal
        open={!!rollingBack}
        danger
        loading={busy}
        title="回滚到该部署版本"
        confirmText="回滚"
        description={
          <span>
            将在本地仓库 <b className="text-ink">{rollingBack?.repoName}</b> 执行{" "}
            <code className="text-neg">git reset --hard {rollingBack?.commitShort}</code>
            ，把当前分支回退到 {rollingBack?.startedAt} 部署的版本，
            <b className="text-ink">未提交的改动与之后的提交将从当前分支移走</b>。确认继续吗？
          </span>
        }
        onCancel={() => setRollingBack(null)}
        onConfirm={() => void handleRollback()}
      />

      <ConfirmModal
        open={clearing}
        danger
        loading={busy}
        title="清空部署记录"
        confirmText="清空全部"
        description="所有部署记录与日志都会被删除，且无法恢复。确定继续吗？"
        onCancel={() => setClearing(false)}
        onConfirm={() => void handleClear()}
      />
    </Page>
  );
}

function InfoCell({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="rounded-md border border-line bg-field px-3 py-2">
      <p className="text-[10px] uppercase tracking-wide text-ink-faint">{label}</p>
      <div className="mt-1 truncate text-ink">{children}</div>
    </div>
  );
}
