use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;

use crate::error::{CoreError, Result};
use crate::models::{FileContent, FileEntry, ReplaceSummary, SearchHit};

use super::{git_error, normalize_rel, replace_all_occurrences, Git};

impl Git {
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
        // 仓库里可能存在指向仓库外的符号链接，解析真实路径后必须仍在仓库内。
        let root =
            std::fs::canonicalize(&self.path).map_err(|e| CoreError::io_path(&self.path, e))?;
        let canonical = std::fs::canonicalize(&file).map_err(|e| CoreError::io_path(&file, e))?;
        if !canonical.starts_with(&root) {
            return Err(CoreError::git(format!(
                "文件路径越界（可能是指向仓库外的符号链接）: {rel}"
            )));
        }
        let mut handle = File::open(&file).map_err(|e| CoreError::io_path(&file, e))?;
        // 只读取上限长度，避免超大文件（如 GB 级日志）整个读进内存导致 OOM。
        let size = handle
            .metadata()
            .map_err(|e| CoreError::io_path(&file, e))?
            .len();
        let mut bytes: Vec<u8> = Vec::with_capacity(size.min(LIMIT as u64) as usize);
        (&mut handle)
            .take(LIMIT as u64)
            .read_to_end(&mut bytes)
            .map_err(|e| CoreError::io_path(&file, e))?;
        if bytes[..bytes.len().min(8000)].contains(&0) {
            return Err(CoreError::git("二进制文件，暂不支持预览"));
        }
        let truncated = size > LIMIT as u64;
        let text = String::from_utf8_lossy(&bytes).into_owned();
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
        let root =
            std::fs::canonicalize(&self.path).map_err(|e| CoreError::io_path(&self.path, e))?;
        // 已存在的文件解析符号链接后必须仍在仓库内；新文件则要求其父目录
        // （解析符号链接后）位于仓库内，避免经由链接目录写到仓库外。
        match std::fs::symlink_metadata(&file) {
            Ok(_) => {
                let canonical =
                    std::fs::canonicalize(&file).map_err(|e| CoreError::io_path(&file, e))?;
                if !canonical.starts_with(&root) {
                    return Err(CoreError::git(format!(
                        "文件路径越界（可能是指向仓库外的符号链接）: {rel}"
                    )));
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                let parent = file
                    .parent()
                    .ok_or_else(|| CoreError::git(format!("非法路径: {rel}")))?;
                let canonical_parent =
                    std::fs::canonicalize(parent).map_err(|e| CoreError::io_path(parent, e))?;
                if !canonical_parent.starts_with(&root) {
                    return Err(CoreError::git(format!(
                        "文件路径越界（父目录指向仓库外）: {rel}"
                    )));
                }
            }
            Err(err) => return Err(CoreError::io_path(&file, err)),
        }
        // 普通文件走「临时文件 + rename」原子写，避免写入中途失败截断原文件；
        // 指向仓库内其他文件的符号链接保持写穿语义，不做替换。
        let is_symlink = std::fs::symlink_metadata(&file)
            .map(|meta| meta.file_type().is_symlink())
            .unwrap_or(false);
        if is_symlink {
            std::fs::write(&file, content.as_bytes()).map_err(|e| CoreError::io_path(&file, e))?;
        } else {
            atomic_write_text(&file, content)?;
        }
        Ok(format!("已保存 {rel}"))
    }

    /// 按文件名快速搜索（tracked + 未忽略的 untracked），大小写不敏感子串匹配。
    /// 非 Git 文件夹退化为遍历本地目录。
    pub fn find_files(&self, query: &str, limit: usize) -> Result<Vec<String>> {
        if !self.is_repo() {
            let needle = query.trim().to_lowercase();
            let mut files = Vec::new();
            walk_files(&self.path, |rel, _| {
                if needle.is_empty() || rel.to_lowercase().contains(&needle) {
                    files.push(rel.to_string());
                    files.len() >= limit
                } else {
                    false
                }
            });
            files.sort();
            return Ok(files);
        }
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
    /// 非 Git 文件夹退化为遍历本地目录逐行匹配。
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
        if !self.is_repo() {
            return self.search_content_fs(query, case_sensitive, limit);
        }
        // git grep 的 `-<n>` 是上下文行数（-C <n>）而非条数上限，必须用 --max-count；
        // 且 --max-count 是「每个文件」上限，解析时再按总量截断。
        let max_arg = format!("--max-count={limit}");
        let mut args: Vec<&str> = vec!["grep", "-n", "-I", "--untracked", &max_arg];
        if !case_sensitive {
            args.push("-i");
        }
        // 用 -e 显式标记 pattern，避免以 `-` 开头的搜索词被 git 当作选项。
        args.extend(["--fixed-strings", "-e", query]);
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
            if hits.len() >= limit {
                break;
            }
        }
        Ok(hits)
    }

    /// 非 Git 文件夹的全文搜索：遍历目录逐行匹配，跳过二进制与超大文件。
    fn search_content_fs(
        &self,
        query: &str,
        case_sensitive: bool,
        limit: usize,
    ) -> Result<Vec<SearchHit>> {
        /// 单个文件最多读取的字节数，与文件预览的上限保持一致。
        const MAX_FILE_SIZE: u64 = 2_000_000;
        let needle = if case_sensitive {
            query.to_string()
        } else {
            query.to_lowercase()
        };
        let mut hits = Vec::new();
        walk_files(&self.path, |rel, path| {
            let Ok(meta) = std::fs::metadata(path) else {
                return false;
            };
            if !meta.is_file() || meta.len() > MAX_FILE_SIZE {
                return false;
            }
            let Ok(bytes) = std::fs::read(path) else {
                return false;
            };
            if bytes[..bytes.len().min(8000)].contains(&0) {
                return false;
            }
            let text = String::from_utf8_lossy(&bytes);
            for (index, line) in text.lines().enumerate() {
                let matched = if case_sensitive {
                    line.contains(&needle)
                } else {
                    line.to_lowercase().contains(&needle)
                };
                if !matched {
                    continue;
                }
                hits.push(SearchHit {
                    path: rel.to_string(),
                    line: (index + 1) as u32,
                    text: line.trim_start().to_string(),
                });
                if hits.len() >= limit {
                    return true;
                }
            }
            false
        });
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
        let root =
            std::fs::canonicalize(&self.path).map_err(|e| CoreError::io_path(&self.path, e))?;
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
            // 与 write_file 一致：解析符号链接后必须仍在仓库内，避免经链接目录写到仓库外。
            let Ok(canonical) = std::fs::canonicalize(&file) else {
                continue;
            };
            if !canonical.starts_with(&root) {
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
                atomic_write_text(&file, &next)?;
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


/// 非 Git 文件夹的文件遍历：跳过 `.git`、`node_modules` 与符号链接（避免环路），
/// 回调收到相对路径与绝对路径；回调返回 true 表示停止遍历。
fn walk_files(root: &Path, mut visit: impl FnMut(&str, &Path) -> bool) {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(read) = std::fs::read_dir(&dir) else {
            continue;
        };
        for item in read.flatten() {
            let Ok(file_type) = item.file_type() else {
                continue;
            };
            let name = item.file_name().to_string_lossy().into_owned();
            if name == ".git" || name == "node_modules" || file_type.is_symlink() {
                continue;
            }
            let path = item.path();
            if file_type.is_dir() {
                stack.push(path);
                continue;
            }
            let rel = path
                .strip_prefix(root)
                .map(|value| value.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();
            if rel.is_empty() {
                continue;
            }
            if visit(&rel, &path) {
                return;
            }
        }
    }
}

/// 原子写回替换后的文本：同目录临时文件 + 刷盘 + rename，避免中途失败截断源文件。
fn atomic_write_text(path: &Path, contents: &str) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("file");
    let tmp = parent.join(format!(".{file_name}.{}.tmp", uuid::Uuid::new_v4()));

    let write = (|| -> Result<()> {
        let mut file = File::create(&tmp).map_err(|e| CoreError::io_path(&tmp, e))?;
        file.write_all(contents.as_bytes())
            .map_err(|e| CoreError::io_path(&tmp, e))?;
        file.sync_all().map_err(|e| CoreError::io_path(&tmp, e))?;
        Ok(())
    })();
    if let Err(err) = write {
        let _ = std::fs::remove_file(&tmp);
        return Err(err);
    }

    if let Ok(meta) = std::fs::metadata(path) {
        let _ = std::fs::set_permissions(&tmp, meta.permissions());
    }
    if let Err(err) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(CoreError::io_path(path, err));
    }
    Ok(())
}
