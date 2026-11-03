# Comet Design Handoff

- Change: client-identity-and-remote-helper
- Phase: design
- Mode: compact
- Context hash: c3ccc3cd17f4c48dd6db1df4054fd6990d7a35ffd85a9e99f6cd84220e75cacb

Generated-by: comet-handoff.sh
Task hash policy: task-content-v1. Read tasks.md for live completion; excerpts are design-time context.

OpenSpec remains the canonical capability spec. This handoff is a deterministic, source-traceable context pack, not an agent-authored summary.

## docs/openspec/changes/client-identity-and-remote-helper/proposal.md

- Source: docs/openspec/changes/client-identity-and-remote-helper/proposal.md
- Lines: 1-33
- SHA256: 02003049c42a5c261d0d34d7b3131a22189d1100848a9ba85c41642580fd13f8

```md
# 提案：远端 helper 与自动安装

## 为什么

tmux 方案要求服务器预装 tmux，且远端会话只有"一个 shell"这一种结构；滚动缓冲、布局、进程状态都留在 tmux 的语义里，客户端拿不到结构化信息。更重要的是，用户要求"装完 Windows 客户端，第一次连接就自动把服务端装好"——这必须由我们自己的常驻进程承担。

## 变更内容

- 新增 `sentinel-helper` 远端二进制：按客户端命名空间持有 PTY、滚动缓冲与会话生命周期
- 新增首次连接**自动部署**：探测架构/依赖 → 上传 → sha256 校验 → 落盘 → 启动；不依赖 root、不写系统目录
- 新增客户端与 helper 之间的线协议：经 SSH exec 通道的 `sentinel-helper proxy` 桥接到 Unix socket，不开放任何网络端口
- 新增 provider 选择策略：helper 可用时优先，失败时回退 tmux，再回退无持久化，并在界面如实标注
- 新增升级策略：无活跃会话时立即替换，有活跃会话时延迟并提示

## 能力

### 新增能力

- `remote-helper-provisioning`：探测、上传、校验、安装、卸载与升级
- `remote-helper-runtime`：常驻进程、socket 权限、会话/PTY 管理、按客户端隔离
- `provider-selection`：多 provider 的优先级、回退与状态展示

### 修改能力

无。会话命名空间已在 `persistent-ssh-terminal-core` 完成。

## 影响

- 新增 crate `crates/helper`（客户端侧不含，仅服务端二进制）与协议消息
- `crates/persistence` 增加 `HelperProvider`
- 新增部署依赖：`sha2`、`flate2`（后续可压缩传输）
- 服务器要求：x86_64 或 aarch64、glibc ≥ 2.17 或 musl 静态、可写 `~/.local/share/sentinel`、允许执行
- 明确不做：root 安装、systemd 服务、多用户共享 helper、跨主机迁移会话

```

## docs/openspec/changes/client-identity-and-remote-helper/design.md

- Source: docs/openspec/changes/client-identity-and-remote-helper/design.md
- Lines: 1-98
- SHA256: d39f34f78d026f003e6cd5f8743027b5951e280b338f0d735bb389e92ae6017d

[TRUNCATED]

```md
---
comet_change: client-identity-and-remote-helper
role: technical-design
canonical_spec: openspec
---

# 设计：远端 helper 与自动安装

## 背景

见 `proposal.md`。约束来自已完成的核心：客户端已有稳定 uuid 与命名空间，provider 抽象（`PersistenceProvider`）已经存在，SSH 连接支持单连接多通道，凭据在 Credential Manager。

## 目标 / 非目标

**目标**：零手工部署；helper 成为首选 provider；客户端断开不影响远端进程；不同客户端互不串会话。

**非目标**：root/systemd 安装；多用户共享；会话跨主机迁移；Windows 侧运行 helper。

## 决策

### D1：helper 是一个独立的静态二进制

- Rust 编写，`x86_64-unknown-linux-musl` 与 `aarch64-unknown-linux-musl` 各产出一个，约 5–8 MB。
- 只依赖 Linux 基础系统调用（PTY、Unix socket），启动时不需要 tmux、python 或任何运行时。
- 子命令：`serve`（常驻）、`proxy`（stdio ↔ socket 桥接）、`version`、`install-check`（探测）。
- 版本号写入二进制并在启动时打印，供客户端比对。

### D2：传输 = Unix socket + SSH exec 桥接

```
GUI → daemon → SSH channel exec "~/.local/share/sentinel/bin/sentinel-helper proxy"
                     ↕ stdio（帧）
              unix socket ~/.local/share/sentinel/run/helper.sock (0600)
                     ↕
                  helper serve
```

- **不监听 TCP**，不新增端口；权限完全依赖 Unix socket 文件权限与目录权限（0700）。
- 帧格式：`u32 BE 长度` + JSON 头 + 可选二进制体；`kind` 区分 `hello/request/event/data`，二进制体用于终端字节，避免 base64 膨胀。
- 一个 SSH 通道可复用多条会话（多路复用），断线重连就是重新 exec 一次 `proxy`。

### D3：会话与 PTY 由 helper 持有

- 每个会话一个 `openpty`，helper 侧保存 `rows/cols`、环形缓冲（默认 100k 行）与进程 pid。
- `attach` 把该会话的后续输出推给订阅者并回放最近 N 行；`detach` 只解除订阅。
- 会话 key = `(client_id, session_id)`，`list` 只返回本客户端命名空间。
- 会话销毁时杀掉进程组并回收缓冲。

### D4：客户端身份

- 复用核心已生成的 `client.json`（uuid4）；helper 不接受未带 `client_id` 的请求。
- **安全边界**：同一 Unix 账号可读取 socket 并伪造 client_id；隔离是"防误入"，不是"防同账号攻击"。文档必须写明。

### D5：部署流程

1. `uname -m` → 选目标；`ldd --version` 或 `/lib/ld-musl-*` 判断 libc；musl 静态二进制作为默认（无 libc 依赖）。
2. 目标目录 `~/.local/share/sentinel/bin`，`mkdir -p` 后写临时文件 → `chmod 700` → `sha256sum` 比对 → 原子 `mv`。
3. 探测 `noexec`：`findmnt -no OPTIONS -T <dir>` 含 `noexec` 则改试 `$HOME/.local/bin`，仍失败则回退。
4. 上传走同一 SSH 连接的 SFTP 子系统；没有 SFTP 时回退 `base64 -d` 管道（慢但可用）。
5. 启动：`setsid nohup <bin> serve >/dev/null 2>&1 &`，随后等待 socket 出现（≤3s），失败则读取 `<dir>/helper.log` 尾部作为诊断。

### D6：常驻、空闲与升级

- helper 用 `flock` 保证单实例；改造/升级时写 `version` 文件。
- 空闲：无会话且无连接超过 30 分钟 → 退出。
- 升级：客户端发现版本不一致 → 若 `list` 为空则停旧起新；否则记录 `pending`，在最后一个会话结束时替换，并在界面提示。
- 失败回退链：helper 不可用 → tmux → 无持久化，原因写入会话事件。

### D7：与现有代码的接缝

- `crates/helper`：新 crate（serve/proxy/探测）。
- `crates/persistence/src/helper.rs`：`HelperProvider` 实现 `PersistenceProvider`，把 `attach/detach/resize/snapshot` 映射到协议请求。
- `crates/daemon/src/manager.rs`：按优先级选择 provider，把"选中原因"带进会话摘要。
- 协议新增 `helper` 相关错误码（`helper_missing`、`helper_incompatible`、`helper_checksum`），GUI 只做展示。

## 失败路径

| 失败 | 行为 |
|---|---|
| 架构不支持 | 不尝试上传，直接 tmux/无持久化，界面说明原因 |

```

Full source: docs/openspec/changes/client-identity-and-remote-helper/design.md

## docs/openspec/changes/client-identity-and-remote-helper/tasks.md

- Source: docs/openspec/changes/client-identity-and-remote-helper/tasks.md
- Lines: 1-41
- SHA256: bc18c4c2e910d93d6427ce152253de37f5685319489eb421cdacc87592da761b

```md
# 任务

## 1. helper crate 与协议

- [ ] 1.1 新建 `crates/helper`，实现 `version` 与 `install-check`（架构/libc/目录可写/noexec），验证：在本机与一个 noexec 目录下输出符合预期
- [ ] 1.2 定义帧协议（长度 + JSON 头 + 二进制体）与控制消息（hello/list/create/attach/detach/resize/destroy/data/event），验证：round-trip 单测覆盖全部消息与非法帧
- [ ] 1.3 实现 `proxy` 子命令：stdio ↔ Unix socket 双向桥接，验证：本地起 helper 后用 `printf` 管道完成一次 hello 往返

## 2. 会话运行时

- [ ] 2.1 用 `openpty` 实现会话创建与进程启动（登录 shell、可指定 cwd），验证：创建后能在 pty 中执行命令并读到输出
- [ ] 2.2 实现环形缓冲与 attach 回放，验证：输出 20k 行后 attach 能取回最近窗口
- [ ] 2.3 实现 detach/attach/resize/destroy 与进程组清理，验证：detach 后进程仍在，destroy 后进程消失
- [ ] 2.4 实现按 `client_id` 的命名空间隔离，验证：两个 namespace 的 list 互不可见，跨 namespace attach 被拒绝
- [ ] 2.5 实现单实例锁与空闲退出，验证：第二个 serve 退出；无会话超时后进程结束

## 3. 自动部署

- [ ] 3.1 实现远端探测（uname/ldd/目录写权限/noexec），验证：普通用户目录、只读目录、noexec 目录三种情况判定正确
- [ ] 3.2 实现上传与原子安装（临时文件 → chmod → sha256 → mv），验证：正常安装成功；哈希不符时删除且报错
- [ ] 3.3 实现 SFTP 上传与 `base64 -d` 回退，验证：禁用 SFTP 子系统时仍能安装
- [ ] 3.4 实现启动与就绪等待（socket 出现 ≤3s），验证：安装后 3 秒内可完成一次 attach
- [ ] 3.5 实现版本比对与延迟升级，验证：有活跃会话时标记 pending，会话结束后替换

## 4. 集成到客户端

- [ ] 4.1 在 `crates/persistence` 实现 `HelperProvider`（attach/detach/resize/snapshot），验证：真实主机上创建会话并回放
- [ ] 4.2 在 manager 中实现 provider 优先级（helper → tmux → none）与"选中原因"，验证：三种路径各走通一次
- [ ] 4.3 首次连接确认框（主机、路径、大小、sha256）与"该主机不再询问"，验证：确认后安装、拒绝后回退且不写文件
- [ ] 4.4 界面展示 provider 与降级原因，验证：helper/tmux/none 三种标注正确

## 5. 端到端与文档

- [ ] 5.1 端到端：全新服务器（无 tmux）上首次连接自动安装并创建会话，验证：全程无手工命令
- [ ] 5.2 断线恢复：attach 后杀掉 SSH 连接再重连，验证：shell 状态与输出保留
- [ ] 5.3 编写 `docs/helper.md`：部署路径、socket 权限、安全边界（同账号风险）、卸载与清理
- [ ] 5.4 回归：helper 不可用时 tmux 路径仍正常，验证：删除 helper 后连接走 tmux

## Workflow follow-up

- 项目审查要求满足后归档本 change。

```

## docs/openspec/changes/client-identity-and-remote-helper/.openspec.yaml

- Source: docs/openspec/changes/client-identity-and-remote-helper/.openspec.yaml
- Lines: 1-2
- SHA256: b6d27c089c6ee9596ce0433bd8f7f771b477758efe1ce7f7a7505af47316ecd0

```md
schema: spec-driven
created: 2026-11-03

```

## docs/openspec/changes/client-identity-and-remote-helper/specs/provider-selection/spec.md

- Source: docs/openspec/changes/client-identity-and-remote-helper/specs/provider-selection/spec.md
- Lines: 1-34
- SHA256: 623b96ece9d33a737347f2668cda48fcf39136046227ec4b41d50df447d61c33

```md
# Spec Delta

## Purpose

在 helper、tmux 与无持久化之间做出可解释的选择，并向用户如实展示当前会话由谁保持。

## ADDED Requirements

### Requirement: 固定优先级

provider 选择 MUST 按 helper → tmux → 无持久化 的顺序尝试，只有前一个不可用时才降级。

#### Scenario: helper 可用时优先

- **WHEN** 远端 helper 已安装且版本兼容
- **THEN** 新会话由 helper 承载，界面标注"helper"而不是"tmux"

### Requirement: 降级可解释

发生降级时 MUST 说明原因（未安装、架构不支持、校验失败、目录不可执行、版本不兼容），MUST NOT 静默切换。

#### Scenario: 降级原因可见

- **WHEN** helper 因架构不支持而降级到 tmux
- **THEN** 界面显示具体原因，而不是只显示"tmux"

### Requirement: 已有会话不被强制迁移

provider 选择只影响新建会话；已存在的会话 MUST 保持原 provider，直到用户显式重建。

#### Scenario: 升级后旧会话仍在 tmux

- **WHEN** helper 安装完成时存在由 tmux 承载的旧会话
- **THEN** 该会话继续使用 tmux，只有新会话才用 helper

```

## docs/openspec/changes/client-identity-and-remote-helper/specs/remote-helper-provisioning/spec.md

- Source: docs/openspec/changes/client-identity-and-remote-helper/specs/remote-helper-provisioning/spec.md
- Lines: 1-57
- SHA256: b757eb8596b0c3149ff7d898dfbcdd7729802da3bc8a0f23064020f31fd49b0b

```md
# Spec Delta

## Purpose

让客户端在首次连接时把远端 helper 自动装好，用户不需要登录服务器做任何手工操作。

## ADDED Requirements

### Requirement: 首次连接自动部署

远端不存在可用 helper 时，系统 MUST 在上传任何内容前向用户展示一次确认（主机、目标路径、文件大小、sha256），确认后自动完成上传、校验、安装与启动。同一主机确认后 MUST 不再重复询问。

#### Scenario: 首次连接自动安装

- **WHEN** 用户第一次连接一台没有 helper 的主机并确认安装
- **THEN** 无需再次操作，连接建立后会话由 helper 接管，界面标注"helper"

#### Scenario: 拒绝安装时回退

- **WHEN** 用户在确认框中选择取消
- **THEN** 系统不写入任何文件，并按 provider 优先级回退到 tmux 或无持久化

### Requirement: 部署前的能力探测

部署前 MUST 检测远端架构、libc 类型与版本、目标目录可写性与挂载是否允许执行；不满足任一条件时 MUST 说明具体原因并回退。

#### Scenario: noexec 挂载被识别

- **WHEN** 目标目录位于 `noexec` 挂载点
- **THEN** 系统报告"目录不可执行"并改用备用目录或回退，而不是反复失败

### Requirement: 完整性校验

上传完成后 MUST 校验文件 sha256 与客户端内置值一致，不一致 MUST 删除文件并中止安装。

#### Scenario: 传输损坏被拒绝

- **WHEN** 上传后的文件哈希与期望值不符
- **THEN** 文件被删除，界面提示校验失败，会话回退到 tmux

### Requirement: 不依赖 root

安装 MUST 只写用户可写目录（默认 `~/.local/share/sentinel`），MUST NOT 请求 sudo 或写入系统路径。

#### Scenario: 普通用户可安装

- **WHEN** 用户以普通账号登录（无 sudo 权限）
- **THEN** 安装成功且不产生任何权限错误

### Requirement: 版本升级

客户端版本高于远端 helper 时 MUST 触发升级；无活跃会话时立即替换，有活跃会话时 MUST 延迟到全部会话结束并提示用户。

#### Scenario: 有会话时延迟升级

- **WHEN** 远端 helper 版本较旧且存在活跃会话
- **THEN** 升级被推迟，界面提示"空闲时升级"，现有会话不受影响

```

## docs/openspec/changes/client-identity-and-remote-helper/specs/remote-helper-runtime/spec.md

- Source: docs/openspec/changes/client-identity-and-remote-helper/specs/remote-helper-runtime/spec.md
- Lines: 1-61
- SHA256: 1f96b1e44f4c95e4aea2b42ce210294fa67b07b038cc226228717ad68e026912

```md
# Spec Delta

## Purpose

在远端以用户态常驻进程管理 PTY 与会话，使断开客户端后进程继续运行，并在重连时按客户端标识隔离接入。

## ADDED Requirements

### Requirement: 常驻与自动启动

helper MUST 在首次使用时自行脱离终端常驻；客户端断开后 MUST 继续持有会话。系统重启后 helper MUST 在下次连接时被重新拉起，并按需恢复会话。

#### Scenario: 客户端退出后进程仍存活

- **WHEN** 用户关闭 Windows 客户端
- **THEN** 远端 helper 与其中的 shell 继续运行，`ps` 可见其常驻进程

### Requirement: 只通过 SSH 通道通信

helper MUST 只监听 Unix socket，MUST NOT 监听任何 TCP 端口；客户端通过 SSH exec 通道运行 `sentinel-helper proxy` 完成桥接。

#### Scenario: 不暴露网络端口

- **WHEN** helper 运行后检查服务器监听端口
- **THEN** 除 sshd 外没有新增监听端口

### Requirement: socket 权限

socket 文件 MUST 仅当前用户可读写（0600），位于不可被其他用户遍历的目录内。

#### Scenario: 其他用户无法访问

- **WHEN** 另一个用户尝试连接该 socket
- **THEN** 因文件权限被拒绝

### Requirement: 按客户端命名空间隔离

helper MUST 按客户端标识隔离会话集合，不同客户端的会话列表 MUST 互不可见，MUST NOT 允许跨命名空间接入。

#### Scenario: 两台客户端互不可见

- **WHEN** 两个不同客户端标识连接同一 helper
- **THEN** 各自只看到并接回自己的会话

### Requirement: 会话生命周期

helper MUST 支持创建、接入、脱离、销毁会话与调整 PTY 尺寸，并在销毁时释放对应缓冲。

#### Scenario: 脱离后重新接入

- **WHEN** 客户端 detach 后再次 attach 同一会话
- **THEN** 会话保持原 shell 状态并回放最近的输出

### Requirement: 空闲回收

当某命名空间下没有任何会话且无客户端连接超过配置时长后，helper MUST 自行退出，避免长期占用内存。

#### Scenario: 空闲后退出

- **WHEN** 所有会话销毁且超过空闲阈值
- **THEN** helper 进程退出，下次连接时按需重新拉起

```
