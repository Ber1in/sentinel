# Comet Design Handoff

- Change: persistent-ssh-terminal-core
- Phase: design
- Mode: compact
- Context hash: 5b9745fb73f725a2af63838b105de20ca670ee4197cfa6bbf9acb3c4115479ff

Generated-by: comet-handoff.sh
Task hash policy: task-content-v1. Read tasks.md for live completion; excerpts are design-time context.

OpenSpec remains the canonical capability spec. This handoff is a deterministic, source-traceable context pack, not an agent-authored summary.

## docs/openspec/changes/persistent-ssh-terminal-core/proposal.md

- Source: docs/openspec/changes/persistent-ssh-terminal-core/proposal.md
- Lines: 1-39
- SHA256: 895fed8b16cacd2df7be1c3d7ba6afb439631f44799dac2ab5c31ba0d67c4885

```md
# 提案：持久化跨主机 SSH 终端核心

## 为什么

现有 Windows SSH 客户端把会话生命周期绑在窗口和 TCP 连接上：关掉窗口、客户端崩溃、网络闪断，正在跑的命令就一起没了；换一台机器还要切换 workspace，无法在一个窗口里同时盯着多台服务器。tty7 解决了「窗口关了 shell 还在」，但它的远端 workspace 模型把主机和工作区绑死，跨主机标签/分屏做不到。

本变更建立产品的核心：一个本地守护进程持有 SSH 连接与远端 tmux 会话，GUI 只是可随时关闭、崩溃、重开的视图；一个窗口内任意标签、任意分屏可以来自任意主机。

## 变更内容

- 新增本地守护进程：持有 SSH 连接、远端 tmux 会话绑定、滚动缓冲、布局状态；GUI 退出、被杀、升级均不影响会话
- 新增 GUI 客户端（Electron + React + xterm.js）：跨主机标签、跨主机横向/纵向分屏、侧边栏主机分组与后台会话、快速连接
- 新增本地 IPC 协议：GUI 与守护进程之间的会话控制、终端数据流、状态事件、重连通知
- 新增 SSH 层：密码 / 私钥（含 passphrase）/ keyboard-interactive / Windows OpenSSH agent 认证，连接复用与断线重连状态机
- 新增 `PersistenceProvider` 抽象，首版实现 `tmux` provider：远端会话的 new/attach/detach/resize/滚动缓冲回放
- 新增会话语义：关闭标签 = detach（远端保留），侧边栏可见并可接回；显式结束才销毁；30 天未访问提示清理
- 新增状态持久化：SQLite 保存主机、标签、布局、会话索引；每会话滚动缓冲落盘（默认 100k 行 / 50MB 环形）；凭据存 Windows Credential Manager
- 新增守护进程生命周期：登录自启、单实例、GUI 缺失时自动拉起、崩溃后自动重启并重连

## 能力

### 新增能力

- `terminal-session-persistence`：会话的持有、脱离、接回、销毁与清理；守护进程与 GUI 的解耦
- `ssh-connection-management`：认证方式、连接建立、断线检测与自动重连、远端持久化 provider 契约
- `multi-host-workspace`：跨主机标签与分屏的数据模型、布局持久化、惰性恢复
- `terminal-emulation`：终端渲染与输入、滚动缓冲、搜索、尺寸同步
- `credential-and-config-storage`：凭据存储、配置读取、状态数据库

### 修改能力

无。这是全新项目，尚无既有 spec。

## 影响

- 全新代码库，无既有代码受影响；本变更确立仓库结构（Rust workspace + Electron 应用 + 共享协议定义）
- 外部依赖：远端 Linux 需 OpenSSH server 与 tmux；Windows 11 需 WebView2 以外的运行时（Electron 自带）
- 运行要求：Windows 11 x64；远端 tmux ≥ 3.0
- 明确不在本变更内：SFTP 面板（独立 change `sftp-file-panel`）、远端 helper provider（阶段二）、端口转发、跳板机、本地 shell/WSL

```

## docs/openspec/changes/persistent-ssh-terminal-core/design.md

- Source: docs/openspec/changes/persistent-ssh-terminal-core/design.md
- Lines: 1-229
- SHA256: 2581fac974da041af28d30daa76529feda05eff517bb6166ffcd5e55ddfdaee1

[TRUNCATED]

```md
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

- daemon 监听 `127.0.0.1:0`，把 `{pid, port, token, protocolVersion, startedAt}` 写入 `%LOCALAPPDATA%\sshTerminal\daemon.json`，文件 ACL 仅当前用户可读。
- 连接握手必须带 token，缺失或错误立即断开；不监听非 loopback 地址。
- 选择理由：Electron 与 Rust 都是一等公民，二进制帧与多路复用直接可用，调试成本低。
- 备选：命名管道（更“Windows 原生”、不出现在 netstat 中，留给后续加固）、stdin/stdout（无法支持多 GUI 实例，排除）。

### D3：SSH 用 russh，一条连接多通道

- 包装 `ssh.exe` 无法区分「认证失败 / 主机密钥 / 网络中断」，而这三种要走不同的状态机分支；也无法在同连接上复用 SFTP。

```

Full source: docs/openspec/changes/persistent-ssh-terminal-core/design.md

## docs/openspec/changes/persistent-ssh-terminal-core/tasks.md

- Source: docs/openspec/changes/persistent-ssh-terminal-core/tasks.md
- Lines: 1-86
- SHA256: bc96c8da4ce9e52c1504c18b761f5cd0783e71323e34eea59f5f3307267ebe67

[TRUNCATED]

```md
# 任务

## 1. 仓库与构建基线

- [ ] 1.1 `git init` 并提交 `.gitignore`（Rust `target/`、Node `node_modules/`、`dist/`、构建产物），验证：`git status` 干净且 `git log` 有一条初始提交
- [ ] 1.2 建立 Cargo workspace 与 `crates/{protocol,daemon,ssh,persistence,terminal,store}` 骨架，每个 crate 一个最小可编译入口，验证：`cargo check --workspace` 通过
- [ ] 1.3 建立 `apps/desktop`（Electron + React + Vite + TypeScript）骨架，能启动空白窗口，验证：`pnpm build` 与 `pnpm dev` 均可启动
- [ ] 1.4 建立协议类型导出管线：`crates/protocol` 为单一事实来源，构建期生成 TS 类型文件，验证：新增一个消息类型后重新生成，TS 侧出现对应定义且类型检查通过
- [ ] 1.5 配置 CI：在 Linux 上跑 `cargo test --workspace` 与前端 `pnpm build`，验证：等价命令在本地全部通过，workflow 文件语法校验通过
- [ ] 1.6 写 `README.md` 的架构概览、目录说明、构建与运行步骤、远端 tmux 版本要求，验证：按 README 从空环境走一遍能启动空白窗口

## 2. 协议与 IPC

- [ ] 2.1 定义控制消息（会话创建/关闭/接回/结束、resize、状态事件、错误）与二进制终端帧，含版本字段，验证：round-trip 单测覆盖全部消息类型
- [ ] 2.2 实现守护进程单实例互斥 + loopback WebSocket 监听 + 一次性 token 文件（限制 ACL），验证：第二个实例启动被拒绝且退出码非零；无 token 的连接被断开
- [ ] 2.3 Electron main 作为唯一 broker 连接 daemon，renderer 经 preload 暴露的最小 API 通讯，验证：跨端 ping/echo 与一次会话列表拉取成功
- [ ] 2.4 实现协议版本协商：版本不兼容时拒绝连接并给出可读提示，验证：人为降低客户端版本时被拒且提示包含双方版本
- [ ] 2.5 在 `docs/protocol.md` 记录消息表与帧格式，验证：文档中的示例帧能被单测解析

## 3. SSH 连接层

- [ ] 3.1 实现四种认证（密码、私钥含 passphrase、keyboard-interactive、Windows OpenSSH agent）并映射错误类别（凭据/主机密钥/网络/超时），验证：错误映射单测 + 对真实主机各跑通一次
- [ ] 3.2 实现主机密钥指纹展示、确认与记忆，验证：指纹变更时连接被拒且告警，单测覆盖首次/匹配/不匹配三种路径
- [ ] 3.3 实现可配置 keepalive，验证：单测确认间隔与超时参数被正确应用，并在真实连接上观察空闲 30 分钟不断线
- [ ] 3.4 实现单连接多通道（PTY 通道 + 预留 SFTP 子系统通道），验证：同一主机同时打开终端与测试用子系统通道时 TCP 连接数仍为 1
- [ ] 3.5 吞吐基线 spike：用 `cat` 输出 10MB 与高频日志刷屏，验证：≥10MB/s、缓冲回放无丢字节，把结果与优化结论写入 `docs/perf.md`

## 4. 持久化 provider（tmux）

- [ ] 4.1 定义 `PersistenceProvider` trait 与确定性会话名生成，验证：同名生成幂等、不同标签名冲突的单测
- [ ] 4.2 实现 tmux provider 的 ensure/attach/detach/resize，验证：真实主机上关闭客户端后 `tmux ls` 仍可见会话，重开后接回
- [ ] 4.3 实现 `capture-pane` 快照回放并在回放后触发重绘，验证：`vim`/`htop` 重连后画面可用；普通输出颜色抽查一致
- [ ] 4.4 实现 tmux 缺失检测、安装提示与「无持久化」降级模式，验证：无 tmux 的容器中触发提示，选择降级后界面持续标注无持久化
- [ ] 4.5 在 `docs/providers.md` 记录 provider 契约、tmux 参数选择（`-f` 最小配置、`-u`）与降级语义，验证：文档参数与代码常量一致

## 5. 重连状态机

- [ ] 5.1 实现状态机（connecting/connected/reconnecting/detached/lost/failed）与指数退避，认证类错误立即终止，验证：状态迁移表单测全覆盖
- [ ] 5.2 实现重连期间的本地输入队列，验证：断线时输入半行、恢复后内容仍在且可继续编辑
- [ ] 5.3 实现重连后 reattach + 缓冲回放，验证：断网 30 秒后日志连续、无重复无丢失（集成测试比对字节数）
- [ ] 5.4 实现「会话已不存在」分支：新建会话、注入上次 cwd、标注进程可能丢失，验证：重启目标主机后走通该路径且界面文案正确

## 6. 终端缓冲

- [ ] 6.1 实现内存环形缓冲（默认 100k 行上限，可配置），验证：单测覆盖回绕与行数上限
- [ ] 6.2 实现增量落盘、50MB 上限裁剪与会话销毁时删除，验证：单测 + 检查磁盘文件大小与存活周期
- [ ] 6.3 实现 attach 时回放最近 2k 行与向上按需加载更早片段，验证：输出 20k 行后重开 GUI 可上翻到最早内容
- [ ] 6.4 实现回放顺序与去重保证（带序号），验证：模拟 GUI 卡顿 5 秒后回放序列完整且无重复

## 7. 状态存储与凭据

- [ ] 7.1 实现 SQLite schema（host/tab/pane/session）与版本化迁移，验证：迁移单测可从空库升级到当前版本
- [ ] 7.2 实现主机、标签、分屏树、会话的仓储与事务写入，验证：写入过程中被杀后重开，数据为上一次完整版本
- [ ] 7.3 实现 Windows Credential Manager 存取（密码、passphrase、粘贴私钥），验证：可读写；配置文件与日志扫描不到明文
- [ ] 7.4 实现配置 JSON 读取与 schema 校验（字体、快捷键、缓冲上限、tmux 选项、keepalive），验证：手改字体与快捷键后重启生效，非法字段给出可读错误
- [ ] 7.5 实现损坏检测与「备份后重建」流程，验证：截断状态库后启动给出提示，确认后生成新库且原文件被保留为备份
- [ ] 7.6 在 `docs/configuration.md` 记录全部配置字段与默认值，验证：文档字段与配置 schema 单测一致

## 8. GUI：会话与布局

- [ ] 8.1 实现终端 pane 组件（xterm.js + WebGL + unicode11 + 搜索 + 链接），验证：中文/emoji 混排样例、`vim` 改尺寸重绘、搜索高亮三项手工通过
- [ ] 8.2 实现标签与跨主机横向/纵向分屏（含拖拽与尺寸持久化），验证：验收场景 1 手工通过
- [ ] 8.3 实现侧边栏（主机分组、最近连接、后台会话、30 天清理提示、「全部重连」入口），验证：验收场景 4 手工通过
- [ ] 8.4 实现快速连接 `user@host[:port]` 并保存为主机，验证：输入后连接成功且主机出现在侧边栏，重启后仍在
- [ ] 8.5 实现会话状态指示与重连提示条，验证：拔网线后 5 秒内出现重连中，恢复后自动消失
- [ ] 8.6 实现启动惰性恢复与「全部重连」，验证：10 个标签重启后流量抓包显示 0 条 SSH 连接，点击标签才建立
- [ ] 8.7 实现 shell 集成注入（OSC 7 / OSC 133）与 cwd 记录，验证：`cd` 后会话记录目录变化；注入失败时会话仍可正常使用
- [ ] 8.8 实现复制粘贴与 bracketed paste，验证：三行命令一次粘贴后不产生分段执行回显

## 9. 守护进程生命周期

- [ ] 9.1 实现登录自启与 GUI 侧自动拉起，验证：注销重登后守护进程在运行；GUI 在守护进程缺失时能拉起且只产生一个实例
- [ ] 9.2 实现看门狗：守护进程崩溃后自动重启并重连全部可恢复会话，验证：任务管理器结束守护进程后 10 秒内恢复且会话标注正确
- [ ] 9.3 实现按需重启守护进程（升级场景）且不丢远端会话，验证：重启守护进程后所有标签自动接回，无远端会话被销毁
- [ ] 9.4 在 `docs/troubleshooting.md` 记录日志位置、协议版本查看方式、守护进程重启方法与凭据清理步骤，验证：按文档可定位一次人为制造的连接失败

## 10. 集成与交付

- [ ] 10.1 端到端跑通 proposal 中的 6 条验收场景，输出 `docs/reports/acceptance.md` 记录每条的实测结果与截图/日志，验证：6 条全部通过或明确记录失败原因
- [ ] 10.2 采集资源基线（冷启动内存、10 会话 CPU、回放延迟）写入 `docs/perf.md`，验证：数据可复现，脚本随仓库提交

```

Full source: docs/openspec/changes/persistent-ssh-terminal-core/tasks.md

## docs/openspec/changes/persistent-ssh-terminal-core/.openspec.yaml

- Source: docs/openspec/changes/persistent-ssh-terminal-core/.openspec.yaml
- Lines: 1-2
- SHA256: 71e91d8f42848565b00ec7bb4da95cf19e2182f4adc440c230b10a0f3345023b

```md
schema: spec-driven
created: 2026-11-02

```

## docs/openspec/changes/persistent-ssh-terminal-core/specs/credential-and-config-storage/spec.md

- Source: docs/openspec/changes/persistent-ssh-terminal-core/specs/credential-and-config-storage/spec.md
- Lines: 1-52
- SHA256: ba9ae32d180f52f92e7c153078dee6c53c33b0e805f52c8719d6f379b4b598aa

```md
# Spec Delta

## Purpose

安全保存登录凭据与用户配置，并在重启后可靠恢复主机、标签、布局与会话元数据。

## ADDED Requirements

### Requirement: 凭据不落明文

密码与 passphrase MUST 存储在 Windows Credential Manager，MUST NOT 以明文写入配置文件、状态数据库、日志或崩溃转储。

#### Scenario: 配置与日志中无明文密码

- **WHEN** 用户保存一个带密码的主机后检查配置目录与日志文件
- **THEN** 文件内容中不出现该密码，凭据仅存在于 Credential Manager

### Requirement: 私钥按引用保存

用户选择私钥文件时系统 MUST 只保存路径；用户直接粘贴的私钥内容 MUST 存入凭据库而非配置文件。

#### Scenario: 私钥路径与内容分别存放

- **WHEN** 用户分别通过文件选择和粘贴方式添加私钥
- **THEN** 前者在配置中只有路径，后者在配置中无内容且凭据库中存在加密条目

### Requirement: 配置可手工编辑

系统 MUST 把用户配置保存为 `%APPDATA%` 下的 JSON 文件，至少覆盖默认 shell、字体、快捷键、滚动缓冲上限与 tmux 选项。手工修改后 MUST 在下次启动生效。

#### Scenario: 手工修改字体生效

- **WHEN** 用户在应用退出状态下修改配置文件中的字体与字号并重新启动
- **THEN** 新字体与字号在新会话中生效

### Requirement: 状态库原子写入

主机、标签、布局与会话索引 MUST 存入本地 SQLite，写入 MUST 具备原子性，进程异常退出 MUST NOT 留下半写状态。

#### Scenario: 写入过程中断不损坏

- **WHEN** 守护进程在更新布局时被强制终止
- **THEN** 重启后状态库可正常打开，布局为上一次完整写入的版本

### Requirement: 版本校验与损坏恢复

系统 MUST 在启动时校验状态库与配置的 schema 版本。发现不可解析的损坏时 MUST 报错并提供备份后重建的选项，MUST NOT 静默丢弃数据。

#### Scenario: 状态库损坏时给出可恢复路径

- **WHEN** 状态库文件被截断导致无法解析
- **THEN** 系统提示损坏、保留原文件备份，并在用户确认后重建空库，不直接覆盖原文件

```

## docs/openspec/changes/persistent-ssh-terminal-core/specs/multi-host-workspace/spec.md

- Source: docs/openspec/changes/persistent-ssh-terminal-core/specs/multi-host-workspace/spec.md
- Lines: 1-70
- SHA256: 6c287efd657e91608519d5142c0661fea2f6f1c895e3c32f9156bc02adaa3d70

```md
# Spec Delta

## Purpose

在单一窗口内组织来自多台主机的持久会话，使标签与分屏不受主机边界限制，并可跨重启恢复。

## ADDED Requirements

### Requirement: 标签可绑定任意主机

同一窗口内 MUST 允许存在绑定到不同主机的标签，标签数量与主机数量 MUST NOT 互相限制。

#### Scenario: 一个窗口内连接两台主机

- **WHEN** 用户打开标签 1 连接主机 A、标签 2 连接主机 B
- **THEN** 两个标签同时存在且各自保持独立连接，切换标签不重建连接

### Requirement: 分屏可跨主机

系统 MUST 支持横向与纵向分屏，且同一标签内的每个分屏 MUST 可以绑定不同主机的会话。

#### Scenario: 左右分屏同时操作两台机器

- **WHEN** 用户创建左右分屏并分别绑定主机 A 与主机 B
- **THEN** 两侧可独立输入输出、独立调整尺寸，互不干扰

### Requirement: 布局持久化

系统 MUST 持久化标签顺序、分屏树结构、各分屏尺寸以及每个分屏绑定的主机与会话标识，MUST 在重启后按原样恢复。

#### Scenario: 重启后布局一致

- **WHEN** 用户建立包含两个标签、其中一个是三分屏的布局后重启应用
- **THEN** 标签顺序、分屏方向、相对尺寸与各分屏所属主机与重启前一致

### Requirement: 默认惰性恢复

启动时系统 MUST 恢复布局与标签，但 MUST NOT 自动为全部标签建立连接；连接 MUST 在标签或分屏被激活时才建立，且 MUST 提供"全部重连"的显式操作。

#### Scenario: 十个标签不触发十次握手

- **WHEN** 用户上次打开了 10 个标签后重启应用
- **THEN** 启动后仅恢复界面结构，不产生 SSH 连接，直到用户点击某个标签

### Requirement: 侧边栏组织主机与会话

系统 MUST 提供侧边栏，按主机分组展示已保存主机、最近连接与后台会话。在 10 台主机的规模内 MUST NOT 要求全文搜索即可定位主机。

#### Scenario: 从侧边栏接回后台会话

- **WHEN** 用户展开侧边栏的后台会话分组
- **THEN** 可看到按最后活动时间排序的会话并点击接回

### Requirement: 快速连接

系统 MUST 支持输入 `user@host[:port]` 直接建立会话，并 MUST 把该连接保存为可复用主机条目。

#### Scenario: 快速连接后主机被记住

- **WHEN** 用户使用快速连接输入 `root@10.0.0.5:2222` 并成功登录
- **THEN** 该主机出现在侧边栏主机列表中，下次可直接选择

### Requirement: 界面操作不影响数据流

新建、关闭、拖拽或调整标签与分屏时，其他会话的终端数据流 MUST NOT 中断或丢字节。

#### Scenario: 拖拽标签期间其他会话继续输出

- **WHEN** 用户拖拽调整标签顺序，同时另一标签正在输出大量日志
- **THEN** 该标签的输出保持连续完整，不出现截断或乱序

```

## docs/openspec/changes/persistent-ssh-terminal-core/specs/ssh-connection-management/spec.md

- Source: docs/openspec/changes/persistent-ssh-terminal-core/specs/ssh-connection-management/spec.md
- Lines: 1-79
- SHA256: 320d7abf67c6eeff83cd1d4463fde960ae91db998984c148eb9088244c74e97b

```md
# Spec Delta

## Purpose

建立并维持到 Linux 主机的 SSH 连接，在连接中断时自动恢复，并通过远端持久化机制保住用户正在运行的进程。

## ADDED Requirements

### Requirement: 支持四种认证方式

系统 MUST 支持密码、私钥（含 passphrase）、keyboard-interactive 与 Windows OpenSSH agent 四种认证。认证失败 MUST 返回可区分的错误类别：凭据错误、主机密钥问题、网络不可达、超时。

#### Scenario: 密码错误的提示可区分

- **WHEN** 用户使用错误密码连接主机
- **THEN** 界面显示"认证失败"，而非"网络不可达"，且不进入自动重连循环

### Requirement: 主机密钥校验

首次连接某主机时 MUST 显示主机密钥指纹并请求确认，确认后 MUST 记住。指纹与记录不一致时 MUST 阻止连接并告警。

#### Scenario: 指纹变化时拒绝连接

- **WHEN** 用户连接一台此前记录过、但当前指纹与记录不符的主机
- **THEN** 系统拒绝建立会话并显示指纹不一致告警，用户需显式清除记录后才能继续

### Requirement: 断线自动重连

连接意外中断时系统 MUST 以指数退避自动重连，MUST NOT 要求用户手动点击。重连成功后 MUST 自动重新接回远端持久化会话。

#### Scenario: 网络闪断后自动恢复

- **WHEN** 网络中断 30 秒后恢复，期间某会话正在运行 `tail -f`
- **THEN** 无需用户操作，会话自动重连并接回，离开期间的日志内容按序出现

### Requirement: 重连期间保留未提交输入

重连期间用户键入但尚未回车的内容 MUST 被保留，重连成功后 MUST 继续可用。

#### Scenario: 断线时输入半行命令

- **WHEN** 会话断线时用户已输入半条命令但未回车，随后网络恢复
- **THEN** 该半行命令仍显示在提示符处，用户可继续编辑并执行

### Requirement: 远端持久化 provider 契约

系统 MUST 通过可替换的 provider 抽象管理远端持久化，首版 MUST 提供 tmux 实现，支持会话创建、接回、脱离、尺寸调整与缓冲回放。会话名 MUST 由主机与标签确定性生成，同一会话不得被两个连接重复创建。

#### Scenario: 同一会话不被重复创建

- **WHEN** 用户关闭标签后立即从后台列表接回同一会话
- **THEN** 远端已存在的 tmux 会话被复用，不会创建第二个同名会话

### Requirement: tmux 缺失时明确降级

远端不存在 tmux 时，系统 MUST 明确告知并停止假装持久化，MUST 提供两种处置：按提示安装 tmux 后重试，或以"无持久化"模式继续并在界面上持续标注。

#### Scenario: 无 tmux 主机的连接提示

- **WHEN** 用户连接一台未安装 tmux 的主机
- **THEN** 界面提示缺少 tmux 与安装命令，并让用户选择重试或降级继续

### Requirement: 服务器重启后的恢复语义

远端会话因主机重启而消失时，系统 MUST 自动新建会话、恢复上次工作目录与布局，并 MUST 明确告知原运行进程已丢失。

#### Scenario: 主机重启后恢复目录但不恢复进程

- **WHEN** 用户在 `/var/log` 下运行 `tail -f syslog`，主机重启后系统重连
- **THEN** 会话在新 shell 中打开并位于 `/var/log`，界面提示原进程已丢失，且不声称已恢复该进程

### Requirement: 连接保活

系统 MUST 以可配置的间隔发送保活探测，使空闲会话不因服务端或中间设备超时被断开。

#### Scenario: 空闲会话不被断开

- **WHEN** 某会话连续 2 小时无用户输入且中间网络设备存在 NAT 超时
- **THEN** 会话保持 connected 状态，不出现断线

```

## docs/openspec/changes/persistent-ssh-terminal-core/specs/terminal-emulation/spec.md

- Source: docs/openspec/changes/persistent-ssh-terminal-core/specs/terminal-emulation/spec.md
- Lines: 1-70
- SHA256: f48f1a45a1128bd6a05bcaf149517763ff8e52a9b9fa1bb6ae8963e9ac18ad3b

```md
# Spec Delta

## Purpose

正确渲染远端终端输出并处理用户输入，并在 GUI 重开或重连后保留可回溯的会话历史。

## ADDED Requirements

### Requirement: 终端渲染能力

系统 MUST 正确渲染真彩色与 256 色、光标形状、备用屏幕切换、鼠标上报与 bracketed paste，MUST 正确处理中文等宽字符与输入法组合输入。

#### Scenario: 中文输入法正常上屏

- **WHEN** 用户在会话中使用中文输入法输入并上屏多字词
- **THEN** 字符顺序与宽度正确，不出现重复字符、半角错位或光标位置错误

### Requirement: 尺寸同步

窗口或分屏尺寸变化时，系统 MUST 把新的行列数同步到远端 PTY，使全屏程序按新尺寸重绘。

#### Scenario: 调整窗口后 vim 重绘正确

- **WHEN** 用户在 `vim` 中拖动窗口边缘改变尺寸
- **THEN** `vim` 按新尺寸重绘，底部状态行位置正确，无残影

### Requirement: 滚动缓冲持久化

每个会话 MUST 在本地保留滚动缓冲，默认上限 100,000 行且磁盘占用不超过 50MB，超出后按环形覆盖。GUI 重开后 MUST 恢复可滚动内容。

#### Scenario: 重开 GUI 后仍能向上翻历史

- **WHEN** 会话输出 20,000 行后用户关闭并重新打开 GUI
- **THEN** 该会话可向上滚动查看此前的输出，行内容与关闭前一致

### Requirement: 会话内搜索

系统 MUST 支持在当前会话的滚动缓冲内搜索并高亮匹配项，MUST 支持在匹配项之间跳转。

#### Scenario: 搜索历史输出

- **WHEN** 用户在某会话中搜索 `error`
- **THEN** 所有匹配被高亮，用户可在匹配之间前后跳转

### Requirement: 复制粘贴

系统 MUST 提供复制与粘贴快捷键；多行粘贴 MUST 通过 bracketed paste 发送，避免被远端误判为逐条执行。

#### Scenario: 多行粘贴不逐行执行

- **WHEN** 用户把三行命令一次性粘贴到 bash 提示符
- **THEN** 三行作为整体粘贴，不产生三段独立的执行回显

### Requirement: shell 集成上报工作目录

系统 MUST 通过 shell 集成识别当前工作目录与命令边界，用于会话恢复与后续文件面板定位；集成 MUST 在无法注入时不影响会话可用性。

#### Scenario: 识别到目录变化

- **WHEN** 用户在会话中执行 `cd /etc/nginx`
- **THEN** 系统记录当前目录为 `/etc/nginx`，并在会话恢复时使用该目录

### Requirement: 输出缓冲不丢字节

GUI 短暂卡顿、重连或重开期间，守护进程 MUST 缓冲终端输出，GUI 恢复后 MUST 按原顺序完整回放。

#### Scenario: 卡顿期间的输出不丢

- **WHEN** GUI 因渲染压力卡顿 5 秒，期间远端持续输出日志
- **THEN** 卡顿结束后所有输出按原顺序出现，无缺失与重复

```

## docs/openspec/changes/persistent-ssh-terminal-core/specs/terminal-session-persistence/spec.md

- Source: docs/openspec/changes/persistent-ssh-terminal-core/specs/terminal-session-persistence/spec.md
- Lines: 1-70
- SHA256: 4dcc6dd9d31daf941a6ebfcb6744d6169bf4342106d4daf87e2d5e61c41c484d

```md
# Spec Delta

## Purpose

让终端会话的生命周期脱离 GUI 窗口存活：用户关闭、崩溃、重开客户端都不中断远端工作。

## ADDED Requirements

### Requirement: 会话由守护进程持有

系统 MUST 由一个常驻守护进程持有 SSH 连接、远端会话绑定与终端缓冲。GUI 退出、被杀或崩溃 MUST NOT 终止 SSH 连接或远端进程。

#### Scenario: 强制结束 GUI 后会话仍在

- **WHEN** 用户在任务管理器中结束 GUI 进程，随后重新启动 GUI
- **THEN** 守护进程报告该会话仍处于存活状态，GUI 接回后恢复画面与滚动缓冲，远端正在运行的进程继续存在

### Requirement: 关闭标签即脱离而非销毁

关闭标签 MUST 只把会话从界面脱离（detach），远端会话与缓冲继续存在。只有显式执行"结束会话"才 MUST 销毁远端会话并删除本地缓冲。

#### Scenario: 关闭标签后从后台列表接回

- **WHEN** 用户关闭一个正在运行 `top` 的标签，随后从侧边栏后台会话列表点击该会话
- **THEN** 会话被重新接回，`top` 仍在运行，界面显示离开期间产生的输出

### Requirement: 后台会话可见且可管理

系统 SHALL 在侧边栏列出全部已脱离会话，并显示主机名、标签名、最后活动时间与最后一行输出摘要。

#### Scenario: 后台会话列表内容正确

- **WHEN** 用户已 detach 三个不同主机的会话
- **THEN** 侧边栏后台会话区显示三条记录，且主机名与最后活动时间的排序与真实情况一致

### Requirement: 长期未访问会话需确认后清理

系统 MUST 在会话超过 30 天未被访问时提示清理，MUST NOT 未经用户确认自动销毁远端会话。

#### Scenario: 提示清理但不自动删除

- **WHEN** 某后台会话最后活动时间超过 30 天且用户打开应用
- **THEN** 系统显示清理提示，用户拒绝后会话保持不变

### Requirement: 守护进程崩溃后可恢复

守护进程异常退出后 MUST 自动重启，重新载入会话表并对仍可连接的会话发起重连。重连成功的会话 MUST 被明确标注其远端进程可能已丢失。

#### Scenario: 守护进程被杀后自动恢复

- **WHEN** 守护进程被外部强制终止
- **THEN** 系统在无人干预下重新启动守护进程，重连全部可通过 tmux 接回的会话，并在界面上标注恢复结果

### Requirement: 守护进程单实例与自动拉起

系统 MUST 保证守护进程单实例运行；GUI 启动时若发现守护进程不存在 MUST 自动拉起，若已存在 MUST 复用同一实例。

#### Scenario: 重复启动不产生第二个守护进程

- **WHEN** 用户连续启动 GUI 两次
- **THEN** 系统中只有一个守护进程实例，两个 GUI 窗口共享同一批会话

### Requirement: 会话状态实时可见

系统 MUST 向 GUI 推送会话状态变化，至少包含 connecting、connected、reconnecting、detached、lost 五种状态。

#### Scenario: 断网时状态变化可见

- **WHEN** 到某主机的网络中断
- **THEN** 对应标签在界面显示 reconnecting 状态，网络恢复后自动变为 connected，无需用户操作

```
