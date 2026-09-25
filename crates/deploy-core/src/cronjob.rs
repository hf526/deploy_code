//! cron-job.org 云端定时请求：REST 客户端 + cron 表达式与它自有 schedule 结构的互转。
//!
//! 调度由对方云端执行，本机不需要任何常驻进程；因此这里只承担「配置管理」，
//! 没有 run-now 端点，暂停走 `PATCH {enabled:false}`。
//! 官方 API 默认限额 100 次/天，所以调用方只在打开页面和手动刷新时拉取，禁止轮询。

use std::collections::BTreeMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::error::{CoreError, Result};
use crate::process::{run_timeout, CommandOutput};

const API_BASE: &str = "https://api.cron-job.org";
/// curl 自身留 5 秒余量先退出，避免进程被硬杀时拿不到可读错误。
const CURL_MAX_TIME_SECS: u64 = 25;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// 支持的 HTTP 方法，下标即 cron-job.org 的 `requestMethod` 数值。
pub const CRON_METHODS: [&str; 9] = [
    "GET", "POST", "OPTIONS", "HEAD", "PUT", "DELETE", "TRACE", "CONNECT", "PATCH",
];

/// 请求头一条。UI 用有序列表编辑，提交时转成服务端的 header 字典。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CronHeader {
    pub key: String,
    pub value: String,
}

/// cron-job.org 的调度结构：五个字段都是整数数组，`[-1]` 表示「任意」。
/// 注意它不吃 cron 表达式，所以 [`cron_to_schedule`] / [`schedule_to_cron`] 负责互转。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CronSchedule {
    #[serde(default)]
    pub timezone: String,
    #[serde(default)]
    pub minutes: Vec<i64>,
    #[serde(default)]
    pub hours: Vec<i64>,
    #[serde(default)]
    pub mdays: Vec<i64>,
    #[serde(default)]
    pub months: Vec<i64>,
    #[serde(default)]
    pub wdays: Vec<i64>,
}

/// 远端任务，只保留界面要用的字段，服务端多出来的字段忽略。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CronJob {
    pub job_id: i64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub request_method: u8,
    #[serde(default)]
    pub request_timeout: u64,
    #[serde(default)]
    pub schedule: CronSchedule,
    /// 由 `schedule` 反算出的 5 段表达式：服务端不返回，读取后本地补齐，让 UI 能直接编辑。
    #[serde(default)]
    pub cron: String,
    #[serde(default)]
    pub extended_data: CronExtendedData,
    #[serde(default)]
    pub last_duration: i64,
    /// 上次实际执行的 unix 秒；0 表示从未执行。
    #[serde(default)]
    pub last_execution: i64,
    #[serde(default)]
    pub next_execution: i64,
}

/// 任务的请求头与请求体容器。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CronExtendedData {
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default)]
    pub body: String,
}

/// 新建 / 编辑任务的入参（UI 直接提交，`cron` 为标准 5 段表达式）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CronJobDraft {
    /// None 表示新建，Some 表示更新该 job_id。
    #[serde(default)]
    pub job_id: Option<i64>,
    pub title: String,
    pub url: String,
    pub method: String,
    pub headers: Vec<CronHeader>,
    pub body: String,
    pub timeout_secs: u64,
    pub enabled: bool,
    pub cron: String,
    pub timezone: String,
}

/// 一次执行记录（历史弹窗用）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CronJobRun {
    /// 服务端给的执行标识，用作列表 key。
    #[serde(default)]
    pub identifier: String,
    /// 实际执行的 unix 秒。
    #[serde(default)]
    pub date: i64,
    #[serde(default)]
    pub status: i64,
    #[serde(default)]
    pub status_text: String,
    #[serde(default)]
    pub http_status: i64,
    #[serde(default)]
    pub duration: i64,
    #[serde(default)]
    pub url: String,
}

fn require_key(api_key: &str) -> Result<&str> {
    let key = api_key.trim();
    if key.is_empty() {
        return Err(CoreError::cronjob(
            "还没有填写 cron-job.org API Key：到「设置 → 定时请求」里填入控制台生成的 Key",
        ));
    }
    Ok(key)
}

/// 拆分 `curl -w "\n%{http_code}"` 的输出：(响应体, 状态码文本)。
fn split_status(output: &str) -> (String, String) {
    let trimmed = output.trim_end();
    match trimmed.rfind('\n') {
        Some(index) => (
            trimmed[..index].to_string(),
            trimmed[index + 1..].trim().to_string(),
        ),
        None => (String::new(), trimmed.to_string()),
    }
}

/// 把服务端的错误状态码翻译成「原因 + 下一步」。
fn describe_status(status: u16, body: &str) -> String {
    let detail = clip(body, 200);
    match status {
        400 => format!("请求被拒绝（表单有误）：{detail}"),
        401 => "API Key 无效或已删除：到 cron-job.org 控制台重新生成后回填「设置 → 定时请求」".to_string(),
        403 => format!("访问被禁止（Key 可能开启了 IP 白名单，或该账号无权访问此接口）：{detail}"),
        404 => "任务不存在：可能已在控制台删除，刷新列表后再试".to_string(),
        409 => "任务标题已存在：cron-job.org 要求同一账号内标题唯一，请改名后重试".to_string(),
        429 => "已超出 API 限额（默认 100 次/天）：明天再试，或到控制台申请提高配额".to_string(),
        other if other >= 500 => format!("cron-job.org 服务异常（{other}）：稍后重试"),
        other => format!("cron-job.org 返回 {other}：{detail}"),
    }
}

fn clip(text: &str, max_chars: usize) -> String {
    let flat: String = text.trim().chars().filter(|c| *c != '\r' && *c != '\n').collect();
    if flat.chars().count() <= max_chars {
        return flat;
    }
    let head: String = flat.chars().take(max_chars).collect();
    format!("{head}…")
}

fn call(api_key: &str, method: &str, path: &str, payload: Option<&Value>) -> Result<Value> {
    let key = require_key(api_key)?;
    let mut args: Vec<String> = vec![
        "-sS".to_string(),
        "--max-time".to_string(),
        CURL_MAX_TIME_SECS.to_string(),
        "-X".to_string(),
        method.to_string(),
        "-H".to_string(),
        format!("Authorization: Bearer {key}"),
        "-w".to_string(),
        "\n%{http_code}".to_string(),
    ];
    if let Some(body) = payload {
        args.push("-H".to_string());
        args.push("Content-Type: application/json".to_string());
        // --data-binary：不裁剪换行，避免转义后的 JSON 被改动。
        args.push("--data-binary".to_string());
        args.push(body.to_string());
    }
    args.push(format!("{API_BASE}{path}"));

    let out: CommandOutput = run_timeout("curl", &args, None, REQUEST_TIMEOUT)?;
    if out.code != 0 {
        return Err(CoreError::cronjob(format!(
            "无法访问 {API_BASE}（curl 退出码 {}）：{}",
            out.code,
            clip(&out.stderr, 200)
        )));
    }
    let (body, status_text) = split_status(&out.stdout);
    let Ok(status) = status_text.parse::<u16>() else {
        return Err(CoreError::cronjob(format!(
            "未取到 HTTP 状态码，响应内容：{}",
            clip(&out.stdout, 200)
        )));
    };
    if !(200..=299).contains(&status) {
        return Err(CoreError::cronjob(describe_status(status, &body)));
    }
    // 删除 / 更新返回 `{}`，空响应体按空对象处理。
    if body.trim().is_empty() {
        return Ok(Value::Object(Map::new()));
    }
    serde_json::from_str(&body).map_err(|e| {
        CoreError::cronjob(format!("响应解析失败：{e}；内容：{}", clip(&body, 200)))
    })
}

fn method_to_code(method: &str) -> Result<u8> {
    let upper = method.trim().to_uppercase();
    CRON_METHODS
        .iter()
        .position(|item| *item == upper)
        .map(|index| index as u8)
        .ok_or_else(|| {
            CoreError::cronjob(format!("不支持的 HTTP 方法：{method}"))
        })
}

/// 服务端的方法编号转名字，未知编号回落 GET（不让一个脏数据把整页打挂）。
pub fn method_name(code: u8) -> &'static str {
    CRON_METHODS.get(code as usize).copied().unwrap_or("GET")
}

struct FieldSpec {
    name: &'static str,
    min: i64,
    max: i64,
}

const fn field_specs() -> [FieldSpec; 5] {
    [
        FieldSpec { name: "分钟", min: 0, max: 59 },
        FieldSpec { name: "小时", min: 0, max: 23 },
        FieldSpec { name: "日", min: 1, max: 31 },
        FieldSpec { name: "月", min: 1, max: 12 },
        // 0 = 周日，与标准 cron 一致。
        FieldSpec { name: "星期", min: 0, max: 6 },
    ]
}

fn cron_error(msg: impl Into<String>) -> CoreError {
    CoreError::cronjob(format!(
        "{}（只支持 5 段表达式：分 时 日 月 星期，字段可用 * 、*/步长、a、a-b、a-b/步长和逗号列表）",
        msg.into()
    ))
}

/// 解析单个 cron 字段为升序去重的整数集合。
fn parse_cron_field(spec: &FieldSpec, text: &str) -> Result<Vec<i64>> {
    let text = text.trim();
    if text.is_empty() {
        return Err(cron_error(format!("{}字段为空", spec.name)));
    }
    let mut values: Vec<i64> = Vec::new();
    for part in text.split(',') {
        let part = part.trim();
        if part.is_empty() {
            return Err(cron_error(format!("{}字段出现空项：{text}", spec.name)));
        }
        let (range_text, step) = match part.split_once('/') {
            Some((head, tail)) => {
                let step: i64 = tail.trim().parse().map_err(|_| {
                    cron_error(format!("{}字段的步长不是整数：{part}", spec.name))
                })?;
                if step < 1 {
                    return Err(cron_error(format!("{}字段的步长必须 >= 1", spec.name)));
                }
                (head.trim(), step)
            }
            None => (part, 1),
        };
        let (start, end) = if range_text == "*" {
            (spec.min, spec.max)
        } else if let Some((from, to)) = range_text.split_once('-') {
            let from: i64 = from.trim().parse().map_err(|_| {
                cron_error(format!("{}字段的区间写法有误：{part}", spec.name))
            })?;
            let to: i64 = to.trim().parse().map_err(|_| {
                cron_error(format!("{}字段的区间写法有误：{part}", spec.name))
            })?;
            (from, to)
        } else {
            let single: i64 = range_text.parse().map_err(|_| {
                cron_error(format!("{}字段不是数字或 *：{part}", spec.name))
            })?;
            // `5/2` 这种「起点 + 步长」写法在 cron 里合法，等价于从 5 到上界按步长取。
            if step > 1 {
                (single, spec.max)
            } else {
                (single, single)
            }
        };
        if start > end || start < spec.min || end > spec.max {
            return Err(cron_error(format!(
                "{}字段的取值超出 {min}~{max}：{part}",
                spec.name,
                min = spec.min,
                max = spec.max
            )));
        }
        let mut current = start;
        while current <= end {
            values.push(current);
            current += step;
        }
    }
    values.sort_unstable();
    values.dedup();
    Ok(values)
}

/// 覆盖整个值域时统一收敛成 `[-1]`，与服务端的「任意」写法保持一致，也保证往返稳定。
fn collapse(values: Vec<i64>, spec: &FieldSpec) -> Vec<i64> {
    let full = spec.max - spec.min + 1;
    if values.len() as i64 == full {
        return vec![-1];
    }
    values
}

/// 标准 5 段 cron → cron-job.org 的 schedule 结构。
pub fn cron_to_schedule(expression: &str, timezone: &str) -> Result<CronSchedule> {
    let fields: Vec<&str> = expression.split_whitespace().collect();
    if fields.len() != 5 {
        return Err(cron_error(format!(
            "需要 5 个字段，实际 {} 个",
            fields.len()
        )));
    }
    let specs = field_specs();
    let mut parsed = Vec::with_capacity(5);
    for (spec, text) in specs.iter().zip(fields) {
        parsed.push(collapse(parse_cron_field(spec, text)?, spec));
    }
    let tz = timezone.trim();
    Ok(CronSchedule {
        timezone: if tz.is_empty() { "UTC".to_string() } else { tz.to_string() },
        minutes: parsed[0].clone(),
        hours: parsed[1].clone(),
        mdays: parsed[2].clone(),
        months: parsed[3].clone(),
        wdays: parsed[4].clone(),
    })
}

fn render_field(values: &[i64], spec: &FieldSpec) -> String {
    let values: Vec<i64> = values
        .iter()
        .filter(|value| **value >= 0)
        .copied()
        .collect();
    if values.is_empty() || values.len() as i64 == spec.max - spec.min + 1 {
        return "*".to_string();
    }
    // 从下界起、等间距且再走一步就越界的集合，等价于 `*/步长`，还原成它更好读。
    if values[0] == spec.min && values.len() > 1 {
        let step = values[1] - values[0];
        let even = step > 1 && values.windows(2).all(|pair| pair[1] - pair[0] == step);
        if even && values[values.len() - 1] + step > spec.max {
            return format!("*/{step}");
        }
    }
    values
        .into_iter()
        .map(|value| value.to_string())
        .collect::<Vec<String>>()
        .join(",")
}

/// schedule → 5 段 cron 表达式（列表按升序逗号展开，保证能原样解析回去）。
pub fn schedule_to_cron(schedule: &CronSchedule) -> String {
    let specs = field_specs();
    [
        render_field(&schedule.minutes, &specs[0]),
        render_field(&schedule.hours, &specs[1]),
        render_field(&schedule.mdays, &specs[2]),
        render_field(&schedule.months, &specs[3]),
        render_field(&schedule.wdays, &specs[4]),
    ]
    .join(" ")
}

/// 校验 UI 提交的草稿，返回裁剪过的副本，避免把空格写进远端配置。
fn validate(draft: &CronJobDraft) -> Result<CronJobDraft> {
    let title = draft.title.trim().to_string();
    if title.is_empty() {
        return Err(CoreError::cronjob("任务名称不能为空"));
    }
    let url = draft.url.trim().to_string();
    let lower = url.to_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        return Err(CoreError::cronjob("请求地址必须以 http:// 或 https:// 开头"));
    }
    if url.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(CoreError::cronjob("请求地址不能包含空格或换行"));
    }
    let method = draft.method.trim().to_uppercase();
    method_to_code(&method)?;
    if draft.timeout_secs < 1 || draft.timeout_secs > 600 {
        return Err(CoreError::cronjob("超时秒数需要在 1~600 之间"));
    }
    let mut seen: Vec<String> = Vec::new();
    for header in &draft.headers {
        let key = header.key.trim();
        if key.is_empty() {
            continue;
        }
        if key.contains(':') || key.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err(CoreError::cronjob(format!("请求头名称有误：{key}")));
        }
        if header.value.chars().any(|c| c == '\r' || c == '\n') {
            return Err(CoreError::cronjob(format!("请求头 {key} 的值不能包含换行")));
        }
        if seen.iter().any(|existing| existing.eq_ignore_ascii_case(key)) {
            return Err(CoreError::cronjob(format!("请求头重复：{key}")));
        }
        seen.push(key.to_string());
    }
    // 先解析一遍，报错信息比 schedule 结构可读。
    cron_to_schedule(&draft.cron, &draft.timezone)?;
    Ok(CronJobDraft {
        job_id: draft.job_id,
        title,
        url,
        method,
        headers: draft
            .headers
            .iter()
            .filter(|header| !header.key.trim().is_empty())
            .map(|header| CronHeader {
                key: header.key.trim().to_string(),
                value: header.value.clone(),
            })
            .collect(),
        body: draft.body.clone(),
        timeout_secs: draft.timeout_secs,
        enabled: draft.enabled,
        cron: draft.cron.trim().to_string(),
        timezone: {
            let tz = draft.timezone.trim();
            if tz.is_empty() {
                "UTC".to_string()
            } else {
                tz.to_string()
            }
        },
    })
}

fn payload(clean: &CronJobDraft) -> Result<Value> {
    let schedule = cron_to_schedule(&clean.cron, &clean.timezone)?;
    let headers: BTreeMap<String, String> = clean
        .headers
        .iter()
        .map(|header| (header.key.clone(), header.value.clone()))
        .collect();
    let mut job = Map::new();
    job.insert("title".to_string(), clean.title.clone().into());
    job.insert("url".to_string(), clean.url.clone().into());
    job.insert("enabled".to_string(), clean.enabled.into());
    job.insert(
        "requestMethod".to_string(),
        method_to_code(&clean.method)?.into(),
    );
    job.insert("requestTimeout".to_string(), clean.timeout_secs.into());
    job.insert(
        "schedule".to_string(),
        serde_json::to_value(schedule).map_err(CoreError::Serde)?,
    );
    job.insert(
        "extendedData".to_string(),
        serde_json::json!({ "headers": headers, "body": clean.body }),
    );
    Ok(serde_json::json!({ "job": job }))
}

fn read_jobs(value: Value) -> Result<Vec<CronJob>> {
    let items = value
        .get("jobs")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    items
        .into_iter()
        .map(|item| {
            let mut job = serde_json::from_value::<CronJob>(item).map_err(CoreError::Serde)?;
            job.cron = schedule_to_cron(&job.schedule);
            Ok(job)
        })
        .collect()
}

/// 列出账号下所有任务（按标题排序，方便和 UI 对照）。
pub fn list_jobs(api_key: &str) -> Result<Vec<CronJob>> {
    let mut jobs = read_jobs(call(api_key, "GET", "/jobs", None)?)?;
    jobs.sort_by(|left, right| {
        left.title
            .to_lowercase()
            .cmp(&right.title.to_lowercase())
    });
    Ok(jobs)
}

/// 新建或更新：`job_id` 为 None 时创建。服务端创建只回 `jobId`，
/// 所以这里不返回任务详情，由调用方刷新列表，省下一次 API 配额。
pub fn save_job(api_key: &str, draft: &CronJobDraft) -> Result<()> {
    let clean = validate(draft)?;
    let body = payload(&clean)?;
    match clean.job_id {
        Some(job_id) => {
            call(api_key, "PATCH", &format!("/jobs/{job_id}"), Some(&body))?;
        }
        None => {
            call(api_key, "PUT", "/jobs", Some(&body))?;
        }
    }
    Ok(())
}

pub fn delete_job(api_key: &str, job_id: i64) -> Result<()> {
    call(api_key, "DELETE", &format!("/jobs/{job_id}"), None)?;
    Ok(())
}

/// 开关任务。只提交 `enabled` 一个字段，避免把列表里没读的字段覆写回服务端。
pub fn set_enabled(api_key: &str, job_id: i64, enabled: bool) -> Result<()> {
    let body = serde_json::json!({ "job": { "enabled": enabled } });
    call(api_key, "PATCH", &format!("/jobs/{job_id}"), Some(&body))?;
    Ok(())
}

/// 最近执行记录。文档只固定了单条记录的字段，外层容器写法两种都出现过，故都兼容。
pub fn job_history(api_key: &str, job_id: i64) -> Result<Vec<CronJobRun>> {
    let value = call(api_key, "GET", &format!("/jobs/{job_id}/history"), None)?;
    let items = value
        .get("history")
        .and_then(Value::as_array)
        .or_else(|| value.as_array())
        .cloned()
        .unwrap_or_default();
    let mut runs = items
        .into_iter()
        .map(|item| serde_json::from_value::<CronJobRun>(item).map_err(CoreError::Serde))
        .collect::<Result<Vec<_>>>()?;
    runs.sort_by(|left, right| right.date.cmp(&left.date));
    Ok(runs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schedule_of(cron: &str) -> CronSchedule {
        cron_to_schedule(cron, "Asia/Shanghai").unwrap()
    }

    #[test]
    fn cron_to_schedule_uses_wildcard_minus_one() {
        let every_five = schedule_of("*/5 * * * *");
        assert_eq!(every_five.timezone, "Asia/Shanghai");
        assert_eq!(every_five.minutes.first(), Some(&0));
        assert_eq!(every_five.minutes.len(), 12);
        assert_eq!(every_five.hours, vec![-1]);
        assert_eq!(every_five.mdays, vec![-1]);
        assert_eq!(every_five.months, vec![-1]);
        assert_eq!(every_five.wdays, vec![-1]);
    }

    #[test]
    fn cron_to_schedule_expands_lists_and_ranges() {
        let schedule = schedule_of("0,30 9-11 * * 1-5");
        assert_eq!(schedule.minutes, vec![0, 30]);
        assert_eq!(schedule.hours, vec![9, 10, 11]);
        assert_eq!(schedule.wdays, vec![1, 2, 3, 4, 5]);
        // 全值域收敛成 -1，和服务端写法一致。
        assert_eq!(schedule.mdays, vec![-1]);
    }

    #[test]
    fn schedule_to_cron_round_trips() {
        for expression in ["*/5 * * * *", "0 3 * * *", "0,30 9-11 * * 1-5", "15 2 1 * *"] {
            let schedule = schedule_of(expression);
            let rendered = schedule_to_cron(&schedule);
            assert_eq!(
                schedule_to_cron(&cron_to_schedule(&rendered, "UTC").unwrap()),
                rendered,
                "{expression} 往返后不稳定"
            );
        }
        assert_eq!(schedule_to_cron(&schedule_of("0 3 * * *")), "0 3 * * *");
        assert_eq!(schedule_to_cron(&schedule_of("* * * * *")), "* * * * *");
    }

    #[test]
    fn cron_rejects_out_of_range_and_bad_shapes() {
        assert!(cron_to_schedule("60 * * * *", "UTC").is_err());
        assert!(cron_to_schedule("* 24 * * *", "UTC").is_err());
        assert!(cron_to_schedule("* * 0 * *", "UTC").is_err());
        assert!(cron_to_schedule("* * * * 7", "UTC").is_err());
        assert!(cron_to_schedule("*/0 * * * *", "UTC").is_err());
        assert!(cron_to_schedule("5-1 * * * *", "UTC").is_err());
        assert!(cron_to_schedule("* * * *", "UTC").is_err());
        assert!(cron_to_schedule("a * * * *", "UTC").is_err());
        // 错误文案要能告诉用户下一步怎么改。
        let err = cron_to_schedule("70 * * * *", "UTC").unwrap_err().to_string();
        assert!(err.contains("5 段"), "{err}");
    }

    #[test]
    fn timezone_falls_back_to_utc() {
        assert_eq!(
            cron_to_schedule("0 * * * *", "").unwrap().timezone,
            "UTC"
        );
        assert_eq!(cron_to_schedule("0 * * * *", "  ").unwrap().timezone, "UTC");
        assert_eq!(schedule_of("0 * * * *").timezone, "Asia/Shanghai");
    }

    #[test]
    fn method_mapping_matches_service_numbering() {
        assert_eq!(method_to_code("get").unwrap(), 0);
        assert_eq!(method_to_code("POST").unwrap(), 1);
        assert_eq!(method_to_code("PATCH").unwrap(), 8);
        assert_eq!(method_name(1), "POST");
        assert_eq!(method_name(200), "GET");
        assert!(method_to_code("BREW").is_err());
    }

    #[test]
    fn validate_rejects_dead_urls_and_bad_headers() {
        let base = CronJobDraft {
            title: "打卡".into(),
            url: "https://example.com/ping".into(),
            method: "GET".into(),
            timeout_secs: 15,
            cron: "*/10 * * * *".into(),
            timezone: "Asia/Shanghai".into(),
            enabled: true,
            ..Default::default()
        };
        let clean = validate(&base).unwrap();
        assert_eq!(clean.title, "打卡");
        assert_eq!(clean.cron, "*/10 * * * *");

        let mut no_scheme = base.clone();
        no_scheme.url = "example.com/ping".into();
        assert!(validate(&no_scheme).is_err());

        let mut spaced = base.clone();
        spaced.url = "https://example.com/a b".into();
        assert!(validate(&spaced).is_err());

        let mut empty_title = base.clone();
        empty_title.title = "   ".into();
        assert!(validate(&empty_title).is_err());

        let mut newline_header = base.clone();
        newline_header.headers = vec![CronHeader {
            key: "X-Token".into(),
            value: "a\r\nInjected: 1".into(),
        }];
        assert!(validate(&newline_header).is_err());

        let mut duplicated = base.clone();
        duplicated.headers = vec![
            CronHeader { key: "X-A".into(), value: "1".into() },
            CronHeader { key: "x-a".into(), value: "2".into() },
        ];
        assert!(validate(&duplicated).is_err());

        // 空键的行直接丢掉，不报错（UI 允许留一行占位）。
        let mut blank = base.clone();
        blank.headers = vec![CronHeader { key: "  ".into(), value: "x".into() }];
        assert!(validate(&blank).unwrap().headers.is_empty());
    }

    #[test]
    fn payload_wraps_job_and_maps_headers() {
        let draft = CronJobDraft {
            title: " 打卡 ".into(),
            url: " https://example.com/ping ".into(),
            method: "post".into(),
            headers: vec![CronHeader { key: "X-A".into(), value: "1".into() }],
            body: "{\"k\":1}".into(),
            timeout_secs: 20,
            cron: "0 3 * * *".into(),
            timezone: "Asia/Shanghai".into(),
            enabled: true,
            ..Default::default()
        };
        let value = payload(&validate(&draft).unwrap()).unwrap();
        let job = value.get("job").expect("外层要包 job");
        assert_eq!(job.get("title").unwrap(), "打卡");
        assert_eq!(job.get("url").unwrap(), "https://example.com/ping");
        assert_eq!(job.get("requestMethod").unwrap(), 1);
        assert_eq!(job.get("requestTimeout").unwrap(), 20);
        assert_eq!(job["extendedData"]["headers"]["X-A"], "1");
        assert_eq!(job["schedule"]["hours"][0], 3);
    }

    #[test]
    fn missing_api_key_is_reported_before_network() {
        let err = list_jobs("   ").unwrap_err().to_string();
        assert!(err.contains("API Key"), "{err}");
    }

    #[test]
    fn status_codes_map_to_actionable_messages() {
        assert!(describe_status(401, "").contains("API Key"));
        assert!(describe_status(409, "").contains("标题"));
        assert!(describe_status(429, "").contains("限额"));
        assert!(describe_status(404, "").contains("刷新"));
        assert!(describe_status(503, "").contains("服务异常"));
    }

    #[test]
    fn split_status_keeps_body_and_code() {
        let (body, status) = split_status("{\"jobs\":[]}\n200");
        assert_eq!(body, "{\"jobs\":[]}");
        assert_eq!(status, "200");
        let (body, status) = split_status("204");
        assert!(body.is_empty());
        assert_eq!(status, "204");
    }

    #[test]
    fn job_deserializes_with_unknown_fields_and_cron_round_trip() {
        let value = serde_json::json!({
            "jobId": 7,
            "title": "打卡",
            "url": "https://example.com/ping",
            "enabled": true,
            "requestMethod": 1,
            "requestTimeout": 300,
            "lastDuration": 120,
            "lastExecution": 1_700_000_000,
            "nextExecution": 1_700_000_060,
            "sslCertExpiry": 1_788_039_686,
            "someFailed": false,
            "schedule": {
                "timezone": "Asia/Shanghai",
                "expiresAt": 0,
                "minutes": [0, 15, 30, 45],
                "hours": [-1],
                "mdays": [-1],
                "months": [-1],
                "wdays": [-1]
            },
            "extendedData": { "headers": { "X-A": "1" }, "body": "" }
        });
        let job: CronJob = serde_json::from_value(value).unwrap();
        assert_eq!(job.job_id, 7);
        assert_eq!(job.extended_data.headers.get("X-A").map(String::as_str), Some("1"));
        assert_eq!(schedule_to_cron(&job.schedule), "*/15 * * * *");
    }

    #[test]
    fn read_jobs_backfills_cron_for_the_ui() {
        let value = serde_json::json!({
            "jobs": [{
                "jobId": 1,
                "title": "打卡",
                "url": "https://example.com/ping",
                "schedule": {
                    "timezone": "Asia/Shanghai",
                    "minutes": [0],
                    "hours": [3],
                    "mdays": [-1],
                    "months": [-1],
                    "wdays": [-1]
                }
            }],
            "someFailed": false
        });
        let jobs = read_jobs(value).unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].cron, "0 3 * * *");
        // 缺字段的旧数据不能让整页失败。
        assert!(read_jobs(serde_json::json!({})).unwrap().is_empty());
    }
}
