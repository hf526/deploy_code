//! 服务器主机的资源占用采集（CPU / 内存）。
//!
//! 只读 `/proc`，跑的是一个固定的短脚本，既不占任务名额也不发事件：它和
//! `list_compose_stacks` 那类扫描命令同一条口径，任务正在跑时照样能看。
//!
//! 采集只负责「在一次已有会话上取一个数」，什么时候采、连不上怎么办全在
//! [`crate::hostlink`] 那一侧——会话是常驻的，这里不再自己建连接。

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};
use crate::ssh::{OutputKind, SshClient};

/// 脚本里两次读 `/proc/stat` 之间的间隔秒数；这段睡眠就是采样的窗口。
///
/// 常驻循环按它安排下一次采集（间隔必须比窗口大），所以这一项是公开的。
pub const SAMPLE_WINDOW_SECS: u64 = 1;
/// 一次采集的预算。**由调用方掐表**（`hostlink::probe_loop`），不是这里内部用。
pub const SAMPLE_TIMEOUT_SECS: u64 = 15;
/// `exec` 自己的兜底上限，刻意是外层预算的两倍：真正掐表的是调用方，必须让外层先响，
/// 因为「外层到点」这一路会换掉整条会话，把被掐断的通道一起带走。两边同值或反过来的话，
/// 谁先响就交给调度了，而那正是这条要避开的分叉。
pub const SAMPLE_CEILING_SECS: u64 = SAMPLE_TIMEOUT_SECS * 2;
/// 采集输出的上限：正常就是 5 行短数字。会话没有校验主机密钥，对端回多少不由它定；
/// 这一路又每 5 秒自动跑一次，所以不能像一次性采集那样「点了才读一次」。
/// 到上限只是不再接新行，已经收下的照原样交给解析判定。
const SAMPLE_MAX_BYTES: usize = 64 * 1024;

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

/// 采集脚本输出的是「键 值」行，解析端按键取值、认不出的行忽略。
///
/// CPU 必须取两次读数之差：`/proc/stat` 第一行是开机以来的累计时间片，单次读数算出来的是
/// 整机平均而不是「此刻」的占用。内存按 `MemTotal - MemAvailable` 算，这才是用户直觉里的
/// 「用了多少」——按 `MemFree` 算会把 page cache 全算成空闲。
fn collect_script() -> String {
    format!(
        r#"R1=$(head -n1 /proc/stat)
sleep {SAMPLE_WINDOW_SECS}
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

/// 在一条已经建好的会话上采一次占用：只开一条通道，不再新建 SSH 连接。
///
/// 采集是只读的、可重复的，失败本身不代表会话没了（会话是否可用由 `is_closed()` 判），
/// 所以调用方按「这一台暂时没有数字 + 原因」处理，不要把一次采集失败升级成断线重连。
/// **例外是超时**：那一路必须由调用方掐表并换掉整条会话，理由见 [`SAMPLE_CEILING_SECS`]。
pub async fn sample(client: &SshClient) -> Result<HostMetrics> {
    let mut output = String::new();
    let mut sink = |kind: OutputKind, line: String| push_line(&mut output, kind, &line);
    let code = client
        .exec_stream(&collect_script(), SAMPLE_CEILING_SECS, &mut sink)
        .await?;
    drop(sink);
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

/// 把一行接进缓冲区，与 `exec_capture` 同口径（stderr 行加前缀，解析端靠它区分）。
///
/// 到上限之后一行都不再接，但**已经接进去的留着**：这一路要挡的是「对端写多少我们就
/// 得留多少」，不是要把输出处处作废。解析认的是五行齐全，五行在被灌满之前就到了的话
/// 那本来就是真读数；被灌满之后才到齐的那次会自然报「没有跑完」。
fn push_line(output: &mut String, kind: OutputKind, line: &str) {
    let prefix = if kind == OutputKind::Stderr {
        "[stderr] "
    } else {
        ""
    };
    // 前缀与换行一起算：上限要的是「缓冲区从头到尾不超过这么多」，
    // 只比正文的话每次都还能多漏出几个字节。
    if output.len() + prefix.len() + line.len() + 1 > SAMPLE_MAX_BYTES {
        return;
    }
    output.push_str(prefix);
    output.push_str(line);
    output.push('\n');
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

    /// 到上限就停止接收新行。这一路防的是「对端写多少我们就得留多少」，
    /// 而会话没有校验主机密钥、采集又每 5 秒自动跑一次。
    #[test]
    fn the_sink_stops_at_the_byte_cap() {
        let mut output = String::new();

        // 收到接近上限后，再把总量顶过线的那一行不接，缓冲区一个字都不许变。
        let big = "y".repeat(SAMPLE_MAX_BYTES - 8);
        push_line(&mut output, OutputKind::Stdout, &big);
        assert_eq!(output.len(), big.len() + 1, "第一行还装得下");
        push_line(&mut output, OutputKind::Stdout, &big);
        assert_eq!(output.len(), big.len() + 1, "越界的行不许改变缓冲区");

        // 上限是对总量的逐行判断，不是「一越线就永久关闭」：还没顶满时小的正常行照收。
        push_line(&mut output, OutputKind::Stdout, "DONE");
        assert_eq!(output.len(), big.len() + 6);

        // 被垃圾顶住的那一次凑不齐五行，由解析报错，不会变成一个假读数。
        assert!(parse_metrics(&output).is_err());
    }

    /// 远端一直不写换行时，`ssh.rs::drain_lines_bytes` 会按 1 MiB 一块吐出来（它自带
    /// `MAX_PENDING` 上限）。那种块必须整个进不来：否则上限就只是「攒到 1 MiB 再说」。
    #[test]
    fn an_oversized_chunk_never_enters() {
        let mut output = String::new();
        push_line(&mut output, OutputKind::Stdout, &"z".repeat(1024 * 1024));
        assert!(output.is_empty(), "一块 1 MiB 的洪水不许留下任何东西");

        // 前缀与换行一起算，所以正好等于上限的那一行也进不来。
        push_line(&mut output, OutputKind::Stderr, &"z".repeat(SAMPLE_MAX_BYTES));
        assert!(output.is_empty());
        for _ in 0..64 {
            push_line(
                &mut output,
                OutputKind::Stdout,
                &"w".repeat(SAMPLE_MAX_BYTES / 2),
            );
            assert!(output.len() <= SAMPLE_MAX_BYTES, "攒多少轮都不许越过上限");
        }
    }

    /// 五行在被灌满之前到齐的话，后面再来多少垃圾都不该弄丢这一次读数。
    /// 上限只是「不再收」，不是「收到的全不算」。
    #[test]
    fn a_flood_after_the_real_lines_does_not_lose_the_reading() {
        let mut output = String::new();
        for line in ["CPU 12", "CORES 4", "MEM_TOTAL 100", "MEM_USED 40", "DONE"] {
            push_line(&mut output, OutputKind::Stdout, line);
        }
        let junk = "y".repeat(SAMPLE_MAX_BYTES);
        push_line(&mut output, OutputKind::Stderr, &junk);
        assert_eq!(
            parse_metrics(&output).expect("五行齐了就够").cpu_percent,
            12,
            "垃圾行本来就被解析端忽略"
        );
    }

    /// 正常输出要走得通，且 stderr 前缀要和 `exec_capture` 同口径——
    /// `failure_reason` 与 `parse_metrics` 现有那套解析全靠这两个字节。
    #[test]
    fn bounded_sink_keeps_normal_output_and_the_stderr_prefix() {
        let mut output = String::new();
        push_line(&mut output, OutputKind::Stderr, "motd: banner");
        push_line(&mut output, OutputKind::Stdout, "CPU 12");
        assert_eq!(output, "[stderr] motd: banner\nCPU 12\n");
    }

    /// 外层预算必须先于兜底上限响：那一路会换掉整条会话，把掐断的通道一起带走。
    #[test]
    fn the_caller_deadline_fires_before_the_exec_ceiling() {
        assert!(
            SAMPLE_TIMEOUT_SECS < SAMPLE_CEILING_SECS,
            "同值或反向时，谁先响就交给调度了"
        );
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
