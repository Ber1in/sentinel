---
comet_change: persistent-ssh-terminal-core
role: technical-design
canonical_spec: openspec
---

# 设计：持久化跨主机 SSH 终端核心

## 背景

见 `proposal.md` 的「为什么」。设计层面有三条硬约束：

1. **GUI 生命周期不可信**：渲染进程会崩溃、被杀死、被升级替换，任何连接与缓冲都不能住在渲染进程里。
2. **远端进程只能由远端保住**：客户端无法让 Linux 上的进程穿越主机重启，持久化必须落脚在远端（首版为 tmux）。
3. **跨主机是数据模型问题，不是窗口问题**：tty7 把主机绑在 workspace 上，导致跨主机标签无法表达；这里主机必须是会话属性，而不是容器属性。

行为契约以 `specs/*/spec.md` 为准，本文只记录实现决策。

## 目标 / 非目标

**目标**

- 守护进程独占 SSH 连接与终端缓冲，GUI 是无状态视图，可随时重建
- 单窗口内标签与分屏任意跨主机
- 断线自动恢复；服务器重启后恢复到「可继续工作」的状态
- 一条连接复用多个能力（终端流、后续 SFTP），不额外建连

**非目标**

- 不做本地 shell / WSL / 串口：**首版不需要 ConPTY**，daemon 不启动任何本地 shell
- 不做远端 helper 协议（阶段二，`PersistenceProvider` 接口预留）
- 不做 tmux control mode（`-CC`）
- 不做多窗口共享布局、云同步、跨设备

## 架构总览

```
+---------------------------- Windows 11 / WSLg ----------------------------+
|                                                                          |
|  supervisor (同一二进制 --supervise)                                      |
|    | spawn / restart with backoff                                        |
|    v                                                                     |
|  daemon (常驻)                          GUI (Electron, 可随时被杀)         |
|  +----------------------------+         +-----------------------------+  |
|  | SessionRegistry            |         | Renderer (React)            |  |
|  |  host -> SshConnection     |  WS     |  pane tree / xterm registry  |  |
|  |  pane -> tmux session      |<------->|                             |  |
|  |  ring buffer + spill       | 127.0.0.1  +-------------+-------------+ |
|  |  reconnect state machine   |         | Main (Node broker)          |  |
|  |  SQLite store              |         |  token / batching / dialogs |  |
|  +-------------+--------------+         +-----------------------------+  |
|                | russh (one TCP conn per host)                            |
+----------------|---------------------------------------------------------+
                 v
        +--------------------+       +------------------------------+
        | OpenSSH server     |       | tmux server                  |
        |  channel: pty      | ----> |  one session per pane        |
        |  channel: sftp     |       |  history-limit 100000        |
        +--------------------+       +------------------------------+
```

## 决策

### D1：进程模型 = supervisor + daemon + GUI

- GUI 只是 WebSocket 客户端，关闭、崩溃、升级都不影响会话。
- 增加 **supervisor**（同一二进制 `--supervise` 模式）：spawn daemon 并等待其句柄，异常退出按 1s/2s/5s/10s 退避重启。用户级进程没有服务级失败重启，这是满足「崩溃后无人干预自动重启」的最小代价方案。
- supervisor 自身由登录自启拉起；GUI 发现 `daemon.json` 失效时启动 supervisor（单实例互斥，重复启动无害）。
- 已知边界：supervisor 与 daemon 同时被杀（如 Windows 重启）时无人重启，直到下次登录或 GUI 启动。

### D2：IPC = loopback WebSocket + 一次性 token

- daemon 监听 `127.0.0.1:0`，把 `{pid, port, token, protocolVersion, startedAt}` 写入 `%LOCALAPPDATA%\sentinel\daemon.json`，文件 ACL 仅当前用户可读。
- 连接握手必须带 token，缺失或错误立即断开；不监听非 loopback 地址。
- 选择理由：Electron 与 Rust 都是一等公民，二进制帧与多路复用直接可用，调试成本低。
- 备选：命名管道（更“Windows 原生”、不出现在 netstat 中，留给后续加固）、stdin/stdout（无法支持多 GUI 实例，排除）。

### D3：SSH 用 russh，一条连接多通道

- 包装 `ssh.exe` 无法区分「认证失败 / 主机密钥 / 网络中断」，而这三种要走不同的状态机分支；也无法在同连接上复用 SFTP。
- 每个主机一条 `russh` 连接，各 pane 开独立 PTY 通道；阶段二/后续 change 在同一连接上开 `sftp` 子系统通道。
- keepalive 默认 30s，连续 3 次失败判定断线。
- 备选：`libssh2`（C 依赖、异步编排差）、`ssh2` crate（通道复用受限）。

### D4：终端渲染交给 xterm.js，daemon 只存字节

- daemon 不解析 VT 序列，只维护原始字节 + 行索引；渲染、搜索、选区、复制全在 xterm.js。
- 代价：回放成本随缓冲增长 → 重连只回放最近 2k 行，更早内容按需读取。

### D5：tmux 作为首版 provider

契约：

```
ensure(sessionRef, cwd) -> AttachHandle
attach(sessionRef)            # 已有则接回
detach(sessionRef)
resize(sessionRef, rows, cols)
snapshot(sessionRef, lines) -> bytes
exists(sessionRef) -> bool
```

- 会话名 `st_<hostId>_<paneUuid>`，确定性生成，重连必然命中同一会话。
- 启动命令：`tmux -u -f <注入配置> new-session -A -s <name> -c <cwd>`。
- 注入配置（独立 `-f` 文件，不读用户 `~/.tmux.conf`）：`status off`（不显示 tmux 状态栏，避免与 UI 重复）、`mouse off`（鼠标事件透传给应用）、`history-limit 100000`、固定 `default-terminal`。
- 重连顺序：`has-session` 判定「接回 / 重建」→ `capture-pane -p -e -S -<N>` 重建历史 → attach 由 tmux 重绘当前屏。重建时注入上次 cwd 并标注“远端进程可能已丢失”。
- tmux 缺失：提示安装命令，或降级为直连 PTY 并持续标注“无持久化”。
- **不用 `-CC` control mode**：需要自行实现 iTerm2 控制协议解析，首版收益不抵成本。
- 备选：自研 helper（阶段二，同接口）、`screen`（维护停滞）、`zellij`（生态不足）。

### D6：重连状态机

```
          connect ok                  link lost
  [idle] -----------> [connected] ----------------> [reconnecting]
    |                   ^   |                              |
    | auth/hostkey err  |   | user closes pane             | backoff 1s..30s cap
    v                   |   v                              |
  [failed]          [reattach] <------ attach ok ----------+
                        |
                        | tmux session gone (host rebooted)
                        v
                  [recreated]  (new session + cwd + "进程可能已丢失")
```

- `failed` 是终态：认证失败、主机密钥不匹配不得进入重试循环，避免被服务器封禁。
- 重连期间用户输入进入 64KB 有界队列；队列满则拒绝新输入并提示，不静默丢弃已排队内容。
- 所有状态变化广播给全部已连接 GUI（多 GUI 共享同一状态）。

### D7：协议与背压

- 控制面：JSON 文本帧，`{v, id, type, payload}`；错误统一 `{code, message, detail}`。
- 数据面：二进制帧 `[u8 type=1][u32 sessionID][u64 seq][bytes]`，每会话 seq 单调递增。
- 背压：daemon 按 16ms 合并同一会话的输出再发送；GUI 侧 socket 积压时丢弃中间帧，GUI 检测到 seq 跳变后请求快照重建，而不是逐字节重传。
- 版本协商：握手交换 `protocolVersion`，不兼容时拒绝并在 GUI 显示双方版本与“重启守护进程”操作。

### D8：滚动缓冲与落盘

- 内存环形 100k 行；按 seq 分段落盘到 `%LOCALAPPDATA%\sentinel\scrollback\<sessionId>\`。
- 每段约 1MB；每 256 行记录一次 `(seq → 文件偏移)` 检查点，5 秒或 1MB 增量持久化索引，支持任意历史位置回读。
- 总量上限 50MB/会话，超出裁剪最旧段；会话销毁删除整个目录；detach 时继续写入。
- 磁盘写入失败（满/只读）时停止落盘、保留内存缓冲并在 UI 警告，不静默降级。
- tmux 模式下 tmux 仍是可见内容的权威源；本地缓冲用于 GUI 重开后的快速滚动与无 tmux 降级模式。

### D9：布局模型

- 分屏存为二叉树：`pane(id, tabId, parentId, direction, ratio, hostId, sessionId)`，还原无损。
- 布局变更走 SQLite 事务；启动只恢复结构、不建连，激活时才连接（惰性恢复）。

### D10：存储、凭据与配置

- `rusqlite`（bundled，WAL）存 host/tab/pane/session；启动时按 `schema_version` 迁移，不可解析时备份原文件后重建。
- 写操作串行化到单个 store task，避免多任务争锁。
- 密码 / passphrase / 粘贴私钥 → Windows Credential Manager（`keyring`），配置只存引用键；日志出口统一脱敏。
- 配置为 `%APPDATA%\sentinel\config.json`，含字体、快捷键、缓冲上限、keepalive、tmux 选项、传输并发等，启动时 schema 校验。

### D11：GUI 运行时结构

- xterm 实例存在 `paneId → Terminal` 的注册表里，用 portal 挂载进 React 树；React 只渲染布局与状态，不持有终端缓冲。
- 终端数据由 main 进程按 16ms 合并后 `webContents.send` 到 renderer（一次 IPC 一批），避免每条输出一次 IPC。
- 未激活的 pane 不创建 xterm 实例；激活时按需建连 + 拉取最近 2k 行。
- WebGL addon 失败自动回退 canvas，不阻塞启动。

### D12：生命周期与升级

- 自启：`HKCU\Software\Microsoft\Windows\CurrentVersion\Run` 指向 `--supervise`；卸载时清理该项与凭据引用。
- 升级：daemon 版本随包升级。GUI 发现协议/版本不兼容时提示“重启守护进程”，daemon 优雅退出（先 detach 全部会话，不 kill tmux），supervisor 拉起新版本，GUI 自动接回。远端会话不因此丢失。

### D13：构建与测试环境

- 开发机为 WSL2（Ubuntu 22.04）：核心 crate 在 Linux 上开发与测试，`sshd` + `tmux` 就在本机，作为真实集成靶机。
- `daemon.exe` 用 `x86_64-pc-windows-gnu` 从 WSL 交叉编译；Build 早期先验证最小二进制在 Windows 上能读写 Credential Manager，若 gnu 工具链受阻则退回 Windows 安装 rustup + MSVC。
- GUI 开发用 WSLg 跑 Electron；打包与真机验收必须在 Windows 11 上完成。
- 协议类型由 Rust 经 `ts-rs` 导出 TS，杜绝两侧手写漂移。

## 数据模型

```
host(id TEXT PK, name, hostname, port, username, authRef, groupName, lastUsedAt)
tab(id TEXT PK, title, sortOrder, createdAt)
pane(id TEXT PK, tabId, parentId, direction, ratio, hostId, sessionId, sortOrder)
session(id TEXT PK, hostId, provider, providerSessionName, cwd, status, lastActiveAt)
schema_version(version INTEGER)
```

`session.provider` 预留 `tmux | agent | none`，阶段二只增加实现，不改表。

## 失败路径

| 失败 | 行为 |
|---|---|
| GUI 崩溃 / 被杀 / 升级 | 会话不受影响，重开即接回 |
| daemon 崩溃 | supervisor 退避重启；重连所有主机；标注恢复结果 |
| supervisor 与 daemon 同时退出 | 下次登录或 GUI 启动时恢复；期间无重连 |
| 认证失败 / 主机密钥不匹配 | 进入 `failed`，不重试，给出可区分的错误 |
| 网络中断 | 指数退避无限重试（上限 30s），保留输入队列 |
| 远端 tmux 会话消失 | 新建会话 + 注入 cwd + 标注进程可能丢失 |
| tmux 未安装 | 提示安装或降级为直连并持续标注 |
| 磁盘满 / 只读 | 停止落盘，保留内存缓冲，UI 警告 |
| SQLite 损坏 | 备份原文件后重建，不覆盖 |
| 协议版本不匹配 | 拒绝连接，提示重启守护进程（不丢远端会话） |
| WS 背压丢帧 | GUI 按 seq 检测缺口并请求快照重建 |

## 测试策略

- **单元**：协议编解码与版本协商、状态机迁移表、环形缓冲回绕、段落索引读写、路径校验、错误映射、配置 schema。
- **集成（WSL）**：本地 `sshd` + `tmux` 真实靶机，覆盖 attach/detach、强杀 SSH 连接、重启 tmux server、`capture-pane` 回放、resize、10MB 吞吐与丢字节检测、并发 10 会话。
- **GUI 冒烟（WSLg + Playwright-Electron）**：启动、快速连接、`echo` 回显、断线黄条与自动恢复、标签/分屏布局持久化。
- **真机验收（Windows 11）**：中文 IME、字体、Credential Manager、自启项、安装/卸载清理、交叉编译产物实跑。

## 风险 / 权衡

- **russh 高吞吐**（`cat` 大文件、日志刷屏）→ Build 早期设定基线 ≥10MB/s 且无丢字节；不达标先调通道读缓冲与批量合并，不轻易换栈。
- **mingw 交叉编译的 Windows API 兼容性** → 早期验证 `keyring` + 单实例互斥 + 文件 ACL；失败即切换 MSVC 路线。
- **WSLg 与真实 Windows 的 IME/字体差异** → 中文与字号回归必须在真机做一遍，不能只信 WSLg。
- **tmux 注入配置压制用户习惯** → 独立 `-f` 配置 + 文档说明 + 可配置项覆盖；不改用户 `~/.tmux.conf`。
- **`capture-pane` 保真度** → 只保证历史文本与可见屏，attach 后触发一次重绘；不承诺全屏程序状态完整重建。
- **daemon 单点** → supervisor + 自动重连 + 如实标注。
- **多 GUI 并发写布局** → store 串行化 + 版本号，GUI 以 daemon 状态为准，不各自本地写。

## 迁移与发布

全新项目，无数据迁移。发布形态：便携 zip + 安装包；daemon、supervisor 与 GUI 同版本打包；首次启动写自启项，卸载清理自启项与凭据引用。

## 已收敛的细节

- 崩溃恢复后的标注只保留一种文案「远端进程可能已丢失」，不区分「确认丢失」与「未知」，避免要求系统判断它无法证明的事实。
- 「全部重连」入口固定在侧边栏后台会话分组底部，并提供 `Ctrl+Shift+R` 与命令面板入口。
- 开发与验证环境：核心 crate 在 WSL 开发并打本地 sshd+tmux；GUI 在 WSLg 冒烟、Windows 11 真机验收；`daemon.exe` 交叉编译产出。
