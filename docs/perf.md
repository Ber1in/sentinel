# 性能基线

所有数字都在同一台机器上测得：WSL2 / Ubuntu 22.04 / 12 vCPU / 7 GiB RAM，目标为同机 `sshd`（回环）。脚本见 `crates/ssh/tests/live.rs`，可用 `eval "$(./scripts/test-sshd.sh start)" && cargo test --release -p ssh --test live -- --nocapture` 复现。

## SSH 吞吐（任务 3.5）

| 构建 | 数据量 | 耗时 | 吞吐 |
| --- | --- | --- | --- |
| release | 10 MiB（`head -c 10485760 /dev/zero`） | 35–38 ms | **260–285 MiB/s** |
| debug | 10 MiB | 2.5 s | 3.9 MiB/s |

结论：

- 基线定为 **≥10 MiB/s（release）**，实测高出约 26 倍，无需更换 SSH 栈。
- debug 构建慢两个数量级，原因是 russh 的加密与校验未优化；因此基线只在 release 下强制，debug 下打印提示不判定失败。
- 该数字是裸通道上限，真实终端还受渲染与 IPC 影响；端到端的会话吞吐在任务 10.2 里重新采集。

## 连接复用（任务 3.4）

`two_channels_share_one_tcp_connection` 在建立一条连接后同时打开 PTY 通道与 `sftp` 子系统通道，读取 `/proc/net/tcp` 确认到目标端口的 ESTABLISHED 连接数只增加 1。这是 SFTP 面板复用同一条连接的前提。

## Keepalive（任务 3.3）

- 配置映射有单测覆盖（`transport_config`）：`interval` 与 `max_misses` 直接落到 russh 的 `keepalive_interval` / `keepalive_max`。
- 集成测试 `keepalive_holds_an_idle_connection_open` 用 1 秒间隔空闲 4 秒后仍能执行命令，证明保活确实在发且被应答。
- **30 分钟真实空闲观察**属于验收场景，放在任务 10.1 的端到端验收里做，不在单测里等待。

## 尚未覆盖

- 10 个并发会话的 CPU 与内存（任务 10.2）
- 滚动缓冲回放延迟（任务 10.2）
- GUI 渲染帧率（任务 10.2）

## 如何复现（任务 10.2）

上节「尚未覆盖」里的回放延迟由本节补齐；10 会话与 GUI 帧率仍只能在 Windows 真机采集（见文末）。

一键采集本机能测的全部指标：

```bash
# 前置：Rust stable 在 PATH 里；root 用来起脚本自带的 sshd 夹具（仅此一处需要 root）
export PATH="$HOME/.cargo/bin:$PATH"
./scripts/perf.sh                  # 全部指标，stdout 是 markdown 表格
./scripts/perf.sh --skip-idle      # 只测吞吐 + 回放（不需要 tmux、不建真会话）
./scripts/perf.sh --seconds 60     # 拉长空闲采样，CPU 分辨率更细
```

脚本行为：

- 自己调用 `./scripts/test-sshd.sh start` 起一次性 sshd（`127.0.0.1:2222`），缺 cargo / awk / tmux / sshd 时**直接报错退出**，不会打印空表格。
- 吞吐直接跑既有集成测试 `crates/ssh/tests/live.rs::throughput_reaches_the_baseline`（10 MiB，`head -c`），断言收满 10485760 字节后才采信数字。
- 回放与空闲由 `scripts/perf/` 下的独立测量程序 `sentinel-perf` 完成：它按 path 依赖复用真实 crate，但**不属于** Cargo workspace，因此 `cargo test --workspace` 不受影响。首次带 `--features session` 编译约 1–2 分钟。
- 原始输出落在 `target/perf/*.log`（已被 `.gitignore` 忽略），表格末尾会打印文件名，作为可复核证据。

### 本次实测（2026-11-03）

环境：WSL2 / Linux 6.18.54.1-microsoft-standard-WSL2 / AMD Ryzen 5 3600（12 vCPU）/ 7 GiB RAM / rustc 1.99.1 / tmux 3.2a / release 构建 / 远端为同机回环 sshd。单次运行结果：

| 指标 | 实测 | 采集方式 |
| --- | --- | --- |
| SSH 通道吞吐（10 MiB） | 235.8 MiB/s（42.4 ms） | `cargo test -p ssh --release --test live --exact throughput_reaches_the_baseline` |
| 滚动缓冲写入（20k 行 / 2.41 MB / 586 个 4 KiB 批次） | 5.7 ms（401 MiB/s） | `sentinel-perf replay` |
| 回放延迟：attach 回放 2000 行（`tail_lines`） | 中位 0.089 ms，p95 0.108 ms | `sentinel-perf replay`，200 次 |
| 回放延迟：上翻 20k 行到最早（10 页 × 2000 行，`page_before`） | 中位 4.36 ms，p95 4.78 ms | `sentinel-perf replay`，50 次 |
| 回放延迟：断帧后 resync 全量 20k 行（`since(0)`） | 中位 0.058 ms，p95 0.116 ms | `sentinel-perf replay`，50 次 |
| 空闲单会话 CPU（daemon 进程增量） | 0.000%（30 s 内 0 tick；分辨率 0.033%） | `sentinel-perf idle-cpu --seconds 30` |
| 空闲单会话 RSS（进程整体） | 5.2 MiB | `sentinel-perf idle-cpu` |

读数说明：

- 吞吐在多次运行间有波动，同一台机器上观察到 **236–280 MiB/s**；上表取其中一次，比较不同优化时应连跑 3 次看区间。
- 回放延迟是同步调用耗时，不含 xterm.js 渲染与 IPC 传输；2026-11-03 的 GUI 端到端表现需要在真机上另测。
- 空闲 CPU 为进程级增量（`/proc/self/stat` 的 utime+stime，CLK_TCK=100），因此「0.000%」的含义是「30 s 内不足 1 个 tick」，而不是绝对零。
- 空闲 RSS 包含测量程序自身与 tokio 运行时，只能当作**同一会话模型下**的相对参考，不能当作产品冷启动内存。

### 只能在 Windows 真机上采集（待真机）

| 指标 | 为什么本机测不了 | 采集方式 |
| --- | --- | --- |
| 冷启动内存（daemon + Electron main + renderer + GPU） | Linux 上没有 Electron/WebView2 的多进程模型，WSL 下进程数与内存结构完全不同 | Windows 任务管理器 / `Get-Process` 记录首启 10 s 内峰值与稳定值 |
| 10 个会话的 CPU 与内存 | 需要 10 个真实标签、真实 GUI 渲染与 10 条 SSH 连接；本夹具只起单会话 | 10 个标签稳定 5 分钟后读任务管理器；对 Linux 侧可用 `top` 观察 sshd |
| GUI 渲染帧率 / 输入延迟 | 只有 Windows + 真实 GPU 的 WebGL 路径有意义，WSL 的软件渲染不代表目标环境 | DevTools Performance 录制，见 `docs/reports/acceptance.md` A6 |
| 凭据保管 | Windows Credential Manager 在 Linux 上是内存替身（`MemoryStore`） | 见 `docs/troubleshooting.md`「清理」一节 |

真机数据写入本文件时请保留「采集命令 + 采样时长 + 机器配置」，否则无法判断是优化还是环境差异；对应的端到端操作步骤在 `docs/reports/acceptance.md`。
