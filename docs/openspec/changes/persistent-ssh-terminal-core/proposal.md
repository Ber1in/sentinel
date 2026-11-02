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
