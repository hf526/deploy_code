import { useState } from "react";
import {
  ChevronRight,
  FolderOpen,
  GitBranch,
  Pencil,
  Rocket,
  Server,
  Terminal,
  Trash2,
} from "lucide-react";
import { useNavigate } from "react-router-dom";

import { Badge, Button, ConfirmModal, Input, Modal, Page } from "../components/ui";
import { api } from "../lib/api";
import { openRepoFolder } from "../lib/openRepo";
import { useApp } from "../lib/store";
import type { RepoInfo } from "../lib/types";
import { shortPath } from "../lib/utils";

const FEATURES = [
  { icon: GitBranch, title: "分支管理", desc: "切换、创建、对比差异" },
  { icon: Rocket, title: "一键部署", desc: "打包上传并执行脚本" },
  { icon: Terminal, title: "实时日志", desc: "部署过程全程可见" },
];

export default function ReposPage() {
  const repos = useApp((state) => state.repos);
  const servers = useApp((state) => state.servers);
  const refreshRepos = useApp((state) => state.refreshRepos);
  const toast = useApp((state) => state.toast);
  const navigate = useNavigate();

  const [opening, setOpening] = useState(false);
  const [renaming, setRenaming] = useState<RepoInfo | null>(null);
  const [renameValue, setRenameValue] = useState("");
  const [removing, setRemoving] = useState<RepoInfo | null>(null);
  const [busy, setBusy] = useState(false);

  const totalChanges = repos.reduce((sum, repo) => sum + repo.changeCount, 0);

  async function handleOpen() {
    setOpening(true);
    try {
      await openRepoFolder(navigate);
    } finally {
      setOpening(false);
    }
  }

  async function handleRename() {
    if (!renaming) return;
    const nextName = renameValue.trim();
    if (!nextName) {
      toast("error", "仓库名称不能为空");
      return;
    }
    setBusy(true);
    try {
      await api.updateRepo({ repoId: renaming.id, name: nextName });
      setRenaming(null);
      await refreshRepos();
    } catch (error) {
      toast("error", String(error));
    } finally {
      setBusy(false);
    }
  }

  async function handleRemove() {
    if (!removing) return;
    setBusy(true);
    try {
      await api.removeRepo(removing.id);
      toast("success", `已移除仓库 ${removing.name}`);
      setRemoving(null);
      await refreshRepos();
    } catch (error) {
      toast("error", String(error));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Page
      title="仓库"
      subtitle="本地 Git 仓库的工作台：分支、差异与部署"
      actions={
        <Button
          loading={opening}
          icon={<FolderOpen className="size-4" />}
          onClick={() => void handleOpen()}
        >
          打开仓库
        </Button>
      }
    >
      {repos.length === 0 ? (
        <div className="mx-auto mt-8 w-full max-w-2xl">
          <div className="ui-hero px-8 py-10 text-center sm:px-12">
            <span className="ui-logo mx-auto flex size-12 items-center justify-center rounded-xl text-white">
              <GitBranch className="size-6" strokeWidth={2} />
            </span>
            <h2 className="mt-5 text-xl font-semibold tracking-tight text-ink">
              开始你的第一次部署
            </h2>
            <p className="mx-auto mt-2 max-w-md text-[13px] leading-relaxed text-ink-dim">
              选择一个本地 Git 仓库文件夹即可开始，不需要任何初始化配置。
            </p>
            <div className="mt-6 flex items-center justify-center gap-3">
              <Button
                size="lg"
                loading={opening}
                icon={<FolderOpen className="size-4" />}
                onClick={() => void handleOpen()}
              >
                打开仓库文件夹
              </Button>
              <Button
                size="lg"
                variant="secondary"
                icon={<Server className="size-4" />}
                onClick={() => navigate("/servers")}
              >
                配置服务器
              </Button>
            </div>
            <div className="mt-8 flex flex-wrap items-start justify-center gap-x-8 gap-y-4 border-t border-line pt-6">
              {FEATURES.map((feature) => (
                <div key={feature.title} className="flex items-start gap-2.5 text-left">
                  <span className="grid size-8 shrink-0 place-items-center rounded-lg bg-brand-soft text-brand">
                    <feature.icon className="size-4" />
                  </span>
                  <div>
                    <p className="text-[13px] font-medium text-ink">{feature.title}</p>
                    <p className="mt-0.5 text-[11px] text-ink-faint">{feature.desc}</p>
                  </div>
                </div>
              ))}
            </div>
          </div>
        </div>
      ) : (
        <>
          <div className="mb-4 flex flex-wrap items-center gap-2">
            <Badge kind="gray">仓库 {repos.length}</Badge>
            <Badge kind="gray">服务器 {servers.length}</Badge>
            {totalChanges > 0 && <Badge kind="amber">未提交变动 {totalChanges}</Badge>}
          </div>
          <div className="grid grid-cols-1 gap-3.5 xl:grid-cols-2">
            {repos.map((repo) => (
              <div
                key={repo.id}
                role="button"
                tabIndex={0}
                onClick={() => navigate(`/repos/${repo.id}`)}
                onKeyDown={(event) => {
                  if (event.key === "Enter") navigate(`/repos/${repo.id}`);
                }}
                className="ui-card group flex cursor-pointer items-center gap-4 px-5 py-4 transition-[border-color,box-shadow,transform] duration-150 hover:-translate-y-0.5 hover:border-brand-line hover:shadow-pop"
              >
                <span
                  className={
                    repo.isRepo
                      ? "grid size-10 shrink-0 place-items-center rounded-lg border border-brand-line bg-brand-soft"
                      : "grid size-10 shrink-0 place-items-center rounded-lg border border-neg/35 bg-neg-soft"
                  }
                >
                  <GitBranch
                    className={repo.isRepo ? "size-5 text-brand" : "size-5 text-neg"}
                    strokeWidth={1.75}
                  />
                </span>
                <div className="min-w-0 flex-1">
                  <div className="flex items-center gap-2">
                    <h3 className="truncate text-[14px] font-semibold tracking-tight text-ink">
                      {repo.name}
                    </h3>
                    {repo.isRepo ? (
                      <Badge kind="brand">{repo.currentBranch}</Badge>
                    ) : (
                      <Badge kind="red">路径不可用</Badge>
                    )}
                    {repo.changeCount > 0 && (
                      <Badge kind="amber">{repo.changeCount} 个变动</Badge>
                    )}
                  </div>
                  <p
                    className="mt-1.5 truncate font-mono text-[11px] text-ink-faint"
                    title={repo.path}
                  >
                    {shortPath(repo.path, 64)}
                  </p>
                </div>
                <div
                  className="flex shrink-0 items-center gap-1"
                  onClick={(event) => event.stopPropagation()}
                >
                  <Button
                    size="sm"
                    variant="ghost"
                    title="在文件管理器中打开"
                    onClick={() =>
                      void api.revealPath(repo.path).catch((e) => toast("error", String(e)))
                    }
                    icon={<FolderOpen className="size-3.5" />}
                  />
                  <Button
                    size="sm"
                    variant="ghost"
                    title="重命名"
                    className="opacity-0 group-hover:opacity-100"
                    onClick={() => {
                      setRenaming(repo);
                      setRenameValue(repo.name);
                    }}
                    icon={<Pencil className="size-3.5" />}
                  />
                  <Button
                    size="sm"
                    variant="ghost"
                    title="移除"
                    className="text-neg opacity-0 hover:bg-neg-soft hover:text-neg group-hover:opacity-100"
                    onClick={() => setRemoving(repo)}
                    icon={<Trash2 className="size-3.5" />}
                  />
                  <span className="ml-2 grid size-7 place-items-center rounded-full border border-line text-ink-faint transition-colors group-hover:border-brand-line group-hover:text-brand">
                    <ChevronRight className="size-4" />
                  </span>
                </div>
              </div>
            ))}
          </div>
        </>
      )}

      <Modal
        open={!!renaming}
        onClose={() => setRenaming(null)}
        title="重命名仓库"
        subtitle={renaming?.path}
        width="max-w-md"
        footer={
          <>
            <Button variant="secondary" onClick={() => setRenaming(null)}>
              取消
            </Button>
            <Button loading={busy} onClick={() => void handleRename()}>
              保存
            </Button>
          </>
        }
      >
        <Input
          autoFocus
          value={renameValue}
          onChange={(event) => setRenameValue(event.target.value)}
          placeholder="仓库显示名称"
          onKeyDown={(event) => {
            if (event.key === "Enter") void handleRename();
          }}
        />
      </Modal>

      <ConfirmModal
        open={!!removing}
        danger
        loading={busy}
        title="移除仓库"
        confirmText="移除"
        description={
          <span>
            确定要移除仓库 <b className="text-ink">{removing?.name}</b> 吗？
            <br />
            只会从列表中移除，不会删除本地代码和部署记录。
          </span>
        }
        onCancel={() => setRemoving(null)}
        onConfirm={() => void handleRemove()}
      />
    </Page>
  );
}
