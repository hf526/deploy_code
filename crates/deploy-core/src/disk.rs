//! 本机磁盘余量检查。
//!
//! 备份包都是 GB 级的，而控制机要替**所有**被备份的服务器攒着这些包，
//! 没有这道闸迟早会把系统盘写满 —— 宿主的 docker 一起被拖死，代价远超一次备份失败。
//! 所以控制机上的 agent 在每个备份任务启动前、以及知道远端大小之后（下载前）各查一次。
//!
//! 笔电上**不拦**（闸门由 `Store::agent_mode` 决定，见 `backup.rs` 与 `container/mod.rs` 的调用点）：
//! 同一把尺子量到装系统的 C: 上，会让原本能跑的定时备份升级后启动即失败。
//!
//! 两种「查不到」要分开：平台没有现成的查询接口 → [`check`] 返回 `Ok(None)`，调用方跳过水位检查，
//! 不至于在没有 statvfs 的地方跑不了备份；平台支持但这次调用失败 → 返回 `Err`，任务不启动。
//! 后者不兜底：盘都读不动了还硬写一个 GB 级的包进去，失败方式比拒绝启动难看得多。

use std::path::Path;

use crate::error::{CoreError, Result};

/// 无论盘多大都要留下的最低余量。
const MIN_FREE_BYTES: u64 = 5 * 1024 * 1024 * 1024;
/// 大容量盘按比例留：固定值在 20 TB 盘上等于没设。
const MIN_FREE_RATIO_PERCENT: u64 = 10;

/// 一次检查的结果。
#[derive(Debug, Clone, Copy)]
pub struct Headroom {
    pub total_bytes: u64,
    pub free_bytes: u64,
    /// 本次判定用的水位线（[`MIN_FREE_BYTES`] 与总量比例取较大者）。
    pub floor_bytes: u64,
}

impl Headroom {
    /// 还够不够再放下 `extra_bytes` 的一个包。
    pub fn fits(&self, extra_bytes: u64) -> bool {
        self.free_bytes.saturating_sub(extra_bytes) >= self.floor_bytes
    }

    /// 面向用户的拒绝理由：报出实际余量，用户才知道该清盘还是该改目录。
    pub fn shortage_message(&self, extra_bytes: u64) -> String {
        use crate::util::human_size;
        format!(
            "磁盘余量不足：剩余 {}，水位线 {}（需再放下 {}）。请清理备份目录或调低保留份数",
            human_size(self.free_bytes),
            human_size(self.floor_bytes),
            human_size(extra_bytes)
        )
    }
}

/// 水位线：固定余量与总量比例取较大者。
pub fn floor_bytes(total_bytes: u64) -> u64 {
    MIN_FREE_BYTES.max(total_bytes / 100 * MIN_FREE_RATIO_PERCENT)
}

/// 查询 `dir` 所在文件系统的余量；目录还不存在时用最近的已存在父目录代查。
///
/// 返回 `Ok(None)` 表示当前平台没有现成的查询接口，调用方应跳过水位检查。
pub fn check(dir: &Path) -> Result<Option<Headroom>> {
    let probe = existing_ancestor(dir);
    let Some((total_bytes, free_bytes)) = filesystem_usage(probe)? else {
        return Ok(None);
    };
    Ok(Some(Headroom {
        total_bytes,
        free_bytes,
        floor_bytes: floor_bytes(total_bytes),
    }))
}

/// 预检：能不能再容纳一个约 `extra_bytes` 的包。查不到余量时放行。
pub fn ensure_room(dir: &Path, extra_bytes: u64) -> Result<()> {
    match check(dir)? {
        None => Ok(()),
        Some(headroom) if headroom.fits(extra_bytes) => Ok(()),
        Some(headroom) => Err(CoreError::backup(headroom.shortage_message(extra_bytes))),
    }
}

fn existing_ancestor(path: &Path) -> &Path {
    let mut current = path;
    while !current.exists() {
        match current.parent() {
            Some(parent) => current = parent,
            None => break,
        }
    }
    current
}

#[cfg(unix)]
fn filesystem_usage(path: &Path) -> Result<Option<(u64, u64)>> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let c_path = CString::new(path.as_os_str().as_bytes()).map_err(|_| {
        CoreError::io_path(
            path,
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "路径含 nul 字节"),
        )
    })?;
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c_path.as_ptr(), &mut stat) } != 0 {
        return Err(CoreError::io_path(path, std::io::Error::last_os_error()));
    }
    let block = stat.f_frsize as u64;
    Ok(Some((
        stat.f_blocks as u64 * block,
        stat.f_bavail as u64 * block,
    )))
}

#[cfg(windows)]
fn filesystem_usage(path: &Path) -> Result<Option<(u64, u64)>> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut free_available = 0u64;
    let mut total = 0u64;
    let mut total_free = 0u64;
    let ok = unsafe {
        GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut free_available,
            &mut total,
            &mut total_free,
        )
    };
    if ok == 0 {
        return Err(CoreError::io_path(path, std::io::Error::last_os_error()));
    }
    Ok(Some((total, free_available)))
}

#[cfg(not(any(unix, windows)))]
fn filesystem_usage(_path: &Path) -> Result<Option<(u64, u64)>> {
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GB: u64 = 1024 * 1024 * 1024;

    #[test]
    fn floor_takes_the_larger_of_fixed_and_ratio() {
        // 20 GB 的小盘：10% 只有 2 GB，固定值 5 GB 更严。
        assert_eq!(floor_bytes(20 * GB), MIN_FREE_BYTES);
        // 1 TB 的盘：10% 是 100 GB，按比例走。
        assert_eq!(floor_bytes(1000 * GB), 100 * GB);
    }

    #[test]
    fn expected_size_squeezes_the_headroom() {
        let room = Headroom {
            total_bytes: 100 * GB,
            free_bytes: 14 * GB,
            floor_bytes: MIN_FREE_BYTES,
        };
        assert!(room.fits(9 * GB));
        assert!(!room.fits(10 * GB));
        // 余量不会减法下溢成天文数字。
        assert!(!room.fits(u64::MAX));
    }

    #[test]
    fn check_returns_a_readable_number_on_this_platform() {
        let dir = std::env::temp_dir();
        if let Some(room) = check(&dir).unwrap() {
            assert!(room.total_bytes > 0);
            assert!(room.floor_bytes >= MIN_FREE_BYTES);
        }
    }

    #[test]
    fn missing_directory_falls_back_to_an_existing_ancestor() {
        let missing = std::env::temp_dir().join("deploycode-not-there-42");
        // 目录不存在不该报错：拿父目录代查，结果仍然可读。
        let _ = check(&missing).unwrap();
    }
}
