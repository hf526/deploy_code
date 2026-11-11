import { useEffect, useState } from "react";
import {
  ChevronDown,
  ChevronRight,
  RefreshCw,
  ShieldAlert,
  Sparkles,
  Terminal,
  Wrench,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import { useNavigate } from "react-router-dom";

import { Badge, Button, Modal } from "../../components/ui";
import { api } from "../../lib/api";
import { useApp } from "../../lib/store";
import type {
  AiDiagnosis,
  AiFixAction,
  DeployRecord,
  DiagnosisPreview,
  DiagnosticCommandId,
  DiagnosticOutput,
} from "../../lib/types";
import { humanSize } from "../../lib/utils";

type Stage = "intro" | "confirm" | "result";

/**
 * 部署失败的 AI 诊断弹窗。
 *
 * 发送前先展示「将发送的内容」（与后端 build_context 共用同一份拼装），用户确认后才发起分析；
 * 结果里的动作只来自后端白名单，凭据类怀疑没有动作。
 */
export function AiDiagnosisModal({
  record,
  onClose,
  onRedeploy,
  onAnalyzed,
}: {
  record: DeployRecord | null;
  onClose: () => void;
  onRedeploy: (record: DeployRecord) => void;
  /** 分析成功后回传，调用方用它更新自己那份记录快照，避免下次再点又重复分析。 */
  onAnalyzed?: (diagnosis: AiDiagnosis) => void;
}) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const toast = useApp((state) => state.toast);

  const [stage, setStage] = useState<Stage>(record?.aiDiagnosis ? "result" : "intro");
  const [diagnosis, setDiagnosis] = useState<AiDiagnosis | null>(record?.aiDiagnosis ?? null);
  const [preview, setPreview] = useState<DiagnosisPreview | null>(null);
  const [previewOpen, setPreviewOpen] = useState(true);
  const [busy, setBusy] = useState(false);
  const [diagnostic, setDiagnostic] = useState<DiagnosticOutput | null>(null);
  const [runningCommand, setRunningCommand] = useState<DiagnosticCommandId | null>(null);

  // 换一条记录就把上一条的预览 / 结果 / 诊断输出全部丢掉，避免串数据。
  useEffect(() => {
    setDiagnosis(record?.aiDiagnosis ?? null);
    setStage(record?.aiDiagnosis ? "result" : "intro");
    setPreview(null);
    setPreviewOpen(true);
    setBusy(false);
    setDiagnostic(null);
    setRunningCommand(null);
  }, [record?.id, record?.aiDiagnosis]);

  if (!record) return null;

  async function handlePrepare() {
    if (!record) return;
    setBusy(true);
    try {
      const data = await api.previewDeployDiagnosis(record.id);
      setPreview(data);
      setPreviewOpen(true);
      setStage("confirm");
    } catch (error) {
      toast("error", String(error));
    } finally {
      setBusy(false);
    }
  }

  async function handleAnalyze() {
    if (!record) return;
    setBusy(true);
    try {
      const result = await api.analyzeDeployFailure(record.id);
      setDiagnosis(result);
      setStage("result");
      onAnalyzed?.(result);
    } catch (error) {
      toast("error", String(error));
    } finally {
      setBusy(false);
    }
  }

  async function handleDiagnostic(command: DiagnosticCommandId) {
    if (!record) return;
    setRunningCommand(command);
    setDiagnostic(null);
    try {
      setDiagnostic(await api.runDeployDiagnostic(record.id, command));
    } catch (error) {
      toast("error", String(error));
    } finally {
      setRunningCommand(null);
    }
  }

  const footer =
    stage === "result" ? (
      <>
        <Button variant="secondary" onClick={onClose}>
          {t("common.close")}
        </Button>
        <Button
          icon={<RefreshCw className="size-4" />}
          loading={busy}
          onClick={() => void handlePrepare()}
        >
          {t("diagnosis.reanalyze")}
        </Button>
      </>
    ) : (
      <>
        <Button variant="secondary" onClick={onClose} disabled={busy}>
          {t("common.close")}
        </Button>
        {stage === "intro" ? (
          <Button
            icon={<Sparkles className="size-4" />}
            loading={busy}
            onClick={() => void handlePrepare()}
          >
            {t("diagnosis.prepare")}
          </Button>
        ) : (
          <Button
            icon={<Sparkles className="size-4" />}
            loading={busy}
            disabled={!preview}
            onClick={() => void handleAnalyze()}
          >
            {t("diagnosis.send")}
          </Button>
        )}
      </>
    );

  return (
    <Modal
      open
      onClose={onClose}
      title={t("diagnosis.title")}
      subtitle={`${record.repoName} · ${record.branch} · ${record.serverName}`}
      width="max-w-3xl"
      footer={footer}
    >
      {stage === "intro" && (
        <div className="flex flex-col gap-3 text-[12.5px] leading-relaxed text-ink-dim">
          <p>{t("diagnosis.intro")}</p>
          <p className="rounded-md border border-line bg-sunken px-4 py-3 text-[11.5px]">
            {t("diagnosis.privacy")}
          </p>
        </div>
      )}

      {stage === "confirm" && preview && (
        <div className="flex flex-col gap-3">
          <div className="flex items-center justify-between gap-3">
            <p className="text-[12.5px] text-ink-dim">{t("diagnosis.confirmHint")}</p>
            <span className="shrink-0 text-[11px] text-ink-faint">
              {t("diagnosis.previewSize", {
                total: humanSize(preview.totalBytes),
                log: humanSize(preview.logBytes),
              })}
            </span>
          </div>
          <button
            className="flex items-center gap-1 self-start text-[11.5px] text-ink-faint hover:text-ink"
            onClick={() => setPreviewOpen((open) => !open)}
          >
            {previewOpen ? (
              <ChevronDown className="size-3.5" />
            ) : (
              <ChevronRight className="size-3.5" />
            )}
            {t("diagnosis.previewLabel")}
          </button>
          {previewOpen && (
            <pre className="max-h-[46vh] overflow-auto rounded-md border border-line bg-sunken px-4 py-2.5 font-mono text-[11px] leading-[1.7] whitespace-pre-wrap text-ink-dim">
              {preview.context}
            </pre>
          )}
        </div>
      )}

      {stage === "result" && diagnosis && (
        <div className="flex flex-col gap-5">
          <div className="flex flex-wrap items-center gap-2">
            <Badge kind={confidenceKind(diagnosis)}>
              {t(`diagnosis.confidence.${diagnosis.confidence}`)}
            </Badge>
            <span className="text-[11px] text-ink-faint">
              {t("diagnosis.meta", {
                model: diagnosis.model,
                time: diagnosis.analyzedAt,
              })}
            </span>
            <span className="text-[11px] text-ink-faint">
              {t("diagnosis.analyzedLog", { size: humanSize(diagnosis.logBytes) })}
            </span>
          </div>

          <section>
            <h4 className="text-[11px] font-medium tracking-wide text-ink-faint uppercase">
              {t("diagnosis.summary")}
            </h4>
            <p className="mt-1.5 text-[13px] text-ink">{diagnosis.summary}</p>
          </section>

          <section>
            <h4 className="text-[11px] font-medium tracking-wide text-ink-faint uppercase">
              {t("diagnosis.cause")}
            </h4>
            <p className="mt-1.5 text-[12.5px] leading-relaxed whitespace-pre-wrap text-ink-dim">
              {diagnosis.likelyCause}
            </p>
          </section>

          {diagnosis.credentialSuspect && (
            <div className="flex gap-3 rounded-md border border-warn/40 bg-warn-soft px-4 py-3">
              <ShieldAlert className="mt-0.5 size-4 shrink-0 text-warn" />
              <div className="text-[12px] leading-relaxed text-warn">
                <p className="font-medium">{t("diagnosis.credentialTitle")}</p>
                <p className="mt-1 whitespace-pre-wrap">
                  {diagnosis.credentialNote || t("diagnosis.credentialFallback")}
                </p>
              </div>
            </div>
          )}

          {diagnosis.steps.length > 0 && (
            <section>
              <h4 className="text-[11px] font-medium tracking-wide text-ink-faint uppercase">
                {t("diagnosis.steps")}
              </h4>
              <ol className="mt-1.5 flex list-decimal flex-col gap-1.5 pl-5 text-[12.5px] leading-relaxed text-ink-dim">
                {diagnosis.steps.map((step, index) => (
                  <li key={index}>{step}</li>
                ))}
              </ol>
            </section>
          )}

          <section>
            <h4 className="text-[11px] font-medium tracking-wide text-ink-faint uppercase">
              {t("diagnosis.actions")}
            </h4>
            {diagnosis.actions.length === 0 ? (
              <p className="mt-1.5 text-[12px] text-ink-faint">{t("diagnosis.noActions")}</p>
            ) : (
              <div className="mt-2 flex flex-wrap gap-2">
                {diagnosis.actions.map((action, index) => (
                  <ActionButton
                    key={index}
                    action={action}
                    record={record}
                    runningCommand={runningCommand}
                    onRedeploy={() => onRedeploy(record)}
                    onOpenTerminal={() => {
                      onClose();
                      navigate(`/servers?server=${record.serverId}`);
                    }}
                    onDiagnostic={(command) => void handleDiagnostic(command)}
                  />
                ))}
              </div>
            )}
          </section>

          {diagnostic && (
            <section>
              <div className="flex items-center justify-between gap-2">
                <h4 className="text-[11px] font-medium tracking-wide text-ink-faint uppercase">
                  {t("diagnosis.diagnosticOutput")} ·{" "}
                  {t(`diagnosis.diagnostic.${diagnostic.command}`)}
                </h4>
                {diagnostic.truncated && (
                  <span className="text-[10px] text-warn">{t("diagnosis.truncated")}</span>
                )}
              </div>
              <pre className="mt-1.5 max-h-72 overflow-auto rounded-md border border-line bg-sunken px-4 py-2.5 font-mono text-[11px] leading-[1.7] whitespace-pre-wrap text-ink-dim">
                {diagnostic.output || t("diagnosis.diagnosticEmpty")}
              </pre>
            </section>
          )}
        </div>
      )}
    </Modal>
  );
}

function ActionButton({
  action,
  record,
  runningCommand,
  onRedeploy,
  onOpenTerminal,
  onDiagnostic,
}: {
  action: AiFixAction;
  record: DeployRecord;
  runningCommand: DiagnosticCommandId | null;
  onRedeploy: () => void;
  onOpenTerminal: () => void;
  onDiagnostic: (command: DiagnosticCommandId) => void;
}) {
  const { t } = useTranslation();
  switch (action.type) {
    case "redeploy":
      return (
        <Button
          size="sm"
          variant="secondary"
          icon={<RefreshCw className="size-3.5" />}
          disabled={record.status === "running"}
          onClick={onRedeploy}
        >
          {t("diagnosis.actionRedeploy")}
        </Button>
      );
    case "openTerminal":
      return (
        <Button
          size="sm"
          variant="secondary"
          icon={<Terminal className="size-3.5" />}
          onClick={onOpenTerminal}
        >
          {t("diagnosis.actionOpenTerminal")}
        </Button>
      );
    case "runDiagnostic":
      return (
        <Button
          size="sm"
          variant="secondary"
          icon={<Wrench className="size-3.5" />}
          loading={runningCommand === action.command}
          disabled={!!runningCommand}
          onClick={() => onDiagnostic(action.command)}
        >
          {t(`diagnosis.diagnostic.${action.command}`)}
        </Button>
      );
  }
}

function confidenceKind(diagnosis: AiDiagnosis): "green" | "amber" | "gray" {
  switch (diagnosis.confidence) {
    case "high":
      return "green";
    case "medium":
      return "amber";
    default:
      return "gray";
  }
}
