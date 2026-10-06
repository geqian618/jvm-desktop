//! 下载器：分块并发 + 断点续传 + SHA-256 校验。
//!
//! 流程：
//! 1. 入口先解析校验和（API 字段优先，其次 `.sha256.txt` 校验和文件），
//!    来源不可得在下载开始前就能从日志看到，而不是数百 MB 下载完才发现；
//! 2. HEAD / Range-0-0 探测总长度与 Range 支持（带响应头超时与重试，
//!    两个探测全部失败才降级单流）；
//! 3. 支持 Range 且文件较大时按 `concurrency` 分块，每块独立 Range 请求，
//!    进度持久化到 `<dest>.part.json` 边车文件，重试/续传按块推进；
//!    CDN/运营商常在连接存活一段时间后限速（表现为越下越慢直到每秒几十
//!    KB），分块按滑动窗口测速，均速跌破自身峰值的 1/4（先快后慢）或
//!    连续两个窗口低于绝对下限（从头就慢：晚间跨境拥塞下新连接可能一直
//!    只有几十 KB/s，攒不出峰值参照）即断开重连换回突发带宽；
//! 4. 完成后流式计算整文件 SHA-256 并比对，通过后才把 `.part` 原子改名
//!    为最终文件——任何失败都不会产生半成品。
//!
//! 校验和来源全部不可得时（如 Oracle 旧版本的 .sha256.txt 已从官网下架，
//! foojay 的 checksum 字段也为空）跳过校验继续下载并告警：这类源的下载
//! 在旧策略下永远失败；归档有效性由安装阶段的 `java -version` 兜底验证。
//!
//! 跨平台说明：文件写入只用 std 文件 API，分块共享一个 `Mutex<File>` 按
//! 绝对偏移写，Windows/Unix 行为一致；无平台专属代码。

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{Error, Result};

/// 进度回调：`(已完成字节, 总字节)`；总字节未知时为 0。
pub type ProgressFn = dyn Fn(u64, u64) + Send + Sync;

/// 单块读取停滞超时：超过则中断本次请求，按断点重试。
const READ_TIMEOUT: Duration = Duration::from_secs(30);
/// 等待响应头的超时：服务器接受连接后不回数据（连接被服务端掐住半死
/// 不活）时，`req.send()` 没有这个超时会永久挂起——进度不走、取消也
/// 无响应的"卡死"就是它。超时按普通可重试失败处理。
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(30);
/// 速率测量滑动窗口：窗口均速与历史峰值比较，判断连接是否被限速。
const RATE_WINDOW: Duration = Duration::from_secs(15);
/// 峰值速率门槛（B/s）：本块峰值低于该值时不做"衰减判定"——只有
/// "曾经快过、现在明显变慢"才是限速信号。注意分块是按并发均分线路的，
/// 门槛必须低于单块的典型峰值（如 1-2MB/s 线路 16 分块下单块峰值仅
/// 60-125KB/s），定高了看门狗永远不触发，限速块会一路爬到底。
const RATE_PEAK_MIN_BPS: u64 = 64 * 1024;
/// 绝对速率下限（B/s）：连续两个测速窗口均速低于该值即弃用当前连接。
/// 覆盖相对衰减规则的盲区——跨境拥塞时段新连接可能"从头就慢"（每个窗口
/// 都只有几十 KB/s，攒不出峰值，衰减判定永远不触发），而实测同一线路换
/// 一条连接常能拿回突发带宽（路由抽签），持续低速就该换线。取值权衡：
/// 定高了会把"慢但稳定"的线路反复拆掉（重连有握手与慢启动代价，贫瘠
/// 线路上换到的连接大概率同样慢），定低了换线迟钝；误判频率由重连冷却
/// 兜底。
const RATE_FLOOR_BPS: u64 = 96 * 1024;
/// 限速重连的冷却时间：重连本身有代价（TCP 慢启动重来），同一块两次
/// 重连至少间隔这么久。取值不能太大——部分下载源对"长连接"降速极狠
/// （新连接能跑满、老连接掉到几 KB/s），轮换是唯一的续命手段。
const RECONNECT_COOLDOWN: Duration = Duration::from_secs(20);
/// 剩余不足该字节数就不再重连：为几十 KB 的尾巴重建连接得不偿失。
const MIN_RECONNECT_REMAINING: u64 = 1024 * 1024;
/// 每块进度落盘间隔（字节）。
const PERSIST_EVERY: u64 = 4 * 1024 * 1024;
/// 文件不小于该值才启用多块并发。
const MIN_CHUNKED_SIZE: u64 = 16 * 1024 * 1024;
/// 每块最小 4MiB，避免小块过多。
const MIN_CHUNK_SIZE: u64 = 4 * 1024 * 1024;
/// 探测类请求（HEAD / Range 0-0 / 校验和文件）的尝试次数：拿到响应即算
/// 成功——4xx/5xx 是有效答案，交调用方按状态码处理；仅网络错误与响应头
/// 超时重试。
const PROBE_ATTEMPTS: u32 = 3;
/// 探测重试的固定间隔：探测请求极轻，固定 1s 足以避开瞬时抖动，不值得
/// 像分块重试那样指数增长。
const PROBE_RETRY_DELAY: Duration = Duration::from_secs(1);
/// 单块连续无进展尝试的上限：「无进展」指收到的字节不足
/// [`MIN_PROGRESS_BYTES`]（限速重连每轮都带着远超该值的进展，不消耗
/// 预算，详见 [`run_chunk`] 文档）。
const MAX_ATTEMPTS: u32 = 5;
/// 一轮尝试视为「有效进展」的最少收字节：达到即清零连续失败计数。
/// 门槛取 64KiB——对 4MiB 起步的分块毫无意义，却足以区分「真的在传」
/// 与「垂死连接每次漏几十字节」：后者靠 1 字节进展就能把重试循环永续。
const MIN_PROGRESS_BYTES: u64 = 64 * 1024;
/// 重试退避步长与封顶：随连续硬失败翻倍（1s→2s→4s→8s）。立即重试在
/// 限流（429）场景是重试风暴——四个分块几乎同时被限又同时重打；对流
/// 中断的重试，退避也能错开持续抖动的窗口。限速重连不走此路径（它有
/// 自己的冷却时钟 [`RECONNECT_COOLDOWN`]）。
const RETRY_BACKOFF_STEP: Duration = Duration::from_secs(1);
const RETRY_BACKOFF_MAX: Duration = Duration::from_secs(8);
/// 下载断点（*.part / *.part.json）的保留时长：取消/失败保留断点用于续传，
/// 超过此时长仍未重试即视为垃圾，由 [`clean_stale_parts`] 回收。
pub const STALE_PART_TTL: Duration = Duration::from_secs(12 * 3600);

#[derive(Debug)]
pub struct DownloadTask {
    pub client: reqwest::Client,
    pub url: String,
    /// 最终落盘路径；下载期间写 `<dest>.part`。
    pub dest_file: PathBuf,
    /// API 提供的 sha256（优先）。
    pub expected_sha256: Option<String>,
    /// 兜底校验和地址（如 `<url>.sha256.txt`）。
    pub checksum_url: Option<String>,
    /// 下载计划给出的期望大小：无校验和时用于完整性兜底校验。
    pub expected_size: Option<u64>,
    /// 分块并发数（服务器支持 Range 时生效，1–16）。
    pub concurrency: usize,
    /// 取消开关：置 true 后各分块尽快中断（GUI 的取消按钮）。
    pub cancel: Option<Arc<AtomicBool>>,
}

/// 看门狗建议弃用当前连接的原因（`bps` 为触发窗口的实测均速）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RateVerdict {
    /// 相对衰减：均速跌破历史峰值的 1/4（先快后慢的长连接限速）。
    Decay { bps: u64, peak_bps: u64 },
    /// 绝对下限：连续两个窗口均速低于 [`RATE_FLOOR_BPS`]（从头就慢的
    /// 连接，没有峰值可参照）。
    Floor { bps: u64 },
}

/// 限速看门狗：滑动窗口测速，两条规则判定连接应当弃用。
///
/// CDN/运营商对存活一段时间的长连接限速，表现为"越下越慢"——连接仍在
/// 滴数据，停滞超时与流错误都不会触发，唯有主动断开重连（按断点续传）
/// 才能换回新连接的突发带宽。两条规则覆盖两类限速形态：
/// - 相对衰减：均速跌破本块历史峰值的 1/4，"曾经快过、现在明显变慢"。
///   线路本身慢时均速≈峰值，不会误伤；
/// - 绝对下限：连续两个窗口低于 [`RATE_FLOOR_BPS`]。跨境拥塞时段新连接
///   可能"从头就慢"（每窗口只有几十 KB/s，攒不出峰值），换连接是拿回
///   突发带宽的唯一手段。
struct RateWatchdog {
    window_start: Instant,
    window_bytes: u64,
    peak_bps: u64,
    /// 连续低于绝对下限的窗口数；出现及格窗口即清零。
    slow_windows: u32,
}

impl RateWatchdog {
    fn new() -> Self {
        Self {
            window_start: Instant::now(),
            window_bytes: 0,
            peak_bps: 0,
            slow_windows: 0,
        }
    }

    /// 累计一次收到的字节；到窗口边界时判定。返回 `Some(原因)` 表示建议
    /// 断开重连——调用方可依据冷却/剩余量否决，否决后调用
    /// [`Self::reset_window`] 继续用当前连接。
    fn on_bytes(&mut self, n: u64, now: Instant) -> Option<RateVerdict> {
        self.window_bytes += n;
        let elapsed = now.duration_since(self.window_start);
        if elapsed < RATE_WINDOW {
            return None;
        }
        let bps = self.window_bytes * 1000 / elapsed.as_millis().max(1) as u64;
        if bps > self.peak_bps {
            self.peak_bps = bps;
        }
        if self.peak_bps >= RATE_PEAK_MIN_BPS && bps * 4 < self.peak_bps {
            return Some(RateVerdict::Decay {
                bps,
                peak_bps: self.peak_bps,
            });
        }
        if bps < RATE_FLOOR_BPS {
            self.slow_windows += 1;
            if self.slow_windows >= 2 {
                return Some(RateVerdict::Floor { bps });
            }
        } else {
            self.slow_windows = 0;
        }
        self.window_bytes = 0;
        self.window_start = now;
        None
    }

    /// 丢弃当前窗口的累计（峰值与慢窗口计数保留——否决只是暂缓换线，
    /// 连接并未恢复，慢窗口 streak 仍有效），从现在起重新计时。
    fn reset_window(&mut self, now: Instant) {
        self.window_bytes = 0;
        self.window_start = now;
    }
}

impl DownloadTask {
    fn cancelled(&self) -> bool {
        self.cancel
            .as_ref()
            .is_some_and(|c| c.load(Ordering::Relaxed))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ChunkState {
    start: u64,
    /// 含端点；总长未知时为 u64::MAX（该模式不支持续传）。
    end: u64,
    done: u64,
}

impl ChunkState {
    fn len(&self) -> u64 {
        if self.end == u64::MAX {
            u64::MAX
        } else {
            self.end - self.start + 1
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Sidecar {
    url: String,
    total: u64,
    #[serde(default)]
    etag: Option<String>,
    chunks: Vec<ChunkState>,
}

struct Shared {
    file: Mutex<std::fs::File>,
    sidecar: Mutex<Sidecar>,
    sidecar_path: PathBuf,
    total_done: AtomicU64,
    progress: Option<Arc<ProgressFn>>,
    total: u64,
}

impl Shared {
    fn report(&self) {
        if let Some(cb) = &self.progress {
            cb(self.total_done.load(Ordering::Relaxed), self.total);
        }
    }

    fn persist(&self) {
        if let Ok(sc) = self.sidecar.lock() {
            let _ = self.persist_sc(&sc);
        }
    }

    fn persist_sc(&self, sc: &Sidecar) -> Result<()> {
        let tmp = self.sidecar_path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string(sc)?)?;
        std::fs::rename(&tmp, &self.sidecar_path)?;
        Ok(())
    }
}

/// 下载主入口；成功返回最终文件路径。
pub async fn download(task: DownloadTask, progress: Option<Arc<ProgressFn>>) -> Result<PathBuf> {
    let started = std::time::Instant::now();
    // 下载前先确定校验和：字段缺失时先取几 KB 的校验和文件，让"来源不可得"
    // 在开头就暴露，而不是数百 MB 下完之后才失败。
    if task.cancelled() {
        return Err(Error::Cancelled);
    }
    let expected = resolve_expected(&task).await;
    if expected.is_none() {
        tracing::warn!(
            dest = %task.dest_file.display(),
            "无可用校验和（API 字段与校验和文件均不可得），跳过完整性校验"
        );
    }
    if task.cancelled() {
        return Err(Error::Cancelled);
    }

    if let Some(parent) = task.dest_file.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // 目标文件已存在：有校验和时验证通过才复用；无校验和时至少核对期望
    // 大小，不符视为损坏/陈旧删除重下（否则截断文件会被永久复用）；大小
    // 信息也缺失时只能复用（可能是上次跳过校验的下载）。
    if task.dest_file.exists() {
        match &expected {
            Some(exp) => {
                if sha256_file(&task.dest_file)
                    .map(|a| a == *exp)
                    .unwrap_or(false)
                {
                    return Ok(task.dest_file);
                }
                std::fs::remove_file(&task.dest_file).ok();
            }
            None => {
                let size_ok = match task.expected_size {
                    Some(sz) if sz > 0 => std::fs::metadata(&task.dest_file)
                        .map(|m| m.len() == sz)
                        .unwrap_or(false),
                    _ => true,
                };
                if size_ok {
                    return Ok(task.dest_file);
                }
                tracing::warn!(
                    dest = %task.dest_file.display(),
                    "已有文件大小与期望不符且无校验和，删除后重新下载"
                );
                std::fs::remove_file(&task.dest_file).ok();
            }
        }
    }

    let part = part_path(&task.dest_file);
    let sidecar_path = sidecar_path(&task.dest_file);

    let (total, ranges_ok, etag) = probe_source(&task).await.unwrap_or((0, false, None));
    // 校验和与探测都可能耗时（含重试），返回后核对取消，避免为已取消的
    // 任务预分配文件与建立分块。
    if task.cancelled() {
        return Err(Error::Cancelled);
    }

    let existing = if sidecar_path.exists() && part.exists() {
        load_sidecar(&sidecar_path)
    } else {
        None
    };
    let resumable = existing
        .as_ref()
        .is_some_and(|s| s.url == task.url && s.total == total && total > 0 && s.etag == etag);

    let sidecar = match existing.filter(|_| resumable) {
        Some(s) => s,
        None if total > 0 => {
            let chunks = plan_chunks(total, task.concurrency, ranges_ok);
            // 全新任务：part 重建并预分配到最终大小。
            let f = std::fs::File::create(&part)
                .map_err(|e| Error::msg(format!("创建 {} 失败: {e}", part.display())))?;
            f.set_len(total)
                .map_err(|e| Error::msg(format!("预分配 {} 失败: {e}", part.display())))?;
            f.sync_all()
                .map_err(|e| Error::msg(format!("落盘 {} 失败: {e}", part.display())))?;
            let sc = Sidecar {
                url: task.url.clone(),
                total,
                etag: etag.clone(),
                chunks,
            };
            write_sidecar(&sc, &sidecar_path)?;
            sc
        }
        None => {
            // 总长未知：单流下载，不支持续传；part 由下面统一打开前创建。
            let sc = Sidecar {
                url: task.url.clone(),
                total: 0,
                etag: None,
                chunks: vec![ChunkState {
                    start: 0,
                    end: u64::MAX,
                    done: 0,
                }],
            };
            write_sidecar(&sc, &sidecar_path)?;
            std::fs::File::create(&part)
                .map_err(|e| Error::msg(format!("创建 {} 失败: {e}", part.display())))?;
            sc
        }
    };

    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(&part)
        .map_err(|e| Error::msg(format!("打开 {} 失败: {e}", part.display())))?;
    let total_done = sidecar.chunks.iter().map(|c| c.done).sum();
    tracing::info!(
        url = %task.url,
        dest = %task.dest_file.display(),
        total,
        chunks = sidecar.chunks.len(),
        resumed = total_done > 0,
        "开始下载"
    );
    let shared = Arc::new(Shared {
        file: Mutex::new(file),
        sidecar: Mutex::new(sidecar.clone()),
        sidecar_path: sidecar_path.clone(),
        total_done: AtomicU64::new(total_done),
        progress,
        total,
    });
    shared.report();

    let mut handles = Vec::new();
    for idx in 0..sidecar.chunks.len() {
        let shared = Arc::clone(&shared);
        let url = task.url.clone();
        let client = task.client.clone();
        let cancelled = task.cancel.clone();
        handles.push(tokio::spawn(async move {
            run_chunk(&client, &url, &shared, idx, cancelled).await
        }));
    }
    // 取消监视：cancel 置位后立刻 abort 全部分块任务。协同取消要等每个
    // await 点自己检查标志，任务若卡在无响应的请求上，取消会迟迟不生效；
    // abort 直接拆任务，连接随之关闭，取消按钮立即生效。
    let cancel_watcher = task.cancel.clone().map(|flag| {
        let aborts: Vec<_> = handles.iter().map(|h| h.abort_handle()).collect();
        tokio::spawn(async move {
            loop {
                if flag.load(Ordering::Relaxed) {
                    for a in aborts {
                        a.abort();
                    }
                    return;
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        })
    });

    // 任一分块失败即叫停其余分块（半成品只存在于 .part，最终文件不受影响）。
    let abort_all: Vec<_> = handles.iter().map(|h| h.abort_handle()).collect();
    let mut outcome: Result<()> = Ok(());
    for handle in handles {
        match handle.await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                outcome = Err(e);
                break;
            }
            Err(e) if e.is_cancelled() => {
                outcome = Err(Error::Cancelled);
                break;
            }
            Err(e) => {
                outcome = Err(Error::msg(format!("下载任务崩溃: {e}")));
                break;
            }
        }
    }
    if outcome.is_err() {
        for a in &abort_all {
            a.abort();
        }
        // abort 会跳过分块自己的收尾落盘，这里补一次，保住已下载的断点。
        shared.persist();
    }
    if let Some(w) = cancel_watcher {
        w.abort();
    }
    outcome?;

    shared.file.lock().unwrap().sync_all()?;

    // 校验：有期望值才比对；来源不可得的情况入口已告警并跳过，
    // 归档是否有效由安装阶段的 java -version 兜底。
    if let Some(exp) = &expected {
        let actual = sha256_file(&part)?;
        if actual != *exp {
            cleanup(&part, &sidecar_path);
            return Err(Error::ChecksumMismatch {
                expected: exp.clone(),
                actual,
            });
        }
    } else if let Some(sz) = task.expected_size.filter(|&s| s > 0) {
        // 无校验和时用计划大小兜底：拦截「流中断被误判完成」等场景产出的
        // 截断文件（有校验和的路径由上面的比对覆盖，无需重复检查大小）。
        let actual = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
        if actual != sz {
            cleanup(&part, &sidecar_path);
            return Err(Error::msg(format!(
                "下载不完整（{actual}/{sz} 字节）且来源无校验和，已丢弃断点"
            )));
        }
    }

    // 全部通过才落最终文件名。
    if task.dest_file.exists() {
        std::fs::remove_file(&task.dest_file)?;
    }
    std::fs::rename(&part, &task.dest_file).map_err(|e| {
        Error::msg(format!(
            "落位 {} → {} 失败: {e}",
            part.display(),
            task.dest_file.display()
        ))
    })?;
    std::fs::remove_file(&sidecar_path).ok();
    tracing::info!(
        url = %task.url,
        dest = %task.dest_file.display(),
        bytes = shared.total_done.load(Ordering::Relaxed),
        elapsed_ms = started.elapsed().as_millis() as u64,
        verified = expected.is_some(),
        "下载完成"
    );
    Ok(task.dest_file)
}

/// 执行单个分块：断点续传 + 停滞重试 + 低速重连 + 取消检查。
///
/// 重试预算只计「连续无进展失败」：一次尝试收到的字节达到
/// [`MIN_PROGRESS_BYTES`] 才把计数清零。限速重连（看门狗）每轮至少带
/// 一个测速窗口（15s）的数据，常态远超门槛，不消耗预算——一次长下载
/// 可能重连十几轮，若沿用总次数上限会在中途误报失败。
async fn run_chunk(
    client: &reqwest::Client,
    url: &str,
    shared: &Shared,
    idx: usize,
    cancel: Option<Arc<AtomicBool>>,
) -> Result<()> {
    let is_cancelled = || cancel.as_ref().is_some_and(|c| c.load(Ordering::Relaxed));
    let mut hard_failures = 0u32;
    // 限速重连的冷却锚点（跨尝试保留）：重连只在"深衰减 + 冷却到期 +
    // 剩余量足够"时发生，频率由冷却时间自然限制，不需要硬上限。
    let mut last_reconnect: Option<Instant> = None;
    loop {
        if is_cancelled() {
            return Err(Error::Cancelled);
        }
        // 总长未知（服务器不支持 Range/HEAD）时无法续传：非首次尝试且已有
        // 部分数据时整档重来——不带 Range 头的重试会从服务端第 0 字节重发，
        // 若沿用旧写偏移会把头部数据写到文件中段造成错位损坏。
        {
            let mut sc = shared.sidecar.lock().unwrap();
            let c = &mut sc.chunks[idx];
            if c.end == u64::MAX && c.done > 0 {
                let dropped = c.done;
                c.done = 0;
                drop(sc);
                shared.total_done.fetch_sub(dropped, Ordering::Relaxed);
                shared.file.lock().unwrap().set_len(0)?;
                shared.persist();
            }
        }
        // 快照本块状态，决定 Range 头与本次写起点。
        let snapshot = {
            let sc = shared.sidecar.lock().unwrap();
            let c = &sc.chunks[idx];
            if c.done >= c.len() {
                return Ok(());
            }
            (
                c.end == u64::MAX,
                c.start + c.done, // 本批网络数据要写的文件偏移
                c.start > 0 || c.done > 0,
                c.len().saturating_sub(c.done),
            )
        };
        let (unknown_total, write_offset, resumed, remaining) = snapshot;

        let mut req = client.get(url);
        if !unknown_total {
            let (start, end) = {
                let sc = shared.sidecar.lock().unwrap();
                let c = &sc.chunks[idx];
                (c.start + c.done, c.end)
            };
            req = req.header(reqwest::header::RANGE, format!("bytes={start}-{end}"));
        }
        let resp = match tokio::time::timeout(RESPONSE_TIMEOUT, req.send()).await {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => {
                tracing::warn!(
                    chunk = idx,
                    failures = hard_failures + 1,
                    max = MAX_ATTEMPTS,
                    error = %e,
                    "分块请求失败，准备重试"
                );
                hard_failures += 1;
                if hard_failures >= MAX_ATTEMPTS {
                    break;
                }
                tokio::time::sleep(retry_backoff(hard_failures)).await;
                continue; // 网络抖动：退避后重试
            }
            // 等响应头超时：服务器收了连接不吐数据。按可重试失败处理，
            // 否则任务会永久挂起——进度不动、取消也无响应。
            Err(_) => {
                tracing::warn!(
                    chunk = idx,
                    timeout_s = RESPONSE_TIMEOUT.as_secs(),
                    failures = hard_failures + 1,
                    max = MAX_ATTEMPTS,
                    "等待响应头超时，准备重试"
                );
                hard_failures += 1;
                if hard_failures >= MAX_ATTEMPTS {
                    break;
                }
                tokio::time::sleep(retry_backoff(hard_failures)).await;
                continue;
            }
        };
        let status = resp.status();
        if status == reqwest::StatusCode::OK && resumed && !unknown_total {
            // 服务器无视 Range 返回 200：该源不支持续传，整档重来。
            return Err(Error::msg("服务器不支持断点续传（对 Range 请求返回 200）"));
        }
        if !(status == reqwest::StatusCode::PARTIAL_CONTENT
            || (status == reqwest::StatusCode::OK && !resumed))
        {
            // 4xx（限流除外）是确定性失败：链接失效/文件下架/无权限，
            // 重试只是浪费，直接报出状态码。
            if status.is_client_error() && status != reqwest::StatusCode::TOO_MANY_REQUESTS {
                return Err(Error::msg(format!(
                    "下载源返回 {status}，文件可能已下架或链接失效"
                )));
            }
            hard_failures += 1;
            if hard_failures >= MAX_ATTEMPTS {
                break;
            }
            // 429/5xx：退避后重试，避免与其它分块同步重打形成风暴。
            tokio::time::sleep(retry_backoff(hard_failures)).await;
            continue;
        }

        let mut since_persist = 0u64;
        let mut attempt_bytes = 0u64;
        // 限速看门狗：仅对支持 Range 的块生效（未知总长时重连等于整档
        // 重来，反而越重连越慢）。断点已落盘，续传无损。
        let mut watchdog = RateWatchdog::new();
        let mut offset = write_offset;
        let mut stream = resp.bytes_stream();
        loop {
            if is_cancelled() {
                shared.persist();
                return Err(Error::Cancelled);
            }
            let next = tokio::time::timeout(READ_TIMEOUT, stream.next()).await;
            let Some(item) = next.ok() else {
                // 读取停滞：保留断点，重试。
                shared.persist();
                break;
            };
            match item {
                None => {
                    // 流自然结束：未知总长时即为完成。
                    if unknown_total {
                        shared.persist();
                        return Ok(());
                    }
                    shared.persist();
                    break;
                }
                Some(Err(_)) => {
                    shared.persist();
                    break;
                }
                Some(Ok(bytes)) => {
                    let n = bytes.len() as u64;
                    {
                        let mut f = shared.file.lock().unwrap();
                        f.seek(SeekFrom::Start(offset))?;
                        f.write_all(&bytes)?;
                    }
                    offset += n;
                    {
                        let mut sc = shared.sidecar.lock().unwrap();
                        sc.chunks[idx].done += n;
                    }
                    shared.total_done.fetch_add(n, Ordering::Relaxed);
                    since_persist += n;
                    attempt_bytes += n;
                    shared.report();
                    if since_persist >= PERSIST_EVERY {
                        shared.persist();
                        since_persist = 0;
                    }
                    // 低速看门狗：先快后慢（相对衰减）或从头就慢（连续
                    // 两窗口低于绝对下限）都建议换线，受冷却与剩余量约束。
                    if !unknown_total {
                        if let Some(verdict) = watchdog.on_bytes(n, Instant::now()) {
                            let cooled =
                                last_reconnect.is_none_or(|t| t.elapsed() >= RECONNECT_COOLDOWN);
                            if cooled && remaining > MIN_RECONNECT_REMAINING {
                                last_reconnect = Some(Instant::now());
                                match verdict {
                                    RateVerdict::Decay { bps, peak_bps } => tracing::info!(
                                        chunk = idx,
                                        rate_kb = bps / 1024,
                                        peak_kb = peak_bps / 1024,
                                        "分块疑似被限速（先快后慢），断开重连（按断点续传）"
                                    ),
                                    RateVerdict::Floor { bps } => tracing::info!(
                                        chunk = idx,
                                        rate_kb = bps / 1024,
                                        "分块疑似被限速（持续低速），断开重连（按断点续传）"
                                    ),
                                }
                                shared.persist();
                                break;
                            }
                            // 否决重连：重置窗口继续用当前连接慢慢爬。
                            watchdog.reset_window(Instant::now());
                        }
                    }
                }
            }
        }

        // 流提前结束/停滞只算未完成：自然结束的路径已在上面 return Ok，
        // 走到这里必须重试补齐（unknown_total 的 len 是 u64::MAX，恒为假，
        // 会在下一轮尝试整档重来）。此前用 `unknown_total ||` 短路，会把
        // 中断的残缺下载误判为完成，无校验和时截断文件直接落位。
        let finished = {
            let sc = shared.sidecar.lock().unwrap();
            let c = &sc.chunks[idx];
            c.done >= c.len()
        };
        if finished {
            shared.persist();
            return Ok(());
        }
        // 有足量进展（含限速重连——每轮重连至少带一个测速窗口的数据，
        // 常态远超门槛）则连续失败计数清零；不足 MIN_PROGRESS_BYTES 视同
        // 无进展：计一次硬失败并退避。「每次漏几十字节」的垂死源不再能
        // 靠 1 字节进展把重试循环永续下去。
        if attempt_bytes >= MIN_PROGRESS_BYTES {
            hard_failures = 0;
        } else {
            hard_failures += 1;
            if hard_failures >= MAX_ATTEMPTS {
                break;
            }
            tokio::time::sleep(retry_backoff(hard_failures)).await;
        }
        // 流提前结束：按断点重试补齐。
    }
    Err(Error::msg(format!(
        "分块 {idx} 连续 {MAX_ATTEMPTS} 次尝试无进展，仍未完成"
    )))
}

/// 计算分块计划：支持 Range 时按并发数均分，否则单块（未知总长）。
fn plan_chunks(total: u64, concurrency: usize, ranges_ok: bool) -> Vec<ChunkState> {
    if !ranges_ok {
        return vec![ChunkState {
            start: 0,
            end: u64::MAX,
            done: 0,
        }];
    }
    let n = if total < MIN_CHUNKED_SIZE {
        1
    } else {
        concurrency
            .clamp(1, 16)
            .min((total / MIN_CHUNK_SIZE).max(1) as usize)
    };
    let per = total / n as u64;
    let mut chunks = Vec::with_capacity(n);
    let mut start = 0u64;
    for i in 0..n {
        // 最后一块吞掉余数。
        let end = if i == n - 1 {
            total - 1
        } else {
            start + per - 1
        };
        chunks.push(ChunkState {
            start,
            end,
            done: 0,
        });
        start = end + 1;
    }
    chunks
}

/// 第 `n` 次连续硬失败后的退避时长：1s、2s、4s，之后封顶 8s。
fn retry_backoff(hard_failures: u32) -> Duration {
    let shift = hard_failures.saturating_sub(1).min(3);
    (RETRY_BACKOFF_STEP * (1u32 << shift)).min(RETRY_BACKOFF_MAX)
}

/// 探测类请求的统一发送：响应头超时 + 固定间隔重试。
///
/// 分块请求有 [`RESPONSE_TIMEOUT`] 兜底「收下连接不吐响应头」的半死
/// 连接，探测请求此前是裸 `send()`——一旦挂起，整个下载卡在开头且取消
/// 无效（取消监视器要到分块启动后才运行）；且探测是只此一次的判定，
/// 一次网络抖动就会把整个下载静默降级为单流无续传。这里补上超时与
/// 重试，把瞬时抖动挡在降级之前。拿到响应（含 4xx/5xx）即算成功——
/// 那是有意义的答案，交调用方按状态码处理。
async fn send_probed(
    build_req: impl Fn() -> reqwest::RequestBuilder,
    cancelled: impl Fn() -> bool,
    what: &str,
) -> Option<reqwest::Response> {
    for attempt in 1..=PROBE_ATTEMPTS {
        if cancelled() {
            return None;
        }
        if attempt > 1 {
            tokio::time::sleep(PROBE_RETRY_DELAY).await;
        }
        match tokio::time::timeout(RESPONSE_TIMEOUT, build_req().send()).await {
            Ok(Ok(resp)) => return Some(resp),
            Ok(Err(e)) => {
                tracing::warn!(probe = what, attempt, error = %e, "探测请求失败");
            }
            Err(_) => {
                tracing::warn!(
                    probe = what,
                    attempt,
                    timeout_s = RESPONSE_TIMEOUT.as_secs(),
                    "探测请求等待响应头超时"
                );
            }
        }
    }
    None
}

/// 探测总长与 Range 支持：先 HEAD，被拒（403/405 等）退化为 Range 0-0。
/// 两个探测各自带响应头超时与重试（[`send_probed`]）。
async fn probe_source(task: &DownloadTask) -> Option<(u64, bool, Option<String>)> {
    // 注意：hyper 客户端对 HEAD 响应的 resp.content_length() 恒为 0
    // （报告的是 body 长度），必须直接读 Content-Length 头。
    fn header_len(resp: &reqwest::Response) -> Option<u64> {
        resp.headers()
            .get(reqwest::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<u64>().ok())
    }

    let cancelled = || task.cancelled();
    if let Some(resp) = send_probed(|| task.client.head(&task.url), cancelled, "head").await {
        if resp.status().is_success() {
            let ranges = resp
                .headers()
                .get(reqwest::header::ACCEPT_RANGES)
                .and_then(|v| v.to_str().ok())
                .map(|v| v.contains("bytes"))
                .unwrap_or(false);
            let etag = etag_of(&resp);
            if let Some(total) = header_len(&resp) {
                return Some((total, ranges, etag));
            }
        }
    }

    // Range 0-0 探测：206 + Content-Range: bytes 0-0/<total>
    let resp = send_probed(
        || {
            task.client
                .get(&task.url)
                .header(reqwest::header::RANGE, "bytes=0-0")
        },
        cancelled,
        "range0-0",
    )
    .await?;
    if resp.status() == reqwest::StatusCode::PARTIAL_CONTENT {
        let cr = resp
            .headers()
            .get(reqwest::header::CONTENT_RANGE)?
            .to_str()
            .ok()?;
        let total = cr.rsplit('/').next()?.parse::<u64>().ok()?;
        return Some((total, true, etag_of(&resp)));
    }
    None
}

fn etag_of(resp: &reqwest::Response) -> Option<String> {
    resp.headers()
        .get(reqwest::header::ETAG)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

fn part_path(dest: &Path) -> PathBuf {
    let mut name = file_name_of(dest);
    name.push_str(".part");
    dest.with_file_name(name)
}

fn sidecar_path(dest: &Path) -> PathBuf {
    let mut name = file_name_of(dest);
    name.push_str(".part.json");
    dest.with_file_name(name)
}

fn file_name_of(p: &Path) -> String {
    p.file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "download".to_string())
}

fn load_sidecar(path: &Path) -> Option<Sidecar> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

fn write_sidecar(sc: &Sidecar, path: &Path) -> Result<()> {
    std::fs::write(path, serde_json::to_string(sc)?)?;
    Ok(())
}

fn cleanup(part: &Path, sidecar: &Path) {
    std::fs::remove_file(part).ok();
    std::fs::remove_file(sidecar).ok();
}

/// 流式计算文件 SHA-256（1MiB 缓冲，~200MB 约 1s）。
pub fn sha256_file(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex(&hasher.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// 清理下载目录里的过期断点文件（`*.part` / `*.part.json`）。
///
/// 取消/失败会保留断点供续传，但之后若不再重试同一版本，这些文件会
/// 永久占据空间（一个中断的 JDK 包可达数百 MB）。在应用启动、CLI 命令
/// 执行前调用：此时不可能有进行中的下载，按修改时间过期即删，返回
/// 清理的文件数。
pub fn clean_stale_parts(downloads_dir: &Path, max_age: Duration) -> usize {
    let Ok(entries) = std::fs::read_dir(downloads_dir) else {
        return 0;
    };
    let mut removed = 0usize;
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        if !name.ends_with(".part") && !name.ends_with(".part.json") {
            continue;
        }
        let stale = std::fs::metadata(&path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age >= max_age);
        if stale && std::fs::remove_file(&path).is_ok() {
            removed += 1;
        }
    }
    if removed > 0 {
        tracing::info!(
            dir = %downloads_dir.display(),
            removed,
            ttl_hours = max_age.as_secs() / 3600,
            "已清理过期的下载断点文件"
        );
    }
    removed
}

/// 从 `.sha256.txt` 风格文本中提取首个 64 位十六进制校验和。
pub fn parse_checksum_text(text: &str) -> Option<String> {
    for token in text.split_whitespace() {
        let t = token.trim().to_lowercase();
        if t.len() == 64 && t.chars().all(|c| c.is_ascii_hexdigit()) {
            return Some(t);
        }
    }
    None
}

async fn fetch_checksum_hex(client: &reqwest::Client, url: &str) -> Result<Option<String>> {
    let resp = match send_probed(|| client.get(url), || false, "checksum").await {
        Some(r) => r,
        None => return Err(Error::msg("校验和文件请求失败（含重试）")),
    };
    let resp = resp.error_for_status()?;
    // 响应体同样可能停滞：校验和文件只有几 KB，套响应头超时足够。
    let text = tokio::time::timeout(RESPONSE_TIMEOUT, resp.text())
        .await
        .map_err(|_| Error::msg("读取校验和文件超时"))??;
    Ok(parse_checksum_text(&text))
}

/// 解析期望校验和：API 字段优先，其次 `.sha256.txt` 风格的校验和文件；
/// 都不可得时返回 None（由调用方决定是否跳过校验继续下载）。
async fn resolve_expected(task: &DownloadTask) -> Option<String> {
    if let Some(e) = &task.expected_sha256 {
        let e = e.trim().to_lowercase();
        if !e.is_empty() {
            return Some(e);
        }
    }
    let url = task.checksum_url.as_deref()?;
    match fetch_checksum_hex(&task.client, url).await {
        Ok(Some(h)) => Some(h),
        Ok(None) => {
            tracing::warn!(url, "校验和文件内容中没有 64 位十六进制校验和");
            None
        }
        Err(e) => {
            tracing::warn!(
                url,
                error = %e,
                "获取校验和文件失败，下载继续但跳过完整性校验"
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 把 N KB 喂给一个窗口并推进到窗口末尾，模拟一段时间内的传输。
    fn feed(w: &mut RateWatchdog, t0: Instant, window: usize, kb: u64) -> Option<RateVerdict> {
        w.on_bytes(kb * 1024, t0 + RATE_WINDOW * window as u32)
    }

    #[test]
    fn watchdog_triggers_on_decay_after_fast_start() {
        let mut w = RateWatchdog::new();
        let t0 = Instant::now();
        // 窗口1：~0.8MiB/s（突发带宽）→ 记录峰值，不触发。
        assert_eq!(feed(&mut w, t0, 1, 8192), None);
        // 窗口2：~25KiB/s，跌破峰值 1/4 → 触发重连。
        assert!(feed(&mut w, t0, 2, 256).is_some());
    }

    #[test]
    fn watchdog_floor_triggers_on_slow_from_start() {
        let mut w = RateWatchdog::new();
        let t0 = Instant::now();
        // 晚间拥塞下的典型形态：连接从头就慢（~34KiB/s），每窗口都攒不出
        // 峰值，衰减判定永不触发；连续两个慢窗口后按绝对下限换线。
        assert_eq!(feed(&mut w, t0, 1, 512), None);
        assert!(matches!(
            feed(&mut w, t0, 2, 512),
            Some(RateVerdict::Floor { .. })
        ));
    }

    #[test]
    fn watchdog_floor_needs_consecutive_slow_windows() {
        let mut w = RateWatchdog::new();
        let t0 = Instant::now();
        // 慢（~64KiB/s）与及格（~110KiB/s）交替：单次慢窗口可能只是抖动，
        // 不触发；及格窗口把连续计数清零。
        assert_eq!(feed(&mut w, t0, 1, 960), None);
        assert_eq!(feed(&mut w, t0, 2, 1650), None);
        assert_eq!(feed(&mut w, t0, 3, 960), None);
        assert_eq!(feed(&mut w, t0, 4, 1650), None);
    }

    #[test]
    fn watchdog_floor_survives_veto_reset() {
        let mut w = RateWatchdog::new();
        let t0 = Instant::now();
        // 触发后被调用方否决（冷却中）：重置窗口但保留慢窗口计数——连接
        // 并未恢复，下个慢窗口应立即再次建议换线（受冷却实际约束频率）。
        assert_eq!(feed(&mut w, t0, 1, 512), None);
        assert!(matches!(
            feed(&mut w, t0, 2, 512),
            Some(RateVerdict::Floor { .. })
        ));
        w.reset_window(t0 + RATE_WINDOW * 2);
        assert!(matches!(
            feed(&mut w, t0, 3, 512),
            Some(RateVerdict::Floor { .. })
        ));
    }

    #[test]
    fn watchdog_ignores_ramp_up() {
        let mut w = RateWatchdog::new();
        let t0 = Instant::now();
        // TCP 慢启动逐步加速：每一窗口都在创新高，永不触发。
        assert_eq!(feed(&mut w, t0, 1, 4096), None);
        assert_eq!(feed(&mut w, t0, 2, 8192), None);
        assert_eq!(feed(&mut w, t0, 3, 16384), None);
    }

    #[test]
    fn watchdog_swallows_bytes_below_window_boundary() {
        let mut w = RateWatchdog::new();
        let t0 = Instant::now();
        // 窗口未满时的字节只累计不判定。
        assert_eq!(w.on_bytes(1024, t0 + Duration::from_secs(1)), None);
        assert_eq!(w.window_bytes, 1024);
    }

    #[test]
    fn part_paths() {
        let dest = PathBuf::from("/tmp/x/jdk.zip");
        assert_eq!(part_path(&dest), PathBuf::from("/tmp/x/jdk.zip.part"));
        assert_eq!(
            sidecar_path(&dest),
            PathBuf::from("/tmp/x/jdk.zip.part.json")
        );
    }

    #[test]
    fn chunk_planning() {
        assert_eq!(plan_chunks(100, 8, false).len(), 1, "不支持 Range 用单块");

        let chunks = plan_chunks(9 * 1024 * 1024, 2, true);
        assert_eq!(chunks.len(), 1, "小于 16MiB 用单块");

        let total = 33 * 1024 * 1024u64;
        let chunks = plan_chunks(total, 4, true);
        assert_eq!(chunks.len(), 4);
        assert_eq!(chunks[0].start, 0);
        assert_eq!(chunks[3].end, total - 1);
        let sum: u64 = chunks.iter().map(|c| c.len()).sum();
        assert_eq!(sum, total, "分块必须无缝覆盖全文件");

        let one = plan_chunks(total, 1, true);
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].len(), total);
    }

    #[test]
    fn checksum_text_parsing() {
        assert_eq!(
            parse_checksum_text(
                "e53a79c3c3d86865bd7e787903884331068e71321714ffd44f145785affc7cb0  jdk.zip\n"
            ),
            Some("e53a79c3c3d86865bd7e787903884331068e71321714ffd44f145785affc7cb0".into())
        );
        assert_eq!(parse_checksum_text("not a checksum"), None);
    }

    #[test]
    fn retry_backoff_doubles_then_caps() {
        assert_eq!(retry_backoff(1), Duration::from_secs(1));
        assert_eq!(retry_backoff(2), Duration::from_secs(2));
        assert_eq!(retry_backoff(3), Duration::from_secs(4));
        assert_eq!(retry_backoff(4), Duration::from_secs(8));
        assert_eq!(retry_backoff(9), Duration::from_secs(8), "封顶");
    }

    #[test]
    fn sha256_of_small_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f.bin");
        std::fs::write(&p, b"hello jvm").unwrap();
        let h = sha256_file(&p).unwrap();
        assert_eq!(h.len(), 64);
        assert_eq!(sha256_file(&p).unwrap(), h, "同文件哈希稳定");
    }

    #[test]
    fn sidecar_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let sc = Sidecar {
            url: "https://example/x.zip".into(),
            total: 100,
            etag: Some("\"abc\"".into()),
            chunks: vec![
                ChunkState {
                    start: 0,
                    end: 49,
                    done: 10,
                },
                ChunkState {
                    start: 50,
                    end: 99,
                    done: 0,
                },
            ],
        };
        let p = dir.path().join("sc.json");
        write_sidecar(&sc, &p).unwrap();
        let loaded = load_sidecar(&p).unwrap();
        assert_eq!(loaded.chunks[0].done, 10);
        assert_eq!(loaded.etag.as_deref(), Some("\"abc\""));
    }

    #[test]
    fn stale_part_cleanup() {
        let dir = tempfile::tempdir().unwrap();
        let old_part = dir.path().join("old.zip.part");
        let old_sidecar = dir.path().join("old.zip.part.json");
        let recent_part = dir.path().join("new.zip.part");
        std::fs::write(&old_part, b"x").unwrap();
        std::fs::write(&old_sidecar, b"x").unwrap();
        std::fs::write(&recent_part, b"x").unwrap();

        // 把 old 的修改时间拨到 TTL 之外
        let past =
            std::time::SystemTime::now() - STALE_PART_TTL - std::time::Duration::from_secs(3600);
        for p in [&old_part, &old_sidecar] {
            std::fs::OpenOptions::new()
                .write(true)
                .open(p)
                .unwrap()
                .set_times(std::fs::FileTimes::new().set_modified(past))
                .unwrap();
        }

        assert_eq!(clean_stale_parts(dir.path(), STALE_PART_TTL), 2);
        assert!(!old_part.exists() && !old_sidecar.exists());
        assert!(recent_part.exists(), "TTL 内的断点保留用于续传");

        // 非 .part/.part.json 文件不受影响，再次运行无事可做
        std::fs::write(dir.path().join("done.zip"), b"x").unwrap();
        assert_eq!(clean_stale_parts(dir.path(), STALE_PART_TTL), 0);
        assert!(dir.path().join("done.zip").exists());
        assert!(recent_part.exists());
    }
}
