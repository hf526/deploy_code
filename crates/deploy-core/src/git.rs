use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use flate2::write::GzEncoder;
use flate2::Compression;

use crate::error::{CoreError, Result};
use crate::models::{
    Branch, Commit, FileChange, FileContent, FileEntry, GraphCommit, GraphRef, RepoStatus,
    ReplaceSummary, ResolvedRev, SearchHit,
};
use crate::process::{run, run_timeout, CommandOutput};

/// 记录分隔符（Unit Separator），避免提交信息中的普通字符造成解析歧义。
const SEP: char = '\u{1f}';

/// 网络类 Git 操作（fetch/pull/push）的超时时间。
const NETWORK_TIMEOUT_SECS: u64 = 600;

/// 对某个本地仓库执行 Git 操作。
pub struct Git {
    path: PathBuf,
}

impl Git {
    /// 打开一个已存在的 Git 仓库。
    pub fn open(path: impl Into<PathBuf>) -> Result<Self> {
        let git = Self { path: path.into() };
        if !git.path.is_dir() {
            return Err(CoreError::not_found(format!(
                "目录不存在: {}",
                git.path.display()
            )));
        }
        let out = git.try_run(&["rev-parse", "--is-inside-work-tree"])?;
        if !out.success() || out.stdout.trim() != "true" {
            return Err(CoreError::git(format!(
                "{} 不是有效的 Git 仓库",
                git.path.display()
            )));
        }
        Ok(git)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn base_args(&self, args: &[&str]) -> Vec<String> {
        let mut base = vec![
            "-C".to_string(),
            self.path.to_string_lossy().into_owned(),
            "-c".to_string(),
            "core.quotepath=false".to_string(),
        ];
        base.extend(args.iter().map(|arg| (*arg).to_string()));
        base
    }

    fn try_run(&self, args: &[&str]) -> Result<CommandOutput> {
        run("git", &self.base_args(args), None)
    }

    fn run(&self, args: &[&str]) -> Result<String> {
        let out = self.try_run(args)?;
        if !out.success() {
            return Err(CoreError::git(git_error(args, &out)));
        }
        Ok(out.stdout)
    }

    /// 需要访问网络的 Git 命令（fetch/pull/push），带超时避免永久卡死。
    fn run_network(&self, args: &[&str]) -> Result<CommandOutput> {
        run_timeout(
            "git",
            &self.base_args(args),
            None,
            Duration::from_secs(NETWORK_TIMEOUT_SECS),
        )
    }

    /// 获取当前分支名（分离头指针时为 HEAD）。
    pub fn current_branch(&self) -> Result<String> {
        Ok(self.run(&["rev-parse", "--abbrev-ref", "HEAD"])?.trim().to_string())
    }

    /// 列出本地（可选远程）分支。
    pub fn branches(&self, include_remote: bool) -> Result<Vec<Branch>> {
        let mut refs: Vec<&str> = vec!["refs/heads"];
        if include_remote {
            refs.push("refs/remotes");
        }

        let format = format!(
            "%(refname){SEP}%(refname:short){SEP}%(objectname:short){SEP}%(upstream:short){SEP}\
             %(committerdate:format:%Y-%m-%d %H:%M){SEP}%(contents:subject)"
        );
        let mut args = vec!["for-each-ref"];
        args.extend(refs.iter().copied());
        args.push("--format");
        args.push(&format);
        args.push("--sort=-committerdate");

        let output = self.run(&args)?;
        let current = self.current_branch().unwrap_or_default();

        let mut branches = Vec::new();
        for line in output.lines() {
            let parts: Vec<&str> = line.split(SEP).collect();
            if parts.len() < 6 {
                continue;
            }
            let refname = parts[0];
            let short = parts[1];
            if refname.starts_with("refs/remotes/") && short.ends_with("/HEAD") {
                continue;
            }
            let is_remote = refname.starts_with("refs/remotes/");
            branches.push(Branch {
                name: short.to_string(),
                is_current: !is_remote && short == current,
                is_remote,
                upstream: non_empty(parts[3]),
                last_commit: parts[2].to_string(),
                last_commit_date: parts[4].to_string(),
                last_commit_subject: parts[5].to_string(),
            });
        }
        Ok(branches)
    }

    /// 工作区状态（当前分支、领先/落后提交数、变动文件）。
    pub fn status(&self) -> Result<RepoStatus> {
        let output = self.run(&["status", "--porcelain=v1", "-b", "--untracked-files=normal"])?;

        let mut status = RepoStatus {
            branch: String::new(),
            upstream: None,
            ahead: 0,
            behind: 0,
            changes: Vec::new(),
        };

        for line in output.lines() {
            if let Some(header) = line.strip_prefix("## ") {
                parse_status_header(header, &mut status);
                continue;
            }
            if line.len() < 3 {
                continue;
            }
            let (code, path) = line.split_at(2);
            status.changes.push(FileChange {
                code: code.to_string(),
                status: describe_code(code),
                path: path.trim().to_string(),
            });
        }
        Ok(status)
    }

    /// 提交记录。
    pub fn log(&self, limit: usize) -> Result<Vec<Commit>> {
        let limit_arg = format!("-n{limit}");
        let format = format!("%H{SEP}%h{SEP}%an{SEP}%ad{SEP}%s");
        let pretty_arg = format!("--pretty=format:{format}");
        let out = self.try_run(&[
            "log",
            &limit_arg,
            "--no-color",
            "--date=format:%Y-%m-%d %H:%M",
            &pretty_arg,
        ])?;
        if !out.success() {
            // 空仓库（还没有任何提交）时直接返回空列表
            return Ok(Vec::new());
        }
        let mut commits = Vec::new();
        for line in out.stdout.lines() {
            let parts: Vec<&str> = line.split(SEP).collect();
            if parts.len() < 5 {
                continue;
            }
            commits.push(Commit {
                hash: parts[0].to_string(),
                short: parts[1].to_string(),
                author: parts[2].to_string(),
                date: parts[3].to_string(),
                subject: parts[4].to_string(),
            });
        }
        Ok(commits)
    }

    /// 工作区文件相对当前提交的差异（unified diff）。未跟踪文件按“新增”构造差异。
    pub fn diff_file(&self, path: &str) -> Result<String> {
        let rel = path.trim().replace('\\', "/");
        if rel.is_empty() {
            return Err(CoreError::git("文件路径不能为空"));
        }
        let text = self.run(&["diff", "HEAD", "--", &rel])?.trim_end().to_string();
        if !text.is_empty() {
            return Ok(truncate_diff(text));
        }
        let status = self.run(&["status", "--porcelain", "--", &rel])?;
        if !status.lines().any(|line| line.starts_with("??")) {
            return Ok(String::new());
        }
        let content = std::fs::read_to_string(self.path.join(&rel))
            .map_err(|_| CoreError::git("二进制或无法读取的文件，暂不支持差异预览"))?;
        let added: Vec<String> = content.lines().map(|line| format!("+{line}")).collect();
        let mut diff = format!("diff --git a/{rel} b/{rel}\nnew file\n--- /dev/null\n+++ b/{rel}\n");
        diff.push_str(&added.join("\n"));
        if !content.is_empty() {
            diff.push('\n');
        }
        Ok(truncate_diff(diff))
    }

    /// 提交图数据：包含父提交与引用装饰，供前端渲染车道图。
    /// `ref_name` 为 Some 时只取该分支历史，否则取全部分支（--all）。
    pub fn commit_graph(&self, ref_name: Option<&str>, limit: usize) -> Result<Vec<GraphCommit>> {
        let limit_arg = format!("-n{limit}");
        let format = format!("%H{SEP}%h{SEP}%P{SEP}%an{SEP}%ad{SEP}%s{SEP}%D");
        let pretty_arg = format!("--pretty=format:{format}");
        let mut args: Vec<&str> = vec!["log"];
        match ref_name {
            Some(name) => args.push(name),
            None => args.push("--all"),
        }
        args.extend([
            "--topo-order",
            &limit_arg,
            "--no-color",
            "--date=format:%Y-%m-%d %H:%M",
            &pretty_arg,
        ]);

        let out = self.try_run(&args)?;
        if !out.success() {
            return Ok(Vec::new());
        }

        let head_branch = self.current_branch().unwrap_or_default();
        let remotes = self.remote_names();

        let mut commits = Vec::new();
        for line in out.stdout.lines() {
            let parts: Vec<&str> = line.split(SEP).collect();
            if parts.len() < 7 {
                continue;
            }
            commits.push(GraphCommit {
                hash: parts[0].to_string(),
                short: parts[1].to_string(),
                parents: parts[2].split_whitespace().map(|p| p.to_string()).collect(),
                author: parts[3].to_string(),
                date: parts[4].to_string(),
                subject: parts[5].to_string(),
                refs: parse_decorations(parts[6], &head_branch, &remotes),
            });
        }
        Ok(commits)
    }

    /// 当前分支是否已设置上游。
    pub fn has_upstream(&self) -> bool {
        self.try_run(&["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"])
            .map(|out| out.success() && !out.stdout.trim().is_empty())
            .unwrap_or(false)
    }

    /// 推送当前分支（无上游时自动关联 origin 同名分支）。
    pub fn push(&self) -> Result<String> {
        let branch = self.current_branch()?;
        let out = if self.has_upstream() {
            self.run_network(&["push"])?
        } else {
            self.run_network(&["push", "--set-upstream", "origin", &branch])?
        };
        if !out.success() {
            return Err(CoreError::git(git_error(&["push"], &out)));
        }
        let text = out.combined();
        Ok(if text.trim().is_empty() {
            format!("已推送 {branch}")
        } else {
            text.trim().to_string()
        })
    }

    /// 切换分支。
    pub fn checkout(&self, branch: &str) -> Result<String> {
        let out = self.try_run(&["checkout", branch])?;
        if !out.success() {
            return Err(CoreError::git(git_error(&["checkout", branch], &out)));
        }
        Ok(format!("已切换到分支 {branch}"))
    }

    /// 创建分支，可选基于某个起点并立即切换过去。
    pub fn create_branch(&self, name: &str, from: Option<&str>, checkout: bool) -> Result<String> {
        let command = if checkout { "checkout" } else { "branch" };
        let mut args = vec![command];
        if checkout {
            args.push("-b");
        }
        args.push(name);
        if let Some(from) = from {
            args.push(from);
        }
        let out = self.try_run(&args)?;
        if !out.success() {
            return Err(CoreError::git(git_error(&args, &out)));
        }
        if checkout {
            Ok(format!("已创建并切换到分支 {name}"))
        } else {
            Ok(format!("已创建分支 {name}"))
        }
    }

    /// 删除分支（force = 强制删除未合并分支）。
    pub fn delete_branch(&self, name: &str, force: bool) -> Result<String> {
        let force_arg = if force { "-D" } else { "-d" };
        let out = self.try_run(&["branch", force_arg, name])?;
        if !out.success() {
            return Err(CoreError::git(git_error(&["branch", force_arg, name], &out)));
        }
        Ok(format!("已删除分支 {name}"))
    }

    /// 拉取远端更新（含清理已删除的远端分支）。
    pub fn fetch(&self) -> Result<String> {
        let out = self.run_network(&["fetch", "--all", "--prune"])?;
        if !out.success() {
            return Err(CoreError::git(git_error(&["fetch", "--all", "--prune"], &out)));
        }
        let text = out.combined();
        if text.trim().is_empty() {
            Ok("已是最新状态".to_string())
        } else {
            Ok(text.trim().to_string())
        }
    }

    /// 拉取并合并当前分支的上游。
    pub fn pull(&self) -> Result<String> {
        let out = self.run_network(&["pull"])?;
        if !out.success() {
            return Err(CoreError::git(git_error(&["pull"], &out)));
        }
        let text = out.combined();
        Ok(if text.trim().is_empty() {
            "已是最新状态".to_string()
        } else {
            text.trim().to_string()
        })
    }

    /// 提交全部改动。
    pub fn commit_all(&self, message: &str) -> Result<String> {
        let message = message.trim();
        if message.is_empty() {
            return Err(CoreError::git("提交信息不能为空"));
        }
        let status = self.run(&["status", "--porcelain"])?;
        if status.trim().is_empty() {
            return Err(CoreError::git("没有需要提交的改动"));
        }
        self.run(&["add", "-A"])?;
        let out = self.try_run(&["commit", "-m", message])?;
        if !out.success() {
            return Err(CoreError::git(git_error(&["commit", "-m", message], &out)));
        }
        let short = self.run(&["rev-parse", "--short", "HEAD"])?;
        Ok(format!("已提交 {}: {}", short.trim(), message))
    }

    /// 回退到指定版本（丢弃工作区改动）。
    pub fn reset_hard(&self, rev: &str) -> Result<String> {
        let resolved = self.resolve(rev)?;
        let out = self.try_run(&["reset", "--hard", &resolved.hash])?;
        if !out.success() {
            return Err(CoreError::git(git_error(
                &["reset", "--hard", &resolved.hash],
                &out,
            )));
        }
        Ok(format!(
            "已回退到 {} {}",
            resolved.short, resolved.subject
        ))
    }

    /// 将分支名 / 标签 / 提交号解析为具体提交。
    pub fn resolve(&self, rev: &str) -> Result<ResolvedRev> {
        let rev = rev.trim();
        if rev.is_empty() {
            return Err(CoreError::git("版本号不能为空"));
        }
        let hash = match self.rev_hash(rev)? {
            Some(hash) => hash,
            None => {
                let remote_rev = format!("origin/{rev}");
                self.rev_hash(&remote_rev)?
                    .ok_or_else(|| CoreError::git(format!("找不到版本: {rev}")))? 
                    .to_string()
            }
        };

        let format = format!("%h{SEP}%s{SEP}%an{SEP}%ad");
        let pretty_arg = format!("--pretty=format:{format}");
        let out = self.run(&[
            "log",
            "-1",
            "--no-color",
            "--date=format:%Y-%m-%d %H:%M",
            &pretty_arg,
            &hash,
        ])?;
        let parts: Vec<&str> = out.trim().split(SEP).collect();
        Ok(ResolvedRev {
            rev: rev.to_string(),
            hash,
            short: parts.first().copied().unwrap_or_default().to_string(),
            subject: parts.get(1).copied().unwrap_or_default().to_string(),
            author: parts.get(2).copied().unwrap_or_default().to_string(),
            date: parts.get(3).copied().unwrap_or_default().to_string(),
        })
    }

    fn rev_hash(&self, rev: &str) -> Result<Option<String>> {
        let expr = format!("{rev}^{{commit}}");
        let out = self.try_run(&["rev-parse", "--verify", "--quiet", &expr])?;
        if out.success() {
            let hash = out.stdout.trim().to_string();
            if hash.is_empty() {
                Ok(None)
            } else {
                Ok(Some(hash))
            }
        } else {
            Ok(None)
        }
    }

    /// 将指定提交打包为 tar，并压缩为 tar.gz。
    ///
    /// 使用 `git archive` + Rust 端 gzip，避免依赖本机 gzip 程序。
    /// 返回压缩包大小（字节）。
    pub fn archive(&self, rev: &str, tar_path: &Path, gz_path: &Path) -> Result<u64> {
        let tar_str = tar_path.to_string_lossy().into_owned();
        let output = self.try_run(&["archive", "--format=tar", "-o", &tar_str, rev])?;
        if !output.success() {
            return Err(CoreError::git(git_error(
                &["archive", "--format=tar", "-o", &tar_str, rev],
                &output,
            )));
        }

        let src = File::open(tar_path).map_err(|e| CoreError::io_path(tar_path, e))?;
        let dst = File::create(gz_path).map_err(|e| CoreError::io_path(gz_path, e))?;
        let mut encoder = GzEncoder::new(BufWriter::new(dst), Compression::default());
        let mut reader = std::io::BufReader::new(src);
        let written = std::io::copy(&mut reader, &mut encoder)?;
        encoder.flush()?;
        encoder.finish()?;
        let _ = std::fs::remove_file(tar_path);
        Ok(written)
    }

    /// 远端仓库地址。
    pub fn remote_url(&self) -> Result<Option<String>> {
        let out = self.try_run(&["remote", "get-url", "origin"])?;
        if out.success() {
            Ok(non_empty(out.stdout.trim()))
        } else {
            Ok(None)
        }
    }

    fn remote_names(&self) -> Vec<String> {
        self.try_run(&["remote"])
            .map(|out| {
                out.stdout
                    .lines()
                    .map(|line| line.trim().to_string())
                    .filter(|line| !line.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// 列出仓库内某个目录（相对路径，空串表示根目录）的内容，目录在前、按名称排序。
    pub fn list_dir(&self, rel: &str) -> Result<Vec<FileEntry>> {
        let rel = normalize_rel(rel)?;
        let dir = self.path.join(&rel);
        if !dir.is_dir() {
            return Err(CoreError::not_found(format!("目录不存在: {rel}")));
        }
        let read = std::fs::read_dir(&dir).map_err(|e| CoreError::io_path(&dir, e))?;
        let mut entries = Vec::new();
        for item in read.flatten() {
            let name = item.file_name().to_string_lossy().into_owned();
            if rel.is_empty() && name == ".git" {
                continue;
            }
            let is_dir = item.path().is_dir();
            let size = if is_dir {
                0
            } else {
                item.metadata().map(|meta| meta.len()).unwrap_or(0)
            };
            let path = if rel.is_empty() {
                name.clone()
            } else {
                format!("{rel}/{name}")
            };
            entries.push(FileEntry {
                name,
                path,
                is_dir,
                size,
            });
        }
        entries.sort_by(|a, b| {
            b.is_dir
                .cmp(&a.is_dir)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        Ok(entries)
    }

    /// 读取仓库内文本文件用于预览，超大文件截断，二进制文件返回错误。
    pub fn read_file(&self, rel: &str) -> Result<FileContent> {
        let rel = normalize_rel(rel)?;
        if rel.is_empty() {
            return Err(CoreError::git("文件路径不能为空"));
        }
        const LIMIT: usize = 2_000_000;
        let file = self.path.join(&rel);
        if !file.is_file() {
            return Err(CoreError::not_found(format!("文件不存在: {rel}")));
        }
        let bytes = std::fs::read(&file).map_err(|e| CoreError::io_path(&file, e))?;
        if bytes[..bytes.len().min(8000)].contains(&0) {
            return Err(CoreError::git("二进制文件，暂不支持预览"));
        }
        let truncated = bytes.len() > LIMIT;
        let text = String::from_utf8_lossy(&bytes[..bytes.len().min(LIMIT)]).into_owned();
        Ok(FileContent {
            path: rel,
            content: text,
            truncated,
        })
    }

    /// 保存文本内容到仓库内的文件（简单编辑器写回）。
    pub fn write_file(&self, rel: &str, content: &str) -> Result<String> {
        let rel = normalize_rel(rel)?;
        if rel.is_empty() {
            return Err(CoreError::git("文件路径不能为空"));
        }
        let file = self.path.join(&rel);
        if file.is_dir() {
            return Err(CoreError::git(format!("目标是目录: {rel}")));
        }
        std::fs::write(&file, content.as_bytes()).map_err(|e| CoreError::io_path(&file, e))?;
        Ok(format!("已保存 {rel}"))
    }

    /// 按文件名快速搜索（tracked + 未忽略的 untracked），大小写不敏感子串匹配。
    pub fn find_files(&self, query: &str, limit: usize) -> Result<Vec<String>> {
        let out = self.try_run(&["ls-files", "--cached", "--others", "--exclude-standard"])?;
        if !out.success() {
            return Ok(Vec::new());
        }
        let needle = query.trim().to_lowercase();
        let mut files = Vec::new();
        for line in out.stdout.lines() {
            let path = line.trim();
            if path.is_empty() {
                continue;
            }
            if needle.is_empty() || path.to_lowercase().contains(&needle) {
                files.push(path.to_string());
                if files.len() >= limit {
                    break;
                }
            }
        }
        Ok(files)
    }

    /// 全仓库内容搜索（git grep 固定字符串，含未跟踪文件，跳过二进制）。
    pub fn search_content(
        &self,
        query: &str,
        case_sensitive: bool,
        limit: usize,
    ) -> Result<Vec<SearchHit>> {
        let query = query.trim();
        if query.is_empty() {
            return Ok(Vec::new());
        }
        let limit_arg = format!("-{limit}");
        let mut args: Vec<&str> = vec!["grep", "-n", "-I", "--untracked", &limit_arg];
        if !case_sensitive {
            args.push("-i");
        }
        args.extend(["--fixed-strings", query]);
        let out = self.try_run(&args)?;
        if out.code != 0 && out.code != 1 {
            return Err(CoreError::git(git_error(&args, &out)));
        }
        let mut hits = Vec::new();
        for line in out.stdout.lines() {
            let mut parts = line.splitn(3, ':');
            let (Some(path), Some(num), Some(text)) = (parts.next(), parts.next(), parts.next())
            else {
                continue;
            };
            let Ok(num) = num.parse::<u32>() else {
                continue;
            };
            hits.push(SearchHit {
                path: path.to_string(),
                line: num,
                text: text.trim_start().to_string(),
            });
        }
        Ok(hits)
    }

    /// 在指定文件列表内做字面量批量替换，跳过二进制/非 UTF-8 文件。
    pub fn replace_content(
        &self,
        search: &str,
        replacement: &str,
        paths: &[String],
        case_sensitive: bool,
    ) -> Result<ReplaceSummary> {
        if search.is_empty() {
            return Err(CoreError::git("搜索内容不能为空"));
        }
        let mut files = 0u32;
        let mut matches = 0u32;
        for rel in paths {
            let Ok(rel) = normalize_rel(rel) else {
                continue;
            };
            if rel.is_empty() {
                continue;
            }
            let file = self.path.join(&rel);
            if !file.is_file() {
                continue;
            }
            let bytes = match std::fs::read(&file) {
                Ok(bytes) => bytes,
                Err(_) => continue,
            };
            if bytes.contains(&0) {
                continue;
            }
            let Ok(text) = String::from_utf8(bytes) else {
                continue;
            };
            let (next, count) = replace_all_occurrences(&text, search, replacement, case_sensitive);
            if count > 0 {
                std::fs::write(&file, next.as_bytes()).map_err(|e| CoreError::io_path(&file, e))?;
                files += 1;
                matches += count;
            }
        }
        Ok(ReplaceSummary {
            files_replaced: files,
            matches_replaced: matches,
        })
    }
}

/// 字面量替换全部出现位置，返回新文本与替换次数。
fn replace_all_occurrences(
    text: &str,
    search: &str,
    replacement: &str,
    case_sensitive: bool,
) -> (String, u32) {
    if search.is_empty() {
        return (text.to_string(), 0);
    }
    if case_sensitive {
        let count = text.matches(search).count() as u32;
        return (text.replace(search, replacement), count);
    }

    // 将原文按“小写展开”映射为字符序列，并记录每个小写字符对应原文的字节区间。
    // to_lowercase 可能改变字节长度（如 İ、ẞ），直接复用字节偏移会越界 panic。
    struct LowerUnit {
        byte_start: usize,
        byte_end: usize,
        ch: char,
    }

    let mut units: Vec<LowerUnit> = Vec::with_capacity(text.len());
    for (start, ch) in text.char_indices() {
        let end = start + ch.len_utf8();
        for lower in ch.to_lowercase() {
            units.push(LowerUnit {
                byte_start: start,
                byte_end: end,
                ch: lower,
            });
        }
    }

    let needle: Vec<char> = search.to_lowercase().chars().collect();
    if needle.is_empty() {
        return (text.to_string(), 0);
    }

    let mut result = String::with_capacity(text.len());
    let mut written = 0usize;
    let mut index = 0usize;
    let mut count = 0u32;
    while index + needle.len() <= units.len() {
        let matched = units[index..index + needle.len()]
            .iter()
            .map(|unit| unit.ch)
            .eq(needle.iter().copied());
        if matched {
            let start = units[index].byte_start;
            let end = units[index + needle.len() - 1].byte_end;
            result.push_str(&text[written..start]);
            result.push_str(replacement);
            count += 1;
            index += needle.len();
            written = end;
        } else {
            index += 1;
        }
    }
    result.push_str(&text[written..]);
    (result, count)
}

/// 规范化仓库内相对路径，阻止越界访问。
fn normalize_rel(rel: &str) -> Result<String> {
    let rel = rel.trim().replace('\\', "/");
    if rel.is_empty() || rel == "." {
        return Ok(String::new());
    }
    if rel.starts_with('/') || rel.contains(":") || rel.split('/').any(|seg| seg == "..") {
        return Err(CoreError::git(format!("非法路径: {rel}")));
    }
    Ok(rel.trim_matches('/').to_string())
}

fn non_empty(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

/// 截断过大的差异文本，按 char 边界安全切割。
fn truncate_diff(mut text: String) -> String {
    const LIMIT: usize = 400_000;
    if text.len() > LIMIT {
        let mut cut = LIMIT;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text.truncate(cut);
        text.push_str("\n… 差异内容过大，已截断");
    }
    text
}

/// 解析 git log %D 装饰（如 `HEAD -> main, origin/main, tag: v1.0`）。
fn parse_decorations(decor: &str, head_branch: &str, remotes: &[String]) -> Vec<GraphRef> {
    let mut refs = Vec::new();
    for part in decor.split(", ") {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some(tag) = part.strip_prefix("tag: ") {
            refs.push(GraphRef {
                name: tag.to_string(),
                kind: "tag".to_string(),
                is_head: false,
            });
            continue;
        }
        let (is_head, name) = match part.strip_prefix("HEAD -> ") {
            Some(name) => (true, name),
            None => {
                if part == "HEAD" {
                    continue;
                }
                (part == head_branch, part)
            }
        };
        let prefix = name.split('/').next().unwrap_or("");
        let kind = if remotes.iter().any(|remote| remote == prefix) {
            "remote"
        } else {
            "local"
        };
        refs.push(GraphRef {
            name: name.to_string(),
            kind: kind.to_string(),
            is_head,
        });
    }
    refs
}

fn parse_status_header(header: &str, status: &mut RepoStatus) {
    if let Some((left, right)) = header.split_once("...") {
        status.branch = left.trim().to_string();

        let mut upstream_part = right;
        if let Some(start) = right.find('[') {
            upstream_part = &right[..start];
            // 从 '[' 之后查找 ']'，否则分支名中先出现的 ']' 会导致切片越界 panic。
            if let Some(offset) = right[start..].find(']') {
                let end = start + offset;
                let flags = &right[start + 1..end];
                for flag in flags.split(',') {
                    let flag = flag.trim();
                    if let Some(value) = flag.strip_prefix("ahead ") {
                        status.ahead = value.trim().parse().unwrap_or(0);
                    } else if let Some(value) = flag.strip_prefix("behind ") {
                        status.behind = value.trim().parse().unwrap_or(0);
                    }
                }
            }
        }
        status.upstream = non_empty(upstream_part);
    } else if let Some(branch) = header.strip_prefix("No commits yet on ") {
        status.branch = branch.trim().to_string();
    } else if header.starts_with("HEAD") {
        status.branch = "HEAD".to_string();
    } else {
        status.branch = header.trim().to_string();
    }
}

fn describe_code(code: &str) -> String {
    let trimmed = code.trim();
    match trimmed {
        "??" => "未跟踪".to_string(),
        _ if trimmed.contains('U') => "冲突".to_string(),
        _ if trimmed.contains('R') => "重命名".to_string(),
        _ if trimmed.contains('C') => "复制".to_string(),
        _ if trimmed.contains('D') => "删除".to_string(),
        _ if trimmed.contains('A') => "新增".to_string(),
        _ if trimmed.contains('M') => "修改".to_string(),
        _ => code.trim().to_string(),
    }
}

fn git_error(args: &[&str], out: &CommandOutput) -> String {
    let detail = out.combined();
    let detail = if detail.is_empty() { "无输出" } else { &detail };
    if detail.contains("Please tell me who you are") {
        return "Git 未配置提交身份，请先执行:\n  git config --global user.name \"你的名字\"\n  git config --global user.email \"你的邮箱\"".to_string();
    }
    format!("git {} 失败: {detail}", args.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_status() -> RepoStatus {
        RepoStatus {
            branch: String::new(),
            upstream: None,
            ahead: 0,
            behind: 0,
            changes: Vec::new(),
        }
    }

    #[test]
    fn parse_status_header_handles_bracket_in_branch_name() {
        // 分支名含 ']' 时旧实现会因 `right.find(']')` 从字符串开头查找而切片越界 panic。
        let mut status = empty_status();
        parse_status_header("foo]bar...origin/foo]bar [ahead 1, behind 2]", &mut status);
        assert_eq!(status.branch, "foo]bar");
        assert_eq!(status.upstream.as_deref(), Some("origin/foo]bar"));
        assert_eq!(status.ahead, 1);
        assert_eq!(status.behind, 2);
    }

    #[test]
    fn parse_status_header_parses_counts() {
        let mut status = empty_status();
        parse_status_header("main...origin/main [ahead 3]", &mut status);
        assert_eq!(status.branch, "main");
        assert_eq!(status.upstream.as_deref(), Some("origin/main"));
        assert_eq!(status.ahead, 3);
        assert_eq!(status.behind, 0);
    }

    #[test]
    fn replace_case_insensitive_basic() {
        let (text, count) = replace_all_occurrences("Hello World hello", "hello", "hi", false);
        assert_eq!(count, 2);
        assert_eq!(text, "hi World hi");
    }

    #[test]
    fn replace_case_insensitive_handles_unicode_length_change() {
        // İ 小写后字节数变化，旧实现复用 lower_text 的偏移索引原文会 panic。
        let (text, count) = replace_all_occurrences("İx", "x", "y", false);
        assert_eq!(count, 1);
        assert_eq!(text, "İy");
    }

    #[test]
    fn replace_empty_search_is_noop() {
        let (text, count) = replace_all_occurrences("abc", "", "x", false);
        assert_eq!(count, 0);
        assert_eq!(text, "abc");
    }
}
