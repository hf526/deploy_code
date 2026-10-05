import { invoke } from "@tauri-apps/api/core";

import type {
  CronJob,
  CronJobDraft,
  CronJobRun,
} from "../types";

export const cronjobCommands = {
  // 定时请求（cron-job.org 云端调度，任务不落本地盘）
  listCronJobs: () => invoke<CronJob[]>("list_cron_jobs"),
  saveCronJob: (draft: CronJobDraft) => invoke<void>("save_cron_job", { draft }),
  deleteCronJob: (jobId: number) => invoke<void>("delete_cron_job", { jobId }),
  setCronJobEnabled: (jobId: number, enabled: boolean) =>
    invoke<void>("set_cron_job_enabled", { jobId, enabled }),
  cronJobHistory: (jobId: number) => invoke<CronJobRun[]>("cron_job_history", { jobId }),
};
