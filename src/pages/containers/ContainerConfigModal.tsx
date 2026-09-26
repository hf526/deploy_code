import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Save } from "lucide-react";

import { ServerSelect } from "../../components/ServerSelect";
import { Button, Checkbox, Field, Input, Modal, Select } from "../../components/ui";
import { api } from "../../lib/api";
import { useApp } from "../../lib/store";
import type { ComposeStack, ContainerConfig } from "../../lib/types";

/**
 * 容器备份配置的新增 / 编辑弹窗。
 *
 * 项目下拉每次换服务器现扫一次：compose 项目不落本地配置，列表只能来自服务器，
 * 所以这里存的只是「哪台机器上的哪个项目」，扫描结果变了也不会让配置失效。
 */
export function ContainerConfigModal({
  config,
  preset,
  onClose,
  onSaved,
}: {
  config: ContainerConfig | null;
  /** 从扫描区「存为配置」带过来的参数：只预填，仍需确认命名后保存。 */
  preset?: ContainerConfig | null;
  onClose: () => void;
  onSaved: (saved: ContainerConfig) => void;
}) {
  const { t } = useTranslation();
  const servers = useApp((state) => state.servers);
  const toast = useApp((state) => state.toast);

  const seed = config ?? preset ?? null;
  const [name, setName] = useState(config?.name ?? "");
  const [serverId, setServerId] = useState(seed?.serverId ?? servers[0]?.id ?? "");
  const [project, setProject] = useState(seed?.project ?? "");
  const [includeVolumes, setIncludeVolumes] = useState(seed?.includeVolumes ?? true);
  const [includeImages, setIncludeImages] = useState(seed?.includeImages ?? true);
  const [pauseSource, setPauseSource] = useState(seed?.pauseSource ?? false);
  const [targetServerId, setTargetServerId] = useState(seed?.target?.serverId ?? "");
  const [targetDir, setTargetDir] = useState(seed?.target?.targetDir ?? "");
  const [startServices, setStartServices] = useState(seed?.target?.startServices ?? true);

  const [stacks, setStacks] = useState<ComposeStack[]>([]);
  const [loadingStacks, setLoadingStacks] = useState(false);
  const [saving, setSaving] = useState(false);
  // 换服务器时作废在途扫描，否则慢的那次返回会把项目列表盖成另一台机器的。
  const stacksSeq = useRef(0);

  useEffect(() => {
    if (!serverId && servers.length > 0) setServerId(servers[0].id);
  }, [serverId, servers]);

  useEffect(() => {
    const seq = ++stacksSeq.current;
    setStacks([]);
    if (!serverId) return;
    setLoadingStacks(true);
    api
      .listComposeStacks(serverId)
      .then((list) => {
        if (seq === stacksSeq.current) setStacks(list);
      })
      .catch((error) => {
        if (seq === stacksSeq.current) toast("error", String(error));
      })
      .finally(() => {
        if (seq === stacksSeq.current) setLoadingStacks(false);
      });
  }, [serverId, toast]);

  // 已保存的项目可能已经不在这台机器上了：仍留在选项里，别让编辑变成静默改项目。
  const projectOptions = useMemo(() => {
    const names = stacks.map((stack) => stack.name);
    return project && !names.includes(project) ? [project, ...names] : names;
  }, [stacks, project]);

  const targets = useMemo(
    () => servers.filter((server) => server.id !== serverId),
    [servers, serverId],
  );

  useEffect(() => {
    if (targetServerId && !targets.some((server) => server.id === targetServerId)) {
      setTargetServerId("");
    }
  }, [targets, targetServerId]);

  /** 选中来源项目时顺手照抄它的项目目录：compose 项目与路径无关，同名换机即可跑。 */
  useEffect(() => {
    const workingDir = stacks.find((stack) => stack.name === project)?.workingDir ?? "";
    if (workingDir) setTargetDir((current) => (current.trim() ? current : workingDir));
  }, [stacks, project]);

  async function handleSave() {
    if (!serverId) {
      toast("error", t("containers.config.serverRequired"));
      return;
    }
    if (!project) {
      toast("error", t("containers.pickProject"));
      return;
    }
    if (!includeVolumes && !includeImages) {
      toast("error", t("containers.emptyBundle"));
      return;
    }
    const dir = targetDir.trim();
    if (targetServerId && !dir.startsWith("/")) {
      toast("error", t("containers.targetDirRequired"));
      return;
    }
    setSaving(true);
    try {
      // 名称留空交给后端按项目名兜底：从扫描区一键保存时不必先起名字。
      const saved = await api.saveContainerConfig({
        id: config?.id ?? "",
        name: name.trim(),
        serverId,
        project,
        pauseSource,
        includeVolumes,
        includeImages,
        target: targetServerId
          ? { serverId: targetServerId, targetDir: dir, startServices }
          : null,
        createdAt: config?.createdAt ?? "",
      });
      onSaved(saved);
    } catch (error) {
      toast("error", String(error));
    } finally {
      setSaving(false);
    }
  }

  return (
    <Modal
      open
      onClose={saving ? () => undefined : onClose}
      title={config ? t("containers.config.edit") : t("containers.config.new")}
      subtitle={t("containers.config.subtitle")}
      width="max-w-xl"
      footer={
        <>
          <Button variant="secondary" disabled={saving} onClick={onClose}>
            {t("common.cancel")}
          </Button>
          <Button
            icon={<Save className="size-4" />}
            loading={saving}
            disabled={!project || loadingStacks}
            onClick={() => void handleSave()}
          >
            {t("common.save")}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-4">
        <Field label={t("containers.config.name")} hint={t("containers.config.nameHint")}>
          <Input
            value={name}
            onChange={(event) => setName(event.target.value)}
            placeholder={project || t("containers.config.namePlaceholder")}
            disabled={saving}
          />
        </Field>

        <div className="grid grid-cols-1 gap-4 md:grid-cols-2">
          <Field label={t("containers.server")} required>
            <ServerSelect
              value={serverId}
              onChange={(id) => {
                // 换机器必须重选项目：留着上一台机器的那个名字，存下来就是一条指向不存在项目的配置。
                setProject("");
                setServerId(id);
              }}
              servers={servers}
              disabled={saving || loadingStacks}
              emptyText={t("containers.noServers")}
            />
          </Field>
          <Field label={t("containers.config.project")} required>
            <Select
              value={project}
              disabled={saving || !serverId}
              onChange={(event) => setProject(event.target.value)}
            >
              <option value="">
                {loadingStacks ? t("common.loading") : t("containers.pickProject")}
              </option>
              {projectOptions.map((stackName) => (
                <option key={stackName} value={stackName}>
                  {stackName}
                </option>
              ))}
            </Select>
          </Field>
        </div>

        <div className="flex flex-col gap-2.5">
          <Checkbox checked={includeVolumes} disabled={saving} onChange={setIncludeVolumes}>
            {t("containers.includeVolumes")}
          </Checkbox>
          <Checkbox checked={includeImages} disabled={saving} onChange={setIncludeImages}>
            {t("containers.includeImages")}
          </Checkbox>
          <Checkbox checked={pauseSource} disabled={saving} onChange={setPauseSource}>
            {t("containers.pauseSource")}
          </Checkbox>
        </div>

        <div className="flex flex-col gap-4 border-t border-line pt-4">
          <div className="grid grid-cols-1 gap-4 md:grid-cols-2">
            <Field label={t("containers.targetServer")} hint={t("containers.targetServerHint")}>
              <Select
                value={targetServerId}
                disabled={saving || targets.length === 0}
                onChange={(event) => setTargetServerId(event.target.value)}
              >
                <option value="">{t("containers.config.backupOnlyTarget")}</option>
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
                disabled={saving || !targetServerId}
                placeholder="/opt/blog"
                onChange={(event) => setTargetDir(event.target.value)}
              />
            </Field>
          </div>
          <Checkbox
            checked={startServices}
            disabled={saving || !targetServerId}
            onChange={setStartServices}
          >
            {t("containers.startServices")}
          </Checkbox>
          <p className="text-[11px] leading-relaxed text-ink-faint">
            {t("containers.config.scheduleNote")}
          </p>
        </div>
      </div>
    </Modal>
  );
}
