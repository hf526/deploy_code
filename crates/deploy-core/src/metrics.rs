//! 服务器主机的资源占用采集（CPU / 内存）。
//!
//! 只读 `/proc`，跑的是一个固定的短脚本，因此既不占任务名额也不发事件：
//! 它和 `list_compose_stacks` 那类扫描命令同一条口径，任务正在跑时照样能看。

use serde::{Deserialize, Serialize};
use tokio::task::JoinSet;

use crate::error::{CoreError, Result};
use crate::models::ServerConfig;
use crate::ssh::SshClient;

/// 脚本里两次读 `/proc/stat` 之间的间隔秒数；这段睡眠就是采样的窗口。
const SAMPLE_SECS: &str = "1";
/// 采集命令的整体超时：正常一秒钟出头就结束，留足余量只为慢机器，不为长任务兜底。
const EXEC_TIMEOUT_SECS: u64 = 30;

/// 一台主机在采集那一刻的资源占用。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostMetrics {
    /// 整机 CPU 占用百分比（0-100，所有核心合起来算，不是单核）。
    pub cpu_percent: u32,
    pub cpu_cores: usize,
    pub memory_total_bytes: u64,
    pub memory_used_bytes: u64,
}

/// 逐台的采集结果：连不上的那台不影响其余，错误随这一条返回，界面按台显示。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerMetrics {
    pub server_id: String,
    pub metrics: Option<HostMetrics>,
    pub error: Option<String>,
}

/// 采集脚本输出的是「键 值」行，解析端按键取值、认不出的行忽略。
///
/// CPU 必须取两次读数之差：`/proc/stat` 第一行是开机以来的累计时间片，单次读数算出来的是
/// 整机平均而不是「此刻」的占用。内存按 `MemTotal - MemAvailable` 算，这才是用户直觉里的
/// 「用了多少」——按 `MemFree` 算会把 page cache 全算成空闲。
fn collect_script() -> String {
    format!(
        r#"R1=$(head -n1 /proc/stat)
sleep {SAMPLE_SECS}
R2=$(head -n1 /proc/stat)
set -- $R1
U1=${{2:-0}}; N1=${{3:-0}}; S1=${{4:-0}}; I1=${{5:-0}}; W1=${{6:-0}}; Q1=${{7:-0}}; F1=${{8:-0}}; T1=${{9:-0}}
set -- $R2
U2=${{2:-0}}; N2=${{3:-0}}; S2=${{4:-0}}; I2=${{5:-0}}; W2=${{6:-0}}; Q2=${{7:-0}}; F2=${{8:-0}}; T2=${{9:-0}}
BUSY=$(( (U2+N2+S2+Q2+F2+T2) - (U1+N1+S1+Q1+F1+T1) ))
ALL=$(( (U2+N2+S2+I2+W2+Q2+F2+T2) - (U1+N1+S1+I1+W1+Q1+F1+T1) ))
if [ "$ALL" -gt 0 ]; then echo "CPU $(( BUSY * 100 / ALL ))"; fi
CORES=$(grep -c '^processor' /proc/cpuinfo 2>/dev/null || true)
echo "CORES ${{CORES:-0}}"
mem_kb() {{
  sed -n "s/^$1: *\([0-9][0-9]*\) kB.*/\1/p" /proc/meminfo
}}
TOTAL=$(mem_kb MemTotal)
AVAIL=$(mem_kb MemAvailable)
FREE=$(mem_kb MemFree)
if [ -z "$AVAIL" ] && [ -n "$FREE" ]; then
  # MemAvailable 要 Linux 3.14 以后才有，老机器退回「空闲 + cache 里能还回来的那部分」。
  BUF=$(mem_kb Buffers)
  CACHED=$(mem_kb Cached)
  AVAIL=$(( FREE + ${{BUF:-0}} + ${{CACHED:-0}} ))
fi
echo "MEM_TOTAL ${{TOTAL:-0}}"
# 可用量一个数都没读到时宁可不印这一行：解析端会因此报错，也好过拿 0 当可用量、
# 算出一条「内存用满了」的假数字（界面上看不出区别）。
if [ -n "$TOTAL" ] && [ -n "$AVAIL" ]; then echo "MEM_USED $(( TOTAL - AVAIL ))"; fi
echo DONE"#
    )
}

/// 连接服务器并采集一次资源占用。
pub async fn collect(server: &ServerConfig, connect_timeout_secs: u64) -> Result<HostMetrics> {
    let client = SshClient::connect(server, connect_timeout_secs).await?;
    let result = client.exec_capture(&collect_script(), EXEC_TIMEOUT_SECS).await;
    client.disconnect().await;
    let (code, output) = result?;
    // 脚本最后一步是 echo DONE，非 0 说明中途就断了（读不到 /proc、没有 sh）：
    // 把残缺输出当成功会显示出一个假数字，不如报错。
    if code != 0 {
        return Err(CoreError::ssh(format!(
            "资源采集失败（退出码 {code}）：{}",
            failure_reason(&output)
        )));
    }
    parse_metrics(&output)
}

/// 逐台并发采集：一台连不上只影响它自己那一行，不阻塞其余服务器。
///
/// 并发度不额外设限——服务器数量是个位数，而这里每次只是一条一秒钟的只读命令。
pub async fn collect_all(
    servers: &[ServerConfig],
    connect_timeout_secs: u64,
) -> Vec<ServerMetrics> {
    let mut tasks = JoinSet::new();
    for server in servers {
        let server = server.clone();
        tasks.spawn(async move {
            let item = match collect(&server, connect_timeout_secs).await {
                Ok(metrics) => ServerMetrics {
                    server_id: server.id.clone(),
                    metrics: Some(metrics),
                    error: None,
                },
                Err(err) => ServerMetrics {
                    server_id: server.id.clone(),
                    metrics: None,
                    error: Some(err.to_string()),
                },
            };
            item
        });
    }
    let mut results = Vec::with_capacity(servers.len());
    while let Some(joined) = tasks.join_next().await {
        // 任务 panic 只会丢掉这一台；界面对没回来的行显示「未取到」而不是旧数字。
        if let Ok(item) = joined {
            results.push(item);
        }
    }
    results
}

fn parse_metrics(output: &str) -> Result<HostMetrics> {
    let mut cpu = None;
    let mut cores = None;
    let mut total_kb = None;
    let mut used_kb = None;
    let mut done = false;
    for line in output.lines() {
        let line = line.trim();
        let line = line.strip_prefix("[stderr] ").unwrap_or(line).trim();
        let mut parts = line.split_whitespace();
        let (Some(key), value) = (parts.next(), parts.next().unwrap_or("")) else {
            continue;
        };
        match key {
            "CPU" => cpu = value.parse::<u32>().ok(),
            "CORES" => cores = value.parse::<usize>().ok(),
            "MEM_TOTAL" => total_kb = value.parse::<u64>().ok(),
            "MEM_USED" => used_kb = value.parse::<u64>().ok(),
            "DONE" => done = true,
            _ => {}
        }
    }
    if !done {
        return Err(CoreError::ssh("资源采集脚本没有跑完（/proc 不可读或远端没有 sh）"));
    }
    let (Some(cpu_percent), Some(cpu_cores), Some(total_kb), Some(used_kb)) =
        (cpu, cores, total_kb, used_kb)
    else {
        return Err(CoreError::ssh("没有读到主机的 CPU / 内存信息（/proc/stat 或 /proc/meminfo 不可用）"));
    };
    if total_kb == 0 {
        return Err(CoreError::ssh("内存总量读出来是 0，这一台的 /proc/meminfo 不可信"));
    }
    Ok(HostMetrics {
        cpu_percent: cpu_percent.min(100),
        cpu_cores,
        memory_total_bytes: total_kb.saturating_mul(1024),
        // used 由差值算出，理论上不会越界；真越界了按总量封顶，免得界面显示「用 20G / 共 8G」。
        memory_used_bytes: used_kb.min(total_kb).saturating_mul(1024),
    })
}

/// 从采集输出里挑一条能说明原因的句子（stderr 行优先）。
fn failure_reason(output: &str) -> String {
    let lines: Vec<&str> = output
        .lines()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty())
        .collect();
    match lines.iter().find_map(|line| line.strip_prefix("[stderr] ")) {
        Some(stderr) => stderr.trim().to_string(),
        // 没有 stderr 就说「没有错误输出」：把最后一条 stdout（可能是半截的 `CPU 12`）
        // 当成失败原因，用户读到的是一个数字而不是一句解释。
        None => "无错误输出".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> String {
        "CPU 37\nCORES 4\nMEM_TOTAL 8192000\nMEM_USED 2048000\nDONE\n".to_string()
    }

    #[test]
    fn parses_a_full_sample() {
        let metrics = parse_metrics(&sample()).expect("解析应通过");
        assert_eq!(metrics.cpu_percent, 37);
        assert_eq!(metrics.cpu_cores, 4);
        assert_eq!(metrics.memory_total_bytes, 8192000 * 1024);
        assert_eq!(metrics.memory_used_bytes, 2048000 * 1024);
    }

    #[test]
    fn ignores_unrelated_and_stderr_lines() {
        let output = "[stderr] motd: banner\nCPU 12\nCORES 2\nMEM_TOTAL 100\nMEM_USED 40\nDONE\n";
        let metrics = parse_metrics(output).expect("杂音不影响解析");
        assert_eq!(metrics.cpu_percent, 12);
        assert_eq!(metrics.memory_used_bytes, 40 * 1024);
    }

    #[test]
    fn clamps_cpu_and_memory_to_sane_ranges() {
        let output = "CPU 900\nCORES 4\nMEM_TOTAL 100\nMEM_USED 900\nDONE\n";
        let metrics = parse_metrics(output).expect("越界要封顶而不是报错");
        assert_eq!(metrics.cpu_percent, 100);
        assert_eq!(metrics.memory_used_bytes, 100 * 1024);
    }

    #[test]
    fn missing_done_is_a_failure() {
        // 半截输出（脚本被掐断）绝不显示成一个刚好「看起来正常」的数字。
        let output = "CPU 12\nCORES 4\nMEM_TOTAL 100\nMEM_USED 40\n";
        assert!(parse_metrics(output).is_err());
    }

    #[test]
    fn missing_memory_section_is_a_failure() {
        let output = "CPU 12\nCORES 4\nDONE\n";
        assert!(parse_metrics(output).is_err());
        assert!(parse_metrics("CPU 1\nCORES 1\nMEM_TOTAL 0\nMEM_USED 0\nDONE\n").is_err());
    }

    #[test]
    fn missing_cpu_line_is_a_failure() {
        // 采样窗口没推进时脚本不印 CPU：这一台该显示「未取到」，而不是一个看着像空闲的 0%。
        let output = "CORES 4\nMEM_TOTAL 100\nMEM_USED 40\nDONE\n";
        assert!(parse_metrics(output).is_err());
    }

    #[test]
    fn failure_reason_prefers_stderr() {
        assert_eq!(
            failure_reason("CPU 0\n[stderr] /proc/stat: No such file\n"),
            "/proc/stat: No such file"
        );
        // 只剩 stdout 的半截数据时不拿它当原因：那是个看着像指标的数。
        assert_eq!(failure_reason("CPU 0\nlast line\n"), "无错误输出");
        assert_eq!(failure_reason(""), "无错误输出");
    }

    #[test]
    fn script_uses_two_proc_reads_and_a_diff() {
        let script = collect_script();
        assert_eq!(script.matches("head -n1 /proc/stat").count(), 2);
        assert!(script.contains("sleep 1"));
        assert!(script.contains("BUSY * 100 / ALL"));
        // 采样窗口没推进时不印 CPU 这一行（少一个「0% 空闲」的假数字）。
        assert!(!script.contains(r#"else echo "CPU 0""#));
        assert!(script.contains("mem_kb MemAvailable"));
        assert!(script.trim_end().ends_with("echo DONE"));
        // 所有字段都有默认值：/proc 读不到时是空串，不能让算术表达式带着空值语法报错后静默出数。
        assert!(script.contains("${2:-0}"));
        // 回退分支的每一项也要包默认值（本机实测：MemFree 有、Buffers/Cached 没有的机器上，
        // 裸的 $(mem_kb Buffers) 会展开成空串并让整个表达式语法报错）。
        assert!(script.contains("${BUF:-0}"));
        assert!(script.contains("${CACHED:-0}"));
        // 读不到可用量时这一行干脆不印，由解析端报错——拿 0 兜底会显示成「内存用满」。
        assert!(script.contains(r#"if [ -n "$TOTAL" ] && [ -n "$AVAIL" ]"#));
    }
}
