use deploy_core::diagnosis::{self, DiagnosisPreview, DiagnosticOutput};
use deploy_core::models::{AiDiagnosis, DiagnosticCommandId};
use deploy_core::Result;
use tauri::State;

use crate::state::AppState;

/// 预览：返回真正要发送给模型的脱敏上下文，用户确认后才发起分析。
#[tauri::command(async)]
pub fn preview_deploy_diagnosis(
    state: State<AppState>,
    record_id: String,
) -> Result<DiagnosisPreview> {
    diagnosis::preview(&state.store, &record_id)
}

/// 对一条失败的部署记录做一次 AI 诊断，结果同时写回记录。
#[tauri::command(async)]
pub async fn analyze_deploy_failure(
    state: State<'_, AppState>,
    record_id: String,
) -> Result<AiDiagnosis> {
    diagnosis::analyze(&state.store, &record_id).await
}

/// 执行一条白名单内的只读诊断命令（模型只能挑选 id，命令由后端拼装）。
#[tauri::command(async)]
pub async fn run_deploy_diagnostic(
    state: State<'_, AppState>,
    record_id: String,
    command: DiagnosticCommandId,
) -> Result<DiagnosticOutput> {
    diagnosis::run_diagnostic(&state.store, &record_id, command).await
}
