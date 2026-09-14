import { useState } from "react";
import {
  ChevronRight,
  Download,
  FolderOpen,
  GitBranch,
  Link2,
  Pencil,
  Rocket,
  Server,
  Terminal,
  Trash2,
} from "lucide-react";
import { Trans, useTranslation } from "react-i18next";
import { useNavigate } from "react-router-dom";
import { open } from "@tauri-apps/plugin-dialog";

import { BindRemoteModal } from "../components/BindRemoteModal";
import { Badge, Button, ConfirmModal, Field, Input, Modal, Page } from "../components/ui";
import { api } from "../lib/api";
import { openRepoFolder } from "../lib/openRepo";
import { useApp } from "../lib/store";
import type { RepoInfo } from "../lib/types";
import { shortPath } from "../lib/utils";

const FEATURES = [
  { icon: GitBranch, titleKey: "repos.featureBranchTitle", descKey: "repos.featureBranchDesc" },
  { icon: Rocket, titleKey: "repos.featureDeployTitle", descKey: "repos.featureDeployDesc" },
  { icon: Terminal, titleKey: "repos.featureLogTitle", descKey: "repos.featureLogDesc" },
];

export default function ReposPage() {
  const { t } = useTranslation();
  const repos = useApp((state) => state.repos);
  const servers = useApp((state) => state.servers);
  const refreshRepos = useApp((state) => state.refreshRepos);
  const toast = useApp((state) => state.toast);
  const navigate = useNavigate();

  const [opening, setOpening] = useState(false);
  const [renaming, setRenaming] = useState<RepoInfo | null>(null);
  const [renameValue, setRenameValue] = useState("");
  const [removing, setRemoving] = useState<RepoInfo | null>(null);
  const [remoteRepo, setRemoteRepo] = useState<RepoInfo | null>(null);
  const [cloneOpen, setCloneOpen] = useState(false);
  const [cloneUrl, setCloneUrl] = useState("");
  const [cloneDir, setCloneDir] = useState("");
  const [cloning, setCloning] = useState(false);
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

  async function pickCloneDir() {
    try {
      const selected = await open({
        directory: true,
        multiple: false,
        title: t("repos.pickCloneDirTitle"),
      });
      if (typeof selected === "string") setCloneDir(selected);
    } catch (error) {
      toast("error", String(error));
    }
  }

  async function handleClone() {
    if (cloning) return;
    const url = cloneUrl.trim();
    if (!url) {
      toast("error", t("remoteModal.urlRequired"));
      return;
    }
    if (!cloneDir.trim()) {
      toast("error", t("repos.errorCloneDir"));
      return;
    }
    setCloning(true);
    try {
      const repo = await api.cloneRepo({ url, parentDir: cloneDir.trim() });
      toast("success", t("repos.cloned", { name: repo.name }));
      setCloneOpen(false);
      setCloneUrl("");
      setCloneDir("");
      await refreshRepos();
      navigate(`/repos/${repo.id}`);
    } catch (error) {
      toast("error", String(error));
    } finally {
      setCloning(false);
    }
  }

  async function handleRename() {
    if (!renaming || busy) return;
    const target = renaming;
    const nextName = renameValue.trim();
    if (!nextName) {
      toast("error", t("repos.nameRequired"));
      return;
    }
    setBusy(true);
    try {
      await api.updateRepo({ repoId: target.id, name: nextName });
      setRenaming((current) => (current?.id === target.id ? null : current));
      await refreshRepos();
    } catch (error) {
      toast("error", String(error));
    } finally {
      setBusy(false);
    }
  }

  async function handleRemove() {
    if (!removing || busy) return;
    const target = removing;
    setBusy(true);
    try {
      await api.removeRepo(target.id);
      toast("success", t("repos.removed", { name: target.name }));
      setRemoving((current) => (current?.id === target.id ? null : current));
      await refreshRepos();
    } catch (error) {
      toast("error", String(error));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Page
      title={t("nav.repos")}
      subtitle={t("repos.subtitle")}
      actions={
        <div className="flex items-center gap-2">
          <Button
            variant="secondary"
            icon={<Download className="size-4" />}
            onClick={() => setCloneOpen(true)}
          >
            {t("repos.clone")}
          </Button>
          <Button
            loading={opening}
            icon={<FolderOpen className="size-4" />}
            onClick={() => void handleOpen()}
          >
            {t("nav.openRepo")}
          </Button>
        </div>
      }
    >
      {repos.length === 0 ? (
        <div className="mx-auto mt-8 w-full max-w-2xl">
          <div className="ui-hero px-8 py-10 text-center sm:px-12">
            <span className="ui-logo mx-auto flex size-12 items-center justify-center rounded-xl text-white">
              <GitBranch className="size-6" strokeWidth={2} />
            </span>
            <h2 className="mt-5 text-xl font-semibold tracking-tight text-ink">
              {t("repos.heroTitle")}
            </h2>
            <p className="mx-auto mt-2 max-w-md text-[13px] leading-relaxed text-ink-dim">
              {t("repos.heroDescription")}
            </p>
            <div className="mt-6 flex items-center justify-center gap-3">
              <Button
                size="lg"
                loading={opening}
                icon={<FolderOpen className="size-4" />}
                onClick={() => void handleOpen()}
              >
                {t("repos.openFolder")}
              </Button>
              <Button
                size="lg"
                variant="secondary"
                icon={<Download className="size-4" />}
                onClick={() => setCloneOpen(true)}
              >
                {t("repos.cloneRemote")}
              </Button>
              <Button
                size="lg"
                variant="secondary"
                icon={<Server className="size-4" />}
                onClick={() => navigate("/servers")}
              >
                {t("repos.configureServers")}
              </Button>
            </div>
            <div className="mt-8 flex flex-wrap items-start justify-center gap-x-8 gap-y-4 border-t border-line pt-6">
              {FEATURES.map((feature) => (
                <div key={feature.titleKey} className="flex items-start gap-2.5 text-left">
                  <span className="grid size-8 shrink-0 place-items-center rounded-lg bg-brand-soft text-brand">
                    <feature.icon className="size-4" />
                  </span>
                  <div>
                    <p className="text-[13px] font-medium text-ink">{t(feature.titleKey)}</p>
                    <p className="mt-0.5 text-[11px] text-ink-faint">{t(feature.descKey)}</p>
                  </div>
                </div>
              ))}
            </div>
          </div>
        </div>
      ) : (
        <>
          <div className="mb-4 flex flex-wrap items-center gap-2">
            <Badge kind="gray">{t("repos.badgeRepos", { count: repos.length })}</Badge>
            <Badge kind="gray">{t("repos.badgeServers", { count: servers.length })}</Badge>
            {totalChanges > 0 && (
              <Badge kind="amber">{t("repos.badgeChanges", { count: totalChanges })}</Badge>
            )}
          </div>
          <div className="grid grid-cols-1 gap-3.5 xl:grid-cols-2">
            {repos.map((repo) => (
              <div
                key={repo.id}
                role="button"
                tabIndex={0}
                onClick={() => navigate(`/repos/${repo.id}`)}
                onKeyDown={(event) => {
                  // 卡片内按钮的回车会冒泡到这里：只处理焦点在卡片自身的情况。
                  if (event.key === "Enter" && event.target === event.currentTarget) {
                    navigate(`/repos/${repo.id}`);
                  }
                }}
                className="ui-card group flex cursor-pointer items-center gap-4 px-5 py-4 transition-[border-color,box-shadow,transform] duration-150 hover:-translate-y-0.5 hover:border-brand-line hover:shadow-pop"
              >
                <span
                  className={
                    !repo.pathExists
                      ? "grid size-10 shrink-0 place-items-center rounded-lg border border-neg/35 bg-neg-soft"
                      : repo.isRepo
                        ? "grid size-10 shrink-0 place-items-center rounded-lg border border-brand-line bg-brand-soft"
                        : "grid size-10 shrink-0 place-items-center rounded-lg border border-line bg-hover"
                  }
                >
                  <GitBranch
                    className={
                      !repo.pathExists
                        ? "size-5 text-neg"
                        : repo.isRepo
                          ? "size-5 text-brand"
                          : "size-5 text-ink-dim"
                    }
                    strokeWidth={1.75}
                  />
                </span>
                <div className="min-w-0 flex-1">
                  <div className="flex items-center gap-2">
                    <h3 className="truncate text-[14px] font-semibold tracking-tight text-ink">
                      {repo.name}
                    </h3>
                    {!repo.pathExists ? (
                      <Badge kind="red">{t("repos.pathUnavailable")}</Badge>
                    ) : repo.isRepo ? (
                      <Badge kind="brand">{repo.currentBranch}</Badge>
                    ) : (
                      <Badge kind="gray">{t("repos.notRepo")}</Badge>
                    )}
                    {repo.changeCount > 0 && (
                      <Badge kind="amber">{t("repos.changeCount", { count: repo.changeCount })}</Badge>
                    )}
                  </div>
                  <p
                    className="mt-1.5 truncate font-mono text-[11px] text-ink-faint"
                    title={repo.path}
                  >
                    {shortPath(repo.path, 64)}
                  </p>
                  <p className="mt-1 flex min-w-0 items-center gap-1.5 text-[11px]">
                    {repo.remote ? (
                      <span className="truncate font-mono text-ink-dim" title={repo.remote}>
                        {repo.remote}
                      </span>
                    ) : (
                      <span className="text-ink-faint">
                        {!repo.pathExists
                          ? t("repos.pathUnavailable")
                          : repo.isRepo
                            ? t("repos.noRemote")
                            : t("repos.notRepo")}
                      </span>
                    )}
                  </p>
                </div>
                <div
                  className="flex shrink-0 items-center gap-1"
                  onClick={(event) => event.stopPropagation()}
                >
                  <Button
                    size="sm"
                    variant="ghost"
                    title={t("repos.revealInFileManager")}
                    onClick={() =>
                      void api.revealPath(repo.path).catch((e) => toast("error", String(e)))
                    }
                    icon={<FolderOpen className="size-3.5" />}
                  />
                  {repo.pathExists && (
                    <Button
                      size="sm"
                      variant="ghost"
                      title={repo.remote ? t("remoteModal.editTitle") : t("remoteModal.bindTitle")}
                      onClick={() => setRemoteRepo(repo)}
                      icon={<Link2 className="size-3.5" />}
                    />
                  )}
                  <Button
                    size="sm"
                    variant="ghost"
                    title={t("repos.rename")}
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
                    title={t("repos.remove")}
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
        onClose={() => {
          if (!busy) setRenaming(null);
        }}
        title={t("repos.renameTitle")}
        subtitle={renaming?.path}
        width="max-w-md"
        footer={
          <>
            <Button variant="secondary" disabled={busy} onClick={() => setRenaming(null)}>
              {t("common.cancel")}
            </Button>
            <Button loading={busy} onClick={() => void handleRename()}>
              {t("common.save")}
            </Button>
          </>
        }
      >
        <Input
          autoFocus
          value={renameValue}
          onChange={(event) => setRenameValue(event.target.value)}
          placeholder={t("repos.renamePlaceholder")}
          onKeyDown={(event) => {
            if (event.key === "Enter") void handleRename();
          }}
        />
      </Modal>

      <ConfirmModal
        open={!!removing}
        danger
        loading={busy}
        title={t("repos.removeTitle")}
        confirmText={t("repos.removeConfirmText")}
        description={
          <Trans
            i18nKey="repos.removeDescription"
            values={{ name: removing?.name }}
            components={{ b: <b className="text-ink" />, br: <br /> }}
          />
        }
        onCancel={() => setRemoving(null)}
        onConfirm={() => void handleRemove()}
      />

      <BindRemoteModal
        repo={remoteRepo}
        onClose={() => setRemoteRepo(null)}
        onSaved={() => void refreshRepos()}
      />

      <Modal
        open={cloneOpen}
        onClose={() => {
          if (!cloning) setCloneOpen(false);
        }}
        title={t("repos.cloneTitle")}
        subtitle={t("repos.cloneSubtitle")}
        width="max-w-lg"
        footer={
          <>
            <Button variant="secondary" disabled={cloning} onClick={() => setCloneOpen(false)}>
              {t("common.cancel")}
            </Button>
            <Button loading={cloning} onClick={() => void handleClone()}>
              {t("repos.cloneConfirmText")}
            </Button>
          </>
        }
      >
        <div className="flex flex-col gap-4">
          <Field label={t("repos.gitUrl")} required hint={t("repos.gitUrlHint")}>
            <Input
              autoFocus
              value={cloneUrl}
              onChange={(event) => setCloneUrl(event.target.value)}
              placeholder="git@github.com:user/repo.git"
            />
          </Field>
          <Field label={t("repos.cloneTo")} required hint={t("repos.cloneToHint")}>
            <div className="flex items-center gap-2">
              <Input
                value={cloneDir}
                onChange={(event) => setCloneDir(event.target.value)}
                placeholder={t("repos.cloneDirPlaceholder")}
              />
              <Button variant="secondary" onClick={() => void pickCloneDir()}>
                {t("repos.select")}
              </Button>
            </div>
          </Field>
          <p className="text-[11px] leading-relaxed text-ink-faint">{t("repos.cloneNote")}</p>
        </div>
      </Modal>
    </Page>
  );
}
