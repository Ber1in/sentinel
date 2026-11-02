# 持久化 provider

远端进程只能由远端保住。provider 负责三件事：绑定或重建远端会话、接回、取回可见历史。契约定义在 `crates/persistence/src/lib.rs` 的 `PersistenceProvider`。

## 契约

| 方法 | 语义 |
| --- | --- |
| `kind()` | 上报给 GUI 的 `tmux` / `none` |
| `session_name(host_id, pane_id)` | 确定性命名，重连必须能重新算出同一个名字 |
| `attach_command(name, cwd)` | 在 PTY 通道里执行的命令，负责接回或新建会话 |
| `ensure(exec, name, cwd)` | 返回 `Attached` / `Recreated` / `Unpersisted`，决定是否提示"进程可能已丢失" |
| `snapshot(exec, name, lines)` | 取回带颜色的可见历史，用于重连后回放 |
| `destroy(exec, name)` | 仅用户显式结束会话时调用 |

所有远端命令都通过 `RemoteExec` 执行，因此同一套逻辑既能跑在真实 SSH 通道上，也能用假执行器做单测。

## tmux 实现

```text
options: socket=sentinel history-limit=100000 status=off mouse=off default-terminal=tmux-256color destroy-unattached=off
```

- **专用 socket**（`-L sentinel`）：不与用户自己的 tmux server 混在一起，行为可预测。需要手工接入时用 `tmux -L sentinel attach -t <name>`。
- **注入配置**（`-f ~/.cache/sentinel/tmux.conf`）：`status off` 避免与我们的 UI 重复，`mouse off` 让鼠标事件透传给应用，`history-limit 100000` 与本地缓冲上限对齐，`destroy-unattached off` 保证 detach 后会话不被回收。绝不读取用户的 `~/.tmux.conf`。
- **会话名**：`st_<hostId>_<paneId>`，非 `[A-Za-z0-9_-]` 字符折叠为 `-`。
- **创建**：先 `has-session` 探测，存在则 `Attached`；不存在则 `new-session -d` 创建并返回 `Recreated`。只有 `Recreated` 会触发"远端进程可能已丢失"的提示。
- **接回**：`new-session -A`，一条命令同时覆盖"已存在"和"刚被清理"两种情况。
- **回放**：`capture-pane -p -e -t <name>: -S -<lines>`。注意 pane 目标必须写成 `session:`，且不能带 `=` 精确匹配前缀（tmux 只对 session 目标接受该前缀）。
- **销毁**：`kill-session -t <name>`，会话已不存在时视为成功。

## 降级实现

`NoPersistence` 用于远端没有 tmux 且用户选择继续的场景：

- `kind()` 返回 `none`，GUI 必须持续显示"无持久化"标注；
- `attach_command` 只启动登录 shell（可带 `cd`），不产生任何远端常驻进程；
- `ensure` 返回 `Unpersisted`，`snapshot` 返回空，`destroy` 是空操作。

## 检测与错误

- 探测命令 `command -v tmux >/dev/null 2>&1 && echo yes || echo no`。
- 探测为 `no` 时 `ensure` 返回 `PersistenceError::TmuxMissing`，映射到错误码 `tmux_missing`；GUI 据此提示安装命令或选择降级，**不得**静默假装有持久化。
- 命令非零退出映射为 `CommandFailed`（`internal`），传输层失败映射为 `Transport`（`network`，可重试）。
