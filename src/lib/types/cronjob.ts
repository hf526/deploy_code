// ---------------------------------------------------------------------------
// cron-job.org 云端定时请求（镜像 deploy-core src/cronjob.rs）
// ---------------------------------------------------------------------------

/** 请求头一条：UI 用有序列表编辑，提交时转成 header 字典。 */
export interface CronHeader {
  key: string;
  value: string;
}

/** cron-job.org 的调度结构：整数数组，`[-1]` 表示「任意」。 */
export interface CronSchedule {
  timezone: string;
  minutes: number[];
  hours: number[];
  mdays: number[];
  months: number[];
  wdays: number[];
}

export interface CronExtendedData {
  headers: Record<string, string>;
  body: string;
}

/** 远端任务（服务端为准，本地不落盘）。 */
export interface CronJob {
  jobId: number;
  title: string;
  url: string;
  enabled: boolean;
  /** 下标对应 CRON_METHODS：0=GET 1=POST 2=OPTIONS 3=HEAD 4=PUT 5=DELETE 6=TRACE 7=CONNECT 8=PATCH。 */
  requestMethod: number;
  requestTimeout: number;
  schedule: CronSchedule;
  /** 后端由 schedule 反算出的 5 段表达式，服务端并不返回它。 */
  cron: string;
  extendedData: CronExtendedData;
  lastDuration: number;
  /** 上次实际执行的 unix 秒；0 表示从未执行。 */
  lastExecution: number;
  nextExecution: number;
}

/** 新建 / 编辑任务的提交体，`cron` 为标准 5 段表达式。 */
export interface CronJobDraft {
  jobId: number | null;
  title: string;
  url: string;
  method: string;
  headers: CronHeader[];
  body: string;
  timeoutSecs: number;
  enabled: boolean;
  cron: string;
  timezone: string;
}

/** 一次执行记录。 */
export interface CronJobRun {
  /** 服务端给的执行标识，用作列表 key。 */
  identifier: string;
  date: number;
  status: number;
  statusText: string;
  httpStatus: number;
  duration: number;
  url: string;
}
