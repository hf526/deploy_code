import { useEffect, useMemo, useState } from "react";
import { ChevronDown, ChevronRight, Cloud, Play, RefreshCw, Save, ShieldCheck, Trash2 } from "lucide-react";

import { LogConsole } from "../components/LogConsole";
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
import { useApp } from "../lib/store";
import type { Branch, PagesConfig, PagesDeployRecord } from "../lib/types";

const EMPTY_CONFIG: PagesConfig = {
  projectName: "",
  buildCommand: "",
  outputDir: "dist",
  branch: "main",
};

function statusBadge(record: PagesDeployRecord) {
  if (record.status === "success") return <Badge kind="green">成功</Badge>;
  if (record.status === "failed") return <Badge kind="red">失败</Badge>;
  return <Badge kind="amber">进行中</Badge>;
}

export default function PagesPage() {
  const repos = useApp((state) => state.repos);
  const settings = useApp((state) => state.settings);
  const records = useApp((state) => state.pagesRecords);
  const livePages = useApp((state) => state.livePages);
  const startPagesDeploy = useApp((state) => state.startPagesDeploy);
  const refreshPagesRecords = useApp((state) => state.refreshPagesRecords);
  const clearLivePages = useApp((state) => state.clearLivePages);
  const toast = useApp((state) => state.toast);

  const [repoId, setRepoId] = useState("");
  const [draft, setDraft] = useState<PagesConfig>(EMPTY_CONFIG);
  const [skipBuild, setSkipBuild] = useState(false);
  const [saving, setSaving] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [loaded, setLoaded] = useState(false);
  const [expanded, setExpanded] = useState<string | null>(null);
  const [confirmClear, setConfirmClear] = useState(false);
  const [branches, setBranches] = useState<Branch[]>([]);

  const selected = useMemo(
    () => repos.find((repo) => repo.id === repoId) ?? null,
    [repos, repoId],
  );

  useEffect(() => {
    if (!repoId && repos.length > 0) {
      setRepoId(repos[0].id);
    }
  }, [repos, repoId]);

  useEffect(() => {
    if (!selected) {
      setDraft(EMPTY_CONFIG);
      setLoaded(false);
      return;
    }
    let cancelled = false;
    setLoaded(false);
    void api
      .getPagesConfig(selected.id)
      .then((config) => {
        if (!cancelled) {
          setDraft(config);
          setLoaded(true);
        }
      })
      .catch((error) => {
        // 加载失败时保留当前草稿，避免后续保存把已保存配置覆盖成空白。
        if (!cancelled) toast("error", String(error));
      });
    return () => {
      cancelled = true;
    };
  }, [selected?.id, toast]);

  useEffect(() => {
    if (!selected) {
      setBranches([]);
      return;
    }
    let cancelled = false;
    void api
      .listBranches(selected.id, false)
      .then((list) => {
        if (!cancelled) setBranches(list);
      })
      .catch(() => {
        if (!cancelled) setBranches([]);
      });
    return () => {
      cancelled = true;
    };
  }, [selected?.id]);

  const branchOptions = useMemo(() => {
    const names = branches.map((branch) => branch.name);
    if (draft.branch && !names.includes(draft.branch)) {
      names.unshift(draft.branch);
    }
    return names;
  }, [branches, draft.branch]);

  const running = livePages?.status === "running";
  const tokenReady = settings.cloudflareApiToken.trim().length > 0;
  const accountReady = settings.cloudflareAccountId.trim().length > 0;

  async function handleSave() {
    if (!selected) return;
    setSaving(true);
    try {
      const saved = await api.savePagesConfig(selected.id, draft);
      setDraft(saved);
      toast("success", `已保存 ${selected.name} 的 Pages 配置`);
    } catch (error) {
      toast("error", String(error));
    } finally {
      setSaving(false);
    }
  }

  async function handleTest() {
    if (!selected || submitting) return;
    setSubmitting(true);
    try {
      const message = await api.testPages(selected.id, draft);
      toast("success", message || "环境检查通过");
    } catch (error) {
      toast("error", String(error));
    } finally {
      setSubmitting(false);
    }
  }

  async function handleDeploy() {
    if (!selected || submitting) return;
    if (!draft.projectName.trim()) {
      toast("error", "请先填写 Pages 项目名");
      return;
    }
    setSubmitting(true);
    try {
      // 先保存当前表单，保证实际部署参数与界面一致。
      const saved = await api.savePagesConfig(selected.id, draft);
      setDraft(saved);
    } catch (error) {
      toast("error", String(error));
      setSubmitting(false);
      return;
    }
    clearLivePages();
    try {
      await startPagesDeploy({ repoId: selected.id, skipBuild });
    } catch {
      // store 已提示错误
    } finally {
      setSubmitting(false);
    }
  }

  async function handleDelete(recordId: string) {
    try {
      await api.deletePagesRecord(recordId);
      await refreshPagesRecords();
    } catch (error) {
      toast("error", String(error));
    }
  }

  async function handleClear() {
    try {
      await api.clearPagesRecords();
      await refreshPagesRecords();
      toast("success", "已清空 Pages 部署记录");
    } catch (error) {
      toast("error", String(error));
    } finally {
      setConfirmClear(false);
    }
  }

  async function copyUrl(url: string) {
    try {
      await navigator.clipboard.writeText(url);
      toast("success", "部署地址已复制");
    } catch {
      toast("info", url);
    }
  }

  return (
    <Page
      title="Cloudflare Pages"
      subtitle="本地构建后通过 wrangler 一键部署到 Cloudflare Pages"
      actions={
        <Button
          icon={<RefreshCw className="size-4" />}
          variant="secondary"
          onClick={() => void refreshPagesRecords()}
        >
          刷新记录
        </Button>
      }
    >
      <div className="grid grid-cols-1 gap-6 xl:grid-cols-[400px_minmax(0,1fr)]">
        <div className="flex flex-col gap-6">
          <section>
            <SectionTitle title="项目配置" description="按仓库保存，构建产物目录如 dist / build / out" />
            <Card className="flex flex-col gap-4 p-5">
              <Field label="仓库" required>
                <Select
                  value={repoId}
                  onChange={(event) => setRepoId(event.target.value)}
                  disabled={repos.length === 0}
                >
                  {repos.length === 0 && <option value="">暂无仓库</option>}
                  {repos.map((repo) => (
                    <option key={repo.id} value={repo.id}>
                      {repo.name}
                    </option>
                  ))}
                </Select>
              </Field>

              <Field label="Pages 项目名" required hint="Cloudflare 控制台中的项目名">
                <Input
                  value={draft.projectName}
                  onChange={(event) => setDraft({ ...draft, projectName: event.target.value })}
                  placeholder="my-site"
                />
              </Field>

              <Field label="构建命令" hint="留空则直接上传现有产物">
                <Input
                  value={draft.buildCommand}
                  onChange={(event) => setDraft({ ...draft, buildCommand: event.target.value })}
                  placeholder="npm run build"
                />
              </Field>

              <div className="grid grid-cols-2 gap-4">
                <Field label="输出目录" required>
                  <Input
                    value={draft.outputDir}
                    onChange={(event) => setDraft({ ...draft, outputDir: event.target.value })}
                    placeholder="dist"
                  />
                </Field>
                <Field label="分支" hint="生产分支">
                  <Select
                    value={draft.branch}
                    onChange={(event) => setDraft({ ...draft, branch: event.target.value })}
                    disabled={!selected || !loaded}
                  >
                    {branchOptions.length === 0 && <option value="">请选择分支</option>}
                    {branchOptions.map((name) => {
                      const branch = branches.find((item) => item.name === name);
                      return (
                        <option key={name} value={name}>
                          {name}
                          {branch?.isCurrent ? "（当前）" : ""}
                        </option>
                      );
                    })}
                  </Select>
                </Field>
              </div>

              <div className="flex justify-end gap-2 border-t border-line pt-4">
                <Button
                  variant="secondary"
                  loading={submitting}
                  disabled={!selected || !loaded}
                  onClick={() => void handleTest()}
                >
                  <ShieldCheck className="size-4" />
                  测试环境
                </Button>
                <Button
                  variant="secondary"
                  loading={saving}
                  disabled={!selected || !loaded || submitting}
                  onClick={() => void handleSave()}
                >
                  <Save className="size-4" />
                  保存
                </Button>
              </div>
            </Card>
          </section>

          <section>
            <SectionTitle title="部署" description="构建（可选）+ wrangler 上传" />
            <Card className="flex flex-col gap-4 p-5">
              <Checkbox checked={skipBuild} onChange={setSkipBuild}>
                跳过构建，直接上传输出目录
              </Checkbox>
              {(!tokenReady || !accountReady) && (
                <p className="text-[11px] leading-relaxed text-warn">
                  尚未配置 Cloudflare API Token / Account ID，请到「设置 → Cloudflare Pages」填写。
                </p>
              )}
              <p className="text-[11px] leading-relaxed text-ink-faint">
                Token 与 Account ID 通过环境变量传给 wrangler，不会出现在命令行参数里。
                需要本机安装 Node.js（wrangler 通过 npx 调用）。
              </p>
              <Button
                size="lg"
                loading={running || submitting}
                disabled={!selected || !loaded || !tokenReady || !accountReady}
                onClick={() => void handleDeploy()}
              >
                <Play className="size-4" />
                {running ? "部署进行中 ..." : "构建并部署"}
              </Button>
            </Card>
          </section>
        </div>

        <div className="flex min-w-0 flex-col gap-6">
          <section className="flex min-h-[320px] flex-col">
            <SectionTitle
              title="实时日志"
              description={running ? "部署执行中，请勿关闭应用" : "最近一次部署的实时输出"}
            />
            <LogConsole
              className="min-h-[280px] flex-1"
              title="Pages 日志"
              emptyText="点击「构建并部署」后这里会显示实时日志 ..."
              lines={livePages?.lines ?? []}
            />
          </section>

          <section>
            <SectionTitle
              title="部署记录"
              description={`共 ${records.length} 条`}
              actions={
                records.length > 0 ? (
                  <Button variant="ghost" size="sm" onClick={() => setConfirmClear(true)}>
                    清空记录
                  </Button>
                ) : undefined
              }
            />
            {records.length === 0 ? (
              <EmptyState
                icon={<Cloud className="size-4.5" />}
                title="暂无 Pages 部署记录"
                description="配置项目名与输出目录后，点击「构建并部署」即可。"
              />
            ) : (
              <Card className="divide-y divide-line overflow-hidden">
                {records.map((record) => (
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
                            {record.repoName} → {record.projectName}
                            <span className="ml-2 text-[11px] font-normal text-ink-faint">
                              {record.branch}
                              {record.commitShort ? ` · ${record.commitShort}` : ""}
                            </span>
                          </p>
                          <p className="mt-0.5 truncate text-[11px] text-ink-faint">
                            {record.startedAt}
                            {record.url ? ` · ${record.url}` : ""}
                          </p>
                        </div>
                        {statusBadge(record)}
                      </button>
                      {record.url && (
                        <Button variant="ghost" size="sm" onClick={() => void copyUrl(record.url!)}>
                          复制地址
                        </Button>
                      )}
                      <Button
                        variant="ghost"
                        size="sm"
                        title="删除记录"
                        onClick={() => void handleDelete(record.id)}
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
        title="清空 Pages 部署记录"
        confirmText="清空"
        description="将删除全部 Pages 部署记录（不会影响 Cloudflare 上的部署），此操作不可恢复。"
        onCancel={() => setConfirmClear(false)}
        onConfirm={() => void handleClear()}
      />
    </Page>
  );
}
