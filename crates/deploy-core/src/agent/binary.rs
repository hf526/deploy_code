//! agent 可执行文件的查找、校验与只读展示。

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::error::{CoreError, Result};
use crate::store::Store;

use super::LOCAL_BINARY_SUBDIR;

/// 本机要用的 agent 可执行文件是从哪一档来的。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentBinarySource {
    /// 客户端安装包自带的（`<资源目录>/agent/deploy-agent`），正常安装就是这一档。
    Bundled,
    /// `<数据目录>/agent/` 下手放的那份（开发期换产物用，压过内置）。
    Manual,
    /// 一处都没有：点「安装/更新」会报错。
    Missing,
}

/// 界面那一行「agent 可执行文件」的素材：只读展示，不再让用户敲路径。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentBinaryInfo {
    pub source: AgentBinarySource,
    /// 绝对路径；[`AgentBinarySource::Missing`] 时是空串。
    pub path: String,
    pub size_bytes: u64,
    /// 一份都没有、或放了却不能用时的原因；空串表示可用。
    ///
    /// 不能只报「没内置」：用户在 `<数据目录>/agent/` 手放了一份 `.exe` 时，真实原因是那份不能用的
    /// 文件挡住了内置那份，把路径与原因写出来他才知道该删掉哪个（AGENTS.md：谁在用哪个要直接写出来）。
    #[serde(default)]
    pub error: String,
}

/// 这份文件能不能当 Linux agent 用：能用返回 `None`，不能用返回一句说明原因的话。
///
/// 只查 4 字节魔数是不够的 —— ARM 或 32 位的 ELF 也会通过，装上之后 systemd 起不来，
/// 而那时旧服务已经被 stop 掉了（安装脚本先 stop 再落盘）。x86_64 Linux 的组合是固定的：
/// ELFCLASS64 + 小端 + `e_machine = 0x3e`。
fn binary_problem(path: &Path) -> Option<String> {
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(_) => return Some("文件读不出来（可能被别的进程占用）".to_string()),
    };
    let mut head = [0u8; 20];
    if std::io::Read::read_exact(&mut file, &mut head).is_err() {
        return Some("文件太短，不像可执行文件".to_string());
    }
    if head[..4] != [0x7f, b'E', b'L', b'F'] {
        return Some("文件头不是 ELF（本机编出来的 deploy-agent.exe 或占位文件都是这样）".to_string());
    }
    if head[4] != 2 {
        return Some("这是 32 位 ELF，控制机要 64 位".to_string());
    }
    if head[5] != 1 {
        return Some("这是大端 ELF，控制机要小端（x86_64）".to_string());
    }
    let machine = u16::from_le_bytes([head[18], head[19]]);
    if machine != 0x3e {
        return Some(format!(
            "这是架构 {machine:#04x} 的 ELF，控制机要 x86_64（0x3e）—— 用 --target x86_64-unknown-linux-musl 重新编一份"
        ));
    }
    None
}

/// 一个候选路径的两道检查：在不在、是不是能跑在 x86_64 Linux 上。
fn accept_binary(path: PathBuf, source: AgentBinarySource) -> Result<(PathBuf, AgentBinarySource)> {
    if !path.is_file() {
        return Err(CoreError::config(format!(
            "找不到 agent 可执行文件: {}",
            path.display()
        )));
    }
    if let Some(problem) = binary_problem(&path) {
        return Err(CoreError::config(format!(
            "{} 不能当 agent 用：{problem}。agent 跑在控制机（Linux）上，本机那份即使能双击也传不过去。",
            path.display()
        )));
    }
    Ok((path, source))
}

/// 找 agent 可执行文件：`<数据目录>/agent/` 下手放的那份 → 安装包内置的那份。
///
/// 刻意没有「设置里指一个路径」这一档：那是这台机器上的文件位置，写进配置就会被导出/导入
/// 带到别的机器上，而界面上早就没有输入框了，指错一路都清不掉。开发期要换一份测试，
/// 就把它放到 `<数据目录>/agent/deploy-agent` —— 这一档排在内置之前，否则安装包里的
/// 那份永远压着，本地新编的产物根本传不上去。谁在用哪个，界面的那一行会直接写出来，
/// 所以「手放的旧文件赢了」不是静默降级。
pub fn locate_binary(store: &Store) -> Result<(PathBuf, AgentBinarySource)> {
    let dir = store.base_dir().join(LOCAL_BINARY_SUBDIR);
    for name in ["deploy-agent", "deploy-agent.exe", "deploy-agent-linux"] {
        let path = dir.join(name);
        if path.is_file() {
            return accept_binary(path, AgentBinarySource::Manual);
        }
    }
    if let Some(bundled) = store.bundled_agent_binary() {
        let bundled = bundled.to_path_buf();
        if bundled.is_file() {
            return accept_binary(bundled, AgentBinarySource::Bundled);
        }
    }
    Err(CoreError::config(format!(
        "本机没有可用的 agent 可执行文件：安装包没内置（旧版客户端会这样），{} 下也没有。装最新客户端，或构建一份放进去：\n  cargo build -p deploy-agent --release --target x86_64-unknown-linux-musl\n（产物在 target/x86_64-unknown-linux-musl/release/deploy-agent）",
        dir.display()
    )))
}

/// 界面用的只读视图：找不到不报错，`source` 会是 `missing`、`error` 里写清楚为什么，
/// 让页面自己决定怎么提示。
pub fn binary_info(store: &Store) -> AgentBinaryInfo {
    match locate_binary(store) {
        Ok((path, source)) => AgentBinaryInfo {
            source,
            size_bytes: std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0),
            error: String::new(),
            path: path.display().to_string(),
        },
        Err(err) => AgentBinaryInfo {
            source: AgentBinarySource::Missing,
            path: String::new(),
            size_bytes: 0,
            error: err.to_string(),
        },
    }
}

/// 本机要用的 agent 可执行文件在哪：见 [`locate_binary`]。
pub fn resolve_binary(store: &Store) -> Result<PathBuf> {
    locate_binary(store).map(|(path, _)| path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::testutil::tempfile;

    #[test]
    fn locate_binary_prefers_the_hand_placed_copy_over_the_bundled_one() {
        let dir = tempfile();
        let store = Store::new(&dir);
        // 两处都没有：报错要点明该往哪放、怎么构建，界面则按 missing 画一行提示。
        let err = resolve_binary(&store).unwrap_err().to_string();
        assert!(err.contains("agent"), "{err}");
        assert!(err.contains("deploy-agent"), "{err}");
        assert_eq!(binary_info(&store).source, AgentBinarySource::Missing);

        let bundled = dir.join("bundle").join("deploy-agent");
        write_fake_binary(&bundled);
        store.set_bundled_agent_binary(&bundled);
        assert_eq!(
            locate_binary(&store).unwrap(),
            (bundled.clone(), AgentBinarySource::Bundled)
        );

        // 开发期手放的那份必须压过安装包里的，否则本地新编的产物永远传不上去。
        let manual = dir.join("agent").join("deploy-agent");
        write_fake_binary(&manual);
        assert_eq!(
            locate_binary(&store).unwrap(),
            (manual.clone(), AgentBinarySource::Manual)
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn locate_binary_rejects_files_that_are_not_linux_binaries() {
        let dir = tempfile();
        let store = Store::new(&dir);
        let binary = dir.join("agent").join("deploy-agent");
        std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
        // 本机 cargo build 出来的那份（PE）或随手放的占位文本，都不该被传上服务器。
        let mut pe = x86_64_head();
        pe[..4].copy_from_slice(b"MZ\x90\x03");
        std::fs::write(&binary, pe).unwrap();
        let err = resolve_binary(&store).unwrap_err().to_string();
        assert!(err.contains("ELF"), "{err}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn agent_binary_info_field_names_match_the_typescript_mirror() {
        let dir = tempfile();
        let store = Store::new(&dir);
        let json = serde_json::to_value(binary_info(&store)).unwrap();
        assert_eq!(json["source"], "missing");
        assert_eq!(json["path"], "");
        assert!(json.get("sizeBytes").is_some(), "{json}");
        // 一处都没有时也要把原因写出来，界面那一行直接画这句话。
        assert!(json["error"]
            .as_str()
            .unwrap()
            .contains("本机没有可用的 agent 可执行文件"), "{json}");
        std::fs::remove_dir_all(&dir).ok();
    }

    fn x86_64_head() -> [u8; 20] {
        let mut head = [0u8; 20];
        head[..4].copy_from_slice(&[0x7f, b'E', b'L', b'F']);
        head[4] = 2; // ELFCLASS64
        head[5] = 1; // 小端
        head[18..20].copy_from_slice(&0x3eu16.to_le_bytes()); // EM_X86_64
        head
    }

    /// 只看 4 字节魔数是不够的：ARM / 32 位 / 大端的 ELF 都会通过，装上之后 systemd 起不来，
    /// 而安装脚本那时已经把旧服务 stop 掉了。
    #[test]
    fn binary_problem_rejects_elf_that_cannot_run_on_the_control_machine() {
        let dir = tempfile();
        let path = dir.join("deploy-agent");
        let check = |head: [u8; 20]| -> Option<String> {
            std::fs::write(&path, head).unwrap();
            binary_problem(&path)
        };

        assert_eq!(check(x86_64_head()), None);

        let mut arm = x86_64_head();
        arm[18..20].copy_from_slice(&0xb7u16.to_le_bytes()); // EM_AARCH64
        assert!(check(arm).unwrap().contains("架构"));

        let mut ilp32 = x86_64_head();
        ilp32[4] = 1; // ELFCLASS32
        assert!(check(ilp32).unwrap().contains("32 位"));

        let mut be = x86_64_head();
        be[5] = 2; // 大端
        assert!(check(be).unwrap().contains("大端"));

        let mut not_elf = x86_64_head();
        not_elf[..4].copy_from_slice(b"MZ\x90\x00"); // PE 头
        assert!(check(not_elf).unwrap().contains("ELF"));

        std::fs::remove_dir_all(&dir).ok();
    }

    /// 写一份能过 ELF 检查的假二进制（够 20 字节，`e_machine` 是 x86_64）。
    fn write_fake_binary(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, x86_64_head()).unwrap();
    }
}
