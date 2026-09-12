use deploy_core::models::DeployRecord;
use deploy_core::Result;
use tauri::State;

use crate::state::AppState;

#[tauri::command(async)]
pub fn list_history(state: State<AppState>, repo_id: Option<String>) -> Result<Vec<DeployRecord>> {
    let mut records = state.store.load_history()?;
    records.reverse();
    if let Some(repo_id) = repo_id.filter(|value| !value.is_empty()) {
        records.retain(|record| record.repo_id == repo_id);
    }
    Ok(records)
}

#[tauri::command(async)]
pub fn get_record(state: State<AppState>, record_id: String) -> Result<DeployRecord> {
    state.store.find_record(&record_id)
}

#[tauri::command(async)]
pub fn delete_record(state: State<AppState>, record_id: String) -> Result<bool> {
    state.store.remove_history(&record_id)
}

#[tauri::command(async)]
pub fn clear_history(state: State<AppState>) -> Result<()> {
    state.store.clear_history()
}
