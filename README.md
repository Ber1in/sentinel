# Sentinel

一个 Windows 11 自用的 SSH 终端工作台。它要解决三件事：

1. **关掉窗口不丢会话**：SSH 连接与终端缓冲由常驻守护进程持有，GUI 只是视图，随时可以关掉、崩溃、重开。
2. **断线无感恢复**：网络抖动或 SSH 被掐断后自动重连，远端 tmux 会话继续跑。
3. **一个窗口连多台机器**：标签与分屏可以来自任意主机，不需要切换 workspace。

服务器真的断电重启时，运行中的进程无法复活（任何方案都做不到）。此时系统恢复布局、工作目录、历史与滚动缓冲，并明确标注原进程已丢失。

## 当前状态

正在实施 `persistent-ssh-terminal-core`。目前完成的是工程骨架：Cargo workspace、Electron 桌面壳、协议类型导出管线、CI 与本地检查脚本。守护进程、SSH、持久化与 UI 功能按 `docs/openspec/changes/persistent-ssh-terminal-core/tasks.md` 推进。

## 架构

```
supervisor (同名二进制 --supervise)   ← 登录自启, 崩溃重启
  └─ daemon                          ← 持有 SSH / tmux 会话 / 滚动缓冲 / 布局
       └─ WebSocket 127.0.0.1 + token
            ├─ GUI 实例 1
            └─ GUI 实例 2（共享同一批会话）
```

- `daemon` 是唯一持有连接的一方；GUI 关闭不影响会话。
- `supervisor` 负责在守护进程异常退出后拉起它；Windows 重启后由登录自启恢复。
- 远端持久化由 `PersistenceProvider` 抽象，首版实现是 tmux。

详细设计见 `docs/openspec/changes/persistent-ssh-terminal-core/design.md`。

## 目录结构

```
crates/protocol      # 协议类型（Rust 单一事实来源，导出 TS）
crates/daemon        # 守护进程入口与 IPC server
crates/ssh           # russh 封装：认证、通道复用、keepalive
crates/persistence   # PersistenceProvider 与 tmux 实现
crates/terminal      # 环形缓冲与滚动缓冲落盘
crates/store         # SQLite 状态存储
apps/desktop         # Electron main / preload / React renderer
scripts/             # 本地检查与代码生成
docs/                # OpenSpec 变更产物与实施计划
```

## 环境要求

- 开发：WSL2（Ubuntu 22.04）或 Windows 11；Rust stable、Node 22、pnpm 9
- 目标：Windows 11 x64
- 被连的 Linux 服务器：OpenSSH server，tmux ≥ 3.0（用于远端会话保持）

## 构建与运行

```bash
# 1. 生成协议 TypeScript 绑定（改过 crates/protocol 后必须重跑）
./scripts/gen-protocol.sh

# 2. Rust 测试
cargo test --workspace

# 3. 安装桌面依赖
pnpm --dir apps/desktop install

# 4. 启动桌面（WSL 或 root 下需要关闭 sandbox）
ELECTRON_DISABLE_SANDBOX=1 pnpm --dir apps/desktop dev

# 5. 生产构建
pnpm --dir apps/desktop build
```

一键跑完全部检查：

```bash
./scripts/check.sh
```

## 交叉编译 Windows 版守护进程

```bash
rustup target add x86_64-pc-windows-gnu
sudo apt-get install -y mingw-w64
cargo build --release --target x86_64-pc-windows-gnu -p daemon
```

产物在 `target/x86_64-pc-windows-gnu/release/daemon.exe`。若 `keyring` 等 Windows API 在 gnu 工具链下不兼容，改用 Windows 上的 rustup + MSVC 工具链构建同一份代码。

## 文档索引

| 文档 | 内容 |
| --- | --- |
| `docs/openspec/changes/persistent-ssh-terminal-core/proposal.md` | 目标、范围、非目标 |
| `docs/openspec/changes/persistent-ssh-terminal-core/specs/` | 行为契约与验收场景 |
| `docs/openspec/changes/persistent-ssh-terminal-core/design.md` | 技术设计与失败路径 |
| `docs/openspec/changes/persistent-ssh-terminal-core/tasks.md` | 任务清单与验收方式 |
| `docs/superpowers/plans/` | 实施计划 |
| `docs/openspec/changes/sftp-file-panel/` | 第二阶段：SFTP 面板 |

## 许可

MIT。

## 桌面客户端

界面为深色优先的终端工作台：左侧主机列表、标签条、状态胶囊，`Ctrl+K` 命令面板，支持简体中文与英文、深色/浅色主题。

```bash
pnpm --dir apps/desktop dev      # 开发（WSL 下需要 ELECTRON_DISABLE_SANDBOX=1）
pnpm --dir apps/desktop dist:win # 产出 Windows 安装包与便携版到 apps/desktop/release
```

当前构建产物位于 `/mnt/d/Downloads/Sentinel Setup 0.1.0.exe`（安装包）与 `/mnt/d/Downloads/Sentinel 0.1.0.exe`（便携版）。安装包内置 `daemon.exe`，首次启动会由 GUI 自动拉起。
