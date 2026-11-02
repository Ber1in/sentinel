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
