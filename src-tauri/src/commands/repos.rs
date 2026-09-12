use deploy_core::models::{
    Branch, Commit, FileContent, FileEntry, GraphCommit, RepoConfig, RepoInfo, RepoStatus,
    ReplaceSummary, ResolvedRev, SearchHit,
};
use deploy_core::{repo_info, CoreError, Git, Result, Store};
use serde::Serialize;
use tauri::State;

use crate::state::AppState;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoDetail {
    pub repo: RepoInfo,
    pub status: Option<RepoStatus>,
}

fn get_repo(state: &State<AppState>, repo_id: &str) -> Result<RepoConfig> {
    let config = state.store.load_config()?;
    Ok(Store::find_repo(&config, repo_id)?.clone())
}

fn git_for(state: &State<AppState>, repo_id: &str) -> Result<Git> {
    let repo = get_repo(state, repo_id)?;
    Git::open(&repo.path)
}

#[tauri::command(async)]
pub fn list_repos(state: State<AppState>) -> Result<Vec<RepoInfo>> {
    let config = state.store.load_config()?;
    Ok(config.repos.iter().map(repo_info).collect())
}

#[tauri::command(async)]
pub fn repo_detail(state: State<AppState>, repo_id: String) -> Result<RepoDetail> {
    let repo = get_repo(&state, &repo_id)?;
    let info = repo_info(&repo);
    let status = Git::open(&repo.path).ok().and_then(|git| git.status().ok());
    Ok(RepoDetail { repo: info, status })
}

#[tauri::command(async)]
pub fn add_repo(
    state: State<AppState>,
    path: String,
    name: Option<String>,
    default_server_id: Option<String>,
    default_target_dir: Option<String>,
) -> Result<RepoInfo> {
    let path_buf = deploy_core::process::canonicalize_path(std::path::Path::new(&path));
    let normalized = path_buf.to_string_lossy().replace('\\', "/");

    // IDE 式“打开文件夹”语义：已经打开过就直接复用，不报错。
    {
        let config = state.store.load_config()?;
        if let Some(existing) = config.repos.iter().find(|repo| {
            repo.path.replace('\\', "/").trim_end_matches('/') == normalized.trim_end_matches('/')
        }) {
            return Ok(repo_info(existing));
        }
    }

    // Git 校验放在锁外，避免持有写锁时执行 git 子进程。
    Git::open(&path_buf)?;

    let base = name
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .or_else(|| {
            path_buf
                .file_name()
                .map(|value| value.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "repo".to_string());

    let repo = state.store.mutate_config(|config| {
        if let Some(existing) = config.repos.iter().find(|repo| {
            repo.path.replace('\\', "/").trim_end_matches('/') == normalized.trim_end_matches('/')
        }) {
            return Ok(existing.clone());
        }
        // 同名自动加序号，避免打开重名目录时报错。
        let mut name = base.clone();
        let mut seq = 2;
        while config.repos.iter().any(|repo| repo.name == name) {
            name = format!("{base}-{seq}");
            seq += 1;
        }
        let mut repo = RepoConfig::new(name, path_buf.to_string_lossy().into_owned());
        repo.default_server_id = default_server_id.filter(|value| !value.is_empty());
        repo.default_target_dir = default_target_dir.unwrap_or_default();
        config.repos.push(repo.clone());
        Ok(repo)
    })?;
    Ok(repo_info(&repo))
}

#[tauri::command(async)]
pub fn update_repo(
    state: State<AppState>,
    repo_id: String,
    name: Option<String>,
    default_server_id: Option<String>,
    default_target_dir: Option<String>,
) -> Result<RepoInfo> {
    let next_name = name
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());

    let updated = state.store.mutate_config(|config| {
        if let Some(name) = &next_name {
            if config
                .repos
                .iter()
                .any(|item| item.id != repo_id && item.name == *name)
            {
                return Err(CoreError::config(format!("仓库名称已存在: {name}")));
            }
        }

        let repo = config
            .repos
            .iter_mut()
            .find(|repo| repo.id == repo_id)
            .ok_or_else(|| CoreError::not_found(format!("仓库不存在: {repo_id}")))?;

        if let Some(name) = next_name {
            repo.name = name;
        }
        // None 表示“本次不修改”，空串才表示清空，避免重命名等局部更新把默认配置抹掉。
        if let Some(value) = default_server_id {
            let value = value.trim().to_string();
            repo.default_server_id = if value.is_empty() { None } else { Some(value) };
        }
        if let Some(value) = default_target_dir {
            repo.default_target_dir = value.trim().to_string();
        }
        Ok(repo.clone())
    })?;
    Ok(repo_info(&updated))
}

#[tauri::command(async)]
pub fn remove_repo(state: State<AppState>, repo_id: String) -> Result<()> {
    state.store.mutate_config(|config| {
        config.repos.retain(|repo| repo.id != repo_id);
        Ok(())
    })?;
    // 仓库移除后停止文件监听，避免 watcher 继续占用资源。
    if let Ok(mut watchers) = state.watchers.lock() {
        watchers.remove(&repo_id);
    }
    Ok(())
}

#[tauri::command(async)]
pub fn list_branches(
    state: State<AppState>,
    repo_id: String,
    include_remote: bool,
) -> Result<Vec<Branch>> {
    git_for(&state, &repo_id)?.branches(include_remote)
}

#[tauri::command(async)]
pub fn checkout_branch(
    state: State<AppState>,
    repo_id: String,
    branch: String,
) -> Result<String> {
    git_for(&state, &repo_id)?.checkout(&branch)
}

#[tauri::command(async)]
pub fn create_branch(
    state: State<AppState>,
    repo_id: String,
    name: String,
    from: Option<String>,
    checkout: bool,
) -> Result<String> {
    let from = from.filter(|value| !value.trim().is_empty());
    git_for(&state, &repo_id)?.create_branch(&name, from.as_deref(), checkout)
}

#[tauri::command(async)]
pub fn delete_branch(
    state: State<AppState>,
    repo_id: String,
    branch: String,
    force: bool,
) -> Result<String> {
    let git = git_for(&state, &repo_id)?;
    if git.current_branch().unwrap_or_default() == branch {
        return Err(CoreError::git("不能删除当前所在分支，请先切换分支"));
    }
    git.delete_branch(&branch, force)
}

#[tauri::command(async)]
pub fn repo_log(state: State<AppState>, repo_id: String, limit: usize) -> Result<Vec<Commit>> {
    git_for(&state, &repo_id)?.log(if limit == 0 { 50 } else { limit })
}

#[tauri::command(async)]
pub fn commit_graph(
    state: State<AppState>,
    repo_id: String,
    head: Option<String>,
    limit: usize,
) -> Result<Vec<GraphCommit>> {
    let head = head.filter(|value| !value.trim().is_empty());
    git_for(&state, &repo_id)?.commit_graph(head.as_deref(), if limit == 0 { 200 } else { limit })
}

#[tauri::command(async)]
pub fn push_repo(state: State<AppState>, repo_id: String) -> Result<String> {
    git_for(&state, &repo_id)?.push()
}

#[tauri::command(async)]
pub fn repo_status(state: State<AppState>, repo_id: String) -> Result<RepoStatus> {
    git_for(&state, &repo_id)?.status()
}

#[tauri::command(async)]
pub fn file_diff(state: State<AppState>, repo_id: String, path: String) -> Result<String> {
    git_for(&state, &repo_id)?.diff_file(&path)
}

#[tauri::command(async)]
pub fn list_dir(state: State<AppState>, repo_id: String, path: String) -> Result<Vec<FileEntry>> {
    git_for(&state, &repo_id)?.list_dir(&path)
}

#[tauri::command(async)]
pub fn read_repo_file(
    state: State<AppState>,
    repo_id: String,
    path: String,
) -> Result<FileContent> {
    git_for(&state, &repo_id)?.read_file(&path)
}

#[tauri::command(async)]
pub fn write_repo_file(
    state: State<AppState>,
    repo_id: String,
    path: String,
    content: String,
) -> Result<String> {
    git_for(&state, &repo_id)?.write_file(&path, &content)
}

#[tauri::command(async)]
pub fn find_files(state: State<AppState>, repo_id: String, query: String) -> Result<Vec<String>> {
    git_for(&state, &repo_id)?.find_files(&query, 300)
}

#[tauri::command(async)]
pub fn search_content(
    state: State<AppState>,
    repo_id: String,
    query: String,
    case_sensitive: bool,
) -> Result<Vec<SearchHit>> {
    git_for(&state, &repo_id)?.search_content(&query, case_sensitive, 800)
}

#[tauri::command(async)]
pub fn replace_content(
    state: State<AppState>,
    repo_id: String,
    search: String,
    replacement: String,
    paths: Vec<String>,
    case_sensitive: bool,
) -> Result<ReplaceSummary> {
    git_for(&state, &repo_id)?.replace_content(&search, &replacement, &paths, case_sensitive)
}

#[tauri::command(async)]
pub fn commit_changes(state: State<AppState>, repo_id: String, message: String) -> Result<String> {
    git_for(&state, &repo_id)?.commit_all(&message)
}

#[tauri::command(async)]
pub fn reset_hard(state: State<AppState>, repo_id: String, rev: String) -> Result<String> {
    git_for(&state, &repo_id)?.reset_hard(&rev)
}

#[tauri::command(async)]
pub fn fetch_repo(state: State<AppState>, repo_id: String) -> Result<String> {
    git_for(&state, &repo_id)?.fetch()
}

#[tauri::command(async)]
pub fn pull_repo(state: State<AppState>, repo_id: String) -> Result<String> {
    git_for(&state, &repo_id)?.pull()
}

#[tauri::command(async)]
pub fn resolve_rev(state: State<AppState>, repo_id: String, rev: String) -> Result<ResolvedRev> {
    git_for(&state, &repo_id)?.resolve(&rev)
}
