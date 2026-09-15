use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use flate2::write::GzEncoder;
use flate2::Compression;

use crate::error::{CoreError, Result};

use super::{git_error, Git};

impl Git {
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
        compress_tar(tar_path, gz_path)
    }

    /// 将当前工作区（含未提交改动与未跟踪文件）打包为 tar.gz，返回压缩包大小（字节）。
    ///
    /// 借助临时索引执行 `git add -A` + `git write-tree` 生成工作区快照，再交给
    /// `git archive` 打包：不会改动真实暂存区，未跟踪文件遵循 `.gitignore` 规则。
    pub fn archive_worktree(&self, tar_path: &Path, gz_path: &Path) -> Result<u64> {
        let index_path = tar_path.with_extension("worktree-index");
        let _ = std::fs::remove_file(&index_path);
        let envs = vec![(
            "GIT_INDEX_FILE".to_string(),
            index_path.to_string_lossy().into_owned(),
        )];
        let result = (|| -> Result<u64> {
            let add = self.try_run_env(&["add", "-A"], &envs)?;
            if !add.success() {
                return Err(CoreError::git(git_error(&["add", "-A"], &add)));
            }
            let tree = self.run_env(&["write-tree"], &envs)?;
            let tree = tree.trim().to_string();
            let tar_str = tar_path.to_string_lossy().into_owned();
            let output = self.try_run(&["archive", "--format=tar", "-o", &tar_str, &tree])?;
            if !output.success() {
                return Err(CoreError::git(git_error(
                    &["archive", "--format=tar", "-o", &tar_str, &tree],
                    &output,
                )));
            }
            compress_tar(tar_path, gz_path)
        })();
        // 临时索引只是打包用的中间产物，成功失败都要清理。
        let _ = std::fs::remove_file(&index_path);
        result
    }
}


/// 将 tar 压缩为 tar.gz，返回压缩包大小（字节），并删除原始 tar。
fn compress_tar(tar_path: &Path, gz_path: &Path) -> Result<u64> {
    let src = File::open(tar_path).map_err(|e| CoreError::io_path(tar_path, e))?;
    let dst = File::create(gz_path).map_err(|e| CoreError::io_path(gz_path, e))?;
    let mut encoder = GzEncoder::new(BufWriter::new(dst), Compression::default());
    let mut reader = std::io::BufReader::new(src);
    std::io::copy(&mut reader, &mut encoder)?;
    encoder.flush()?;
    // finish() 只把 gzip 尾部写进 BufWriter，必须显式 flush/sync，否则最终写盘错误会被 drop 静默吞掉。
    let mut writer = encoder.finish()?;
    writer.flush().map_err(|e| CoreError::io_path(gz_path, e))?;
    writer
        .into_inner()
        .map_err(|e| CoreError::io_path(gz_path, e.into_error()))?
        .sync_all()
        .map_err(|e| CoreError::io_path(gz_path, e))?;
    let _ = std::fs::remove_file(tar_path);
    // 返回实际压缩包大小（io::copy 返回的是未压缩的 tar 字节数）。
    let size = std::fs::metadata(gz_path)
        .map_err(|e| CoreError::io_path(gz_path, e))?
        .len();
    Ok(size)
}
