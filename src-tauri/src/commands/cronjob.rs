use deploy_core::cronjob::{self, CronJob, CronJobDraft, CronJobRun};
use deploy_core::Result;
use tauri::State;

use crate::state::AppState;

/// cron-job.org 是远端服务，这里只做「取 Key + 转调」，任务本身不存在本地配置里。
/// 全部标 async：单次请求最长 30s，不能卡住主线程；但都是用户主动触发，不占部署/备份任务锁。
fn api_key(state: &State<AppState>) -> Result<String> {
    Ok(state.store.load_config()?.settings.cronjob_api_key)
}

#[tauri::command(async)]
pub fn list_cron_jobs(state: State<AppState>) -> Result<Vec<CronJob>> {
    cronjob::list_jobs(&api_key(&state)?)
}

/// 新建或更新（`draft.jobId` 为空表示新建）。服务端创建只回 id，所以不返回详情，
/// 由前端刷新列表，省下一次 API 配额。
#[tauri::command(async)]
pub fn save_cron_job(state: State<AppState>, draft: CronJobDraft) -> Result<()> {
    cronjob::save_job(&api_key(&state)?, &draft)
}

#[tauri::command(async)]
pub fn delete_cron_job(state: State<AppState>, job_id: i64) -> Result<()> {
    cronjob::delete_job(&api_key(&state)?, job_id)
}

#[tauri::command(async)]
pub fn set_cron_job_enabled(
    state: State<AppState>,
    job_id: i64,
    enabled: bool,
) -> Result<()> {
    cronjob::set_enabled(&api_key(&state)?, job_id, enabled)
}

#[tauri::command(async)]
pub fn cron_job_history(state: State<AppState>, job_id: i64) -> Result<Vec<CronJobRun>> {
    cronjob::job_history(&api_key(&state)?, job_id)
}
