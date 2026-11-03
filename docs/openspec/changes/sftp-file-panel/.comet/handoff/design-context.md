# Comet Design Handoff

- Change: sftp-file-panel
- Phase: design
- Mode: compact
- Context hash: d43353698db6a5bca7e20baed3e583dc9d8ddf4c53453e09954f396fc7595e6c

Generated-by: comet-handoff.sh
Task hash policy: task-content-v1. Read tasks.md for live completion; excerpts are design-time context.

OpenSpec remains the canonical capability spec. This handoff is a deterministic, source-traceable context pack, not an agent-authored summary.

## docs/openspec/changes/sftp-file-panel/proposal.md

- Source: docs/openspec/changes/sftp-file-panel/proposal.md
- Lines: 1-34
- SHA256: f7178bb3eb6841f522cb2ee11c68d7ec884ac6c69d96c30d9a75d532f8ae693d

```md
# 提案：SFTP 文件面板

## 为什么

在服务器上排查问题时，改配置、看日志、拉取证文件都离不开在终端和文件传输工具之间来回切换：SSH 客户端只管终端，Windows 资源管理器只能靠 WinSCP/FileZilla 另开连接。核心 change 已经建立了常驻 SSH 连接与跨主机标签，文件访问没有理由再新建一条连接——它应当是当前标签的附属视图。

## 变更内容

- 新增右侧可折叠 SFTP 面板，绑定当前活动标签的主机，随标签切换而切换
- 面板默认跟随终端当前工作目录（通过 shell 集成上报），可切换为手动锁定
- 目录浏览：面包屑导航、双击进入、返回上级、刷新、隐藏文件开关
- 文件操作右键菜单：上传文件、上传目录、下载到…、新建目录、重命名、删除、修改权限
- 拖拽上传：从 Windows 资源管理器拖入面板即上传到当前目录
- 传输队列：进度、速度、取消、失败重试入口；不保留历史记录
- 复用核心 change 的 SSH 连接（同一 TCP 连接上的 SFTP 子系统通道），不为面板新建连接

## 能力

### 新增能力

- `sftp-file-browser`：目录浏览、导航、跟随终端目录、面板与标签的绑定关系
- `file-transfer-queue`：上传/下载的发起方式、拖拽、进度与取消、失败处理
- `remote-file-operations`：远端新建、重命名、删除、改权限的语义与确认规则

### 修改能力

无。核心 change 尚未归档，本 change 只新增能力。

## 影响

- 依赖 `persistent-ssh-terminal-core` 的连接层与 IPC 协议：需要新增 SFTP 相关 RPC 与在同一连接上打开子系统的能力
- 新增依赖：SFTP 客户端实现（与核心同栈）、传输队列与进度上报
- 影响模块：`crates/ssh`（子系统通道）、`crates/protocol`（新增消息）、`apps/desktop`（新面板 UI）
- 明确不做：在线编辑、双向目录同步、断点续传、传输历史、批量多选、独立 SFTP 会话

```

## docs/openspec/changes/sftp-file-panel/design.md

- Source: docs/openspec/changes/sftp-file-panel/design.md
- Lines: 1-86
- SHA256: 030a29d479ca3047b6c1ed7028718532bd3a890d6e2ee6a88fc6feb06000742b

[TRUNCATED]

```md
---
comet_change: sftp-file-panel
role: technical-design
canonical_spec: openspec
---

# 设计：SFTP 文件面板

## 背景

见 `proposal.md`。约束来自核心 change 已确定的架构：SSH 连接与凭据都住在守护进程里，GUI 不持有连接；因此文件访问只能是「守护进程上的新通道 + GUI 上的新视图」，不能引入第二套连接或第二个运行时。核心 change 的协议、连接复用与 shell 集成 cwd 事件是本设计的前提。

## 目标 / 非目标

**目标**

- 文件面板与终端共享同一条 SSH 连接，切标签不新建连接
- 大文件不经过 GUI 进程，避免内存与 IPC 压力
- 目录浏览与传输都能在中止/失败后给出可读结果

**非目标**

- 不做编辑器、不做双向同步、不做断点续传（重试从头开始）
- 不做多选批量与传输历史
- 不做独立的 SFTP 连接或 SFTP-only 主机

## 关键决策

### D1：SFTP 走同一连接上的子系统通道

```
crates/ssh  HostConnection
              |-- channel: session/pty   (终端)
              |-- channel: subsystem sftp (文件面板, 每个主机连接一个)
```

- `russh` 的 `channel_open_session` + `request_subsystem("sftp")` 复用既有连接，天然满足「切标签不新建连接」。
- 一个主机连接共享一个 SFTP 会话，用请求 ID 多路复用；同一主机的多个标签各自保存目录，互不影响。
- 备选：每标签一条 SFTP 通道（浪费、且服务端并发限制易触发）、`scp` 子进程（无目录列表、无进度）。

### D2：文件数据由守护进程直接读写本地磁盘

```
GUI (选择本地路径) --RPC--> daemon --SFTP--> remote
GUI (进度事件)     <--event-- daemon --本地文件系统--> D:\tmp\x.log
```

- 下载：daemon 直接写入用户选定路径；上传：daemon 直接读取。
- 理由：10MB~GB 级文件穿过 WebSocket 会让 GUI 主进程成为瓶颈，且需要额外的分片/背压协议。
- 代价：daemon 需要以用户身份访问任意用户选定路径（它本来就是用户级进程），并必须校验路径来自 GUI 的可信选择，拒绝 `..` 穿越与设备路径。
- 备选：数据经 WS 分片传输（灵活但复杂，留作远程客户端场景）。

### D3：上传先写 `.part` 再原子改名

- 上传目标为 `<dir>/.<name>.part-<taskId>`，完成后 `rename` 到最终名；取消或失败时删除临时文件。
- 覆盖确认在 daemon 侧以显式 RPC 完成，避免 GUI 与 daemon 对「是否存在」判断不一致。
- 下载同理写 `<path>.part` 后改名，避免半文件被当成完整文件。

### D4：跟随 cwd 来自核心的 shell 集成事件

- 核心已通过 OSC 7 上报 cwd，面板订阅 `cwdChanged(sessionId, path)`。
- 锁定态下忽略事件；锁定是每标签的状态，随布局一起持久化。
- cwd 指向的目录不存在时（例如刚被删除），面板回退到上一可访问目录并提示。

### D5：目录列表一次读取、虚拟滚动渲染

- 一次 `readdir` 全量返回（服务端无分页 API），GUI 用虚拟列表渲染，10k 条目仍可滚动。
- 排序按目录优先 + 名称，本地完成；不在服务端排序。
- 符号链接显示为链接图标，进入时由服务端解析；断链给出错误而不是崩溃。

### D6：传输队列住在 daemon

- 队列与并发（每方向 3）由 daemon 管理，GUI 只是渲染与下发指令；因此 GUI 重启后队列消失（符合「不留历史」），但守护进程仍在跑的任务不会被 GUI 关掉影响。
- 进度事件按 200ms 节流推送，避免高频事件压垮渲染。

## 风险 / 权衡

- **大目录（>50k 条目）阻塞**：`readdir` 全量解析会卡住该连接的请求循环 → 目录读取放到独立任务并设置超时，超时返回部分结果 + 「继续加载」标记。
- **SFTP 与终端共享连接的带宽争抢**：上传大文件时终端可能卡顿 → 传输按块限速（默认不限，可配），并给终端通道更高调度优先级。
- **符号链接与权限边界**：删除/改权限可能越界到挂载点 → 所有破坏性操作在 daemon 侧解析真实路径并确认仍在用户选定路径内；跨挂载点删除要求显式确认。

```

Full source: docs/openspec/changes/sftp-file-panel/design.md

## docs/openspec/changes/sftp-file-panel/tasks.md

- Source: docs/openspec/changes/sftp-file-panel/tasks.md
- Lines: 1-48
- SHA256: bfcf7a6319422ad039f1bb264d315058b28fba513bf5ad1384f48360771d5648

```md
# 任务

## 1. 协议与连接扩展

- [ ] 1.1 在 `crates/protocol` 增加 SFTP 消息族（list/stat/mkdir/rename/delete/chmod、upload/download/queue/cancel、进度事件、错误码）并重新生成 TS 类型，验证：round-trip 单测覆盖全部消息，前端类型检查通过
- [ ] 1.2 在 `crates/ssh` 实现同一连接上的 SFTP 子系统通道与请求多路复用（每主机连接一个会话），验证：同时开终端与 SFTP 时 TCP 连接数仍为 1，并发 20 个 list 请求不乱序
- [ ] 1.3 实现 daemon 侧路径校验（拒绝 `..` 穿越、设备路径、跨挂载点破坏性操作需显式标记），验证：单测覆盖恶意路径样例且均被拒绝
- [ ] 1.4 在 `docs/protocol.md` 补充 SFTP 消息表与进度事件语义，验证：文档示例帧可被单测解析

## 2. 远端文件操作

- [ ] 2.1 实现目录读取（名称/大小/修改时间/权限/类型/符号链接标记）与虚拟列表所需的最小字段，验证：对含 5k 条目的目录读取在 2 秒内返回
- [ ] 2.2 实现新建目录、重命名、删除（含非空目录提示）、chmod（八进制校验），验证：每项在真实主机上成功执行，非法权限 `999` 被拒且远端不变
- [ ] 2.3 实现操作失败的错误映射（权限不足、目标已存在、目录非空、连接中断），验证：每类错误有对应单测与可读文案
- [ ] 2.4 实现大目录超时与「继续加载」，验证：模拟慢目录时先返回部分结果且界面可继续加载

## 3. 传输引擎

- [ ] 3.1 实现下载：写 `<path>.part` → 完成后改名，验证：中途取消后正式文件不存在、`.part` 被清理
- [ ] 3.2 实现上传（单文件与目录递归），先写远端 `.<name>.part-<taskId>` 再原子改名，验证：取消后远端无正式文件、无残留临时文件
- [ ] 3.3 实现队列与并发限制（每方向 3）、进度/速度节流推送（200ms），验证：并发 5 个任务时只有 3 个处于传输中，进度事件频率符合节流
- [ ] 3.4 实现覆盖确认 RPC（覆盖/跳过/全部覆盖），验证：同名文件上传出现确认，选择跳过后远端内容不变
- [ ] 3.5 实现失败分类与重试（重试从头开始），验证：断网失败后恢复网络重试成功，队列状态从失败转完成
- [ ] 3.6 实现 Windows 文件名合法化与冲突提示，验证：远端文件名含 `:` 时提示改名而非静默修改
- [ ] 3.7 实现残留 `.part` 扫描与清理提示，验证：人为留下临时文件后启动扫描能列出并可清理

## 4. 文件面板 UI

- [ ] 4.1 实现右侧可折叠面板、宽度调整与状态持久化，验证：折叠与宽度在重启后保持
- [ ] 4.2 实现目录列表（虚拟滚动、目录优先排序、隐藏文件开关、刷新）与面包屑导航，验证：5k 条目目录滚动流畅、面包屑跳转正确
- [ ] 4.3 实现面板与活动标签的绑定：切标签切换主机与目录，同主机多标签共享连接但目录独立，验证：验收场景「切换标签不串台」
- [ ] 4.4 实现 cwd 跟随与锁定模式（按标签持久化），验证：`cd /var/log` 后跟随；锁定后 `cd /etc` 不跳转
- [ ] 4.5 实现右键菜单（上传文件/上传目录/下载到…/新建目录/重命名/删除/改权限）与对应确认框，验证：每项操作在真实主机上可用，删除有确认
- [ ] 4.6 实现拖拽上传（资源管理器 → 面板），验证：拖入文件后上传到当前目录且队列出现任务
- [ ] 4.7 实现传输队列 UI（进度、速度、取消、重试、失败原因），验证：验收场景中的取消与重试路径
- [ ] 4.8 实现错误态与空态（无权限目录、断链符号链接、目录已删除），验证：每种情况都有可读提示与返回入口

## 5. 集成与文档

- [ ] 5.1 端到端跑通 `proposal.md` 关联的 5 条验收场景，输出 `docs/reports/sftp-acceptance.md`，验证：5 条全部通过或明确记录失败原因
- [ ] 5.2 采集传输基线（1GB 文件上传/下载速度、大目录打开耗时、内存峰值）写入 `docs/perf.md`，验证：数据可复现且脚本随仓库提交
- [ ] 5.3 在 `docs/configuration.md` 补充 SFTP 相关配置（并发数、限速、是否默认跟随 cwd、隐藏文件默认值），验证：字段与配置 schema 单测一致
- [ ] 5.4 在 `docs/troubleshooting.md` 补充 SFTP 排错（权限、残留 `.part`、连接中断、面板不显示），验证：按文档可定位一次人为制造的上传失败

## Workflow follow-up

- 核心 change 归档且 SFTP 依赖的协议稳定后再进入本 change 的 Build。
- 项目审查要求满足后归档本 change。

```

## docs/openspec/changes/sftp-file-panel/.openspec.yaml

- Source: docs/openspec/changes/sftp-file-panel/.openspec.yaml
- Lines: 1-2
- SHA256: 71e91d8f42848565b00ec7bb4da95cf19e2182f4adc440c230b10a0f3345023b

```md
schema: spec-driven
created: 2026-11-02

```

## docs/openspec/changes/sftp-file-panel/specs/file-transfer-queue/spec.md

- Source: docs/openspec/changes/sftp-file-panel/specs/file-transfer-queue/spec.md
- Lines: 1-61
- SHA256: dde4c1572f512533259a85513456fb7c8d9e5f1641503a79bcf2bdd516d6952c

```md
# Spec Delta

## Purpose

提供可观察、可取消的文件上传与下载，让用户在终端旁完成取证、改配置、传文件，而不必切换到独立 FTP 工具。

## ADDED Requirements

### Requirement: 上传入口

系统 MUST 支持右键上传文件、右键上传目录与从 Windows 资源管理器拖拽文件到面板三条上传路径，目标目录 MUST 为面板当前目录。

#### Scenario: 拖拽上传到当前目录

- **WHEN** 用户把 `nginx.conf` 从资源管理器拖到面板的 `/etc/nginx` 视图
- **THEN** 文件被上传到 `/etc/nginx/nginx.conf`，队列出现该任务

### Requirement: 下载入口

系统 MUST 支持右键下载到本地，MUST 弹出 Windows 目录选择对话框；选择单个文件时 MUST 允许指定文件名。

#### Scenario: 下载文件并保留原名

- **WHEN** 用户对 `/var/log/syslog` 选择"下载到…"并确认目录 `D:\tmp`
- **THEN** 文件被保存为 `D:\tmp\syslog`，队列显示完成

### Requirement: 传输队列可见可控

队列 MUST 显示每个任务的进度、速度、已传/总量，MUST 支持取消，并 MUST 限制并发数。

#### Scenario: 取消进行中的传输

- **WHEN** 用户取消一个正在传输的大文件
- **THEN** 传输停止，队列该项标注为已取消，远端不留下不完整的同名正式文件

### Requirement: 失败可读且可重试

传输失败时 MUST 显示可读原因（权限、磁盘空间、连接中断）与失败文件，MUST 提供重试入口。

#### Scenario: 连接中断后重试成功

- **WHEN** 上传过程中网络中断导致失败，网络恢复后用户点击重试
- **THEN** 该任务重新开始并最终完成，队列状态从失败变为完成

### Requirement: 覆盖前确认

目标位置已存在同名文件时，系统 MUST 在覆盖前请求确认，并 MUST 支持"全部覆盖"与"跳过"。

#### Scenario: 同名文件需确认

- **WHEN** 用户上传的文件在远端已存在
- **THEN** 出现覆盖确认，选择跳过后远端文件保持不变

### Requirement: 不保留传输历史

传输队列 MUST 只在本次运行期间存在，应用重启后 MUST 清空，MUST NOT 写入历史记录。

#### Scenario: 重启后队列为空

- **WHEN** 用户完成若干传输后重启应用
- **THEN** 队列为空，界面上不出现任何历史传输记录

```

## docs/openspec/changes/sftp-file-panel/specs/remote-file-operations/spec.md

- Source: docs/openspec/changes/sftp-file-panel/specs/remote-file-operations/spec.md
- Lines: 1-43
- SHA256: 0737ca0f4cd687efd8fa60e333f3f71ccbb7b3bc35687e274a492ab9c45ca041

```md
# Spec Delta

## Purpose

在面板内完成常见的远端文件管理操作，并保证删除、改权限这类不可逆或高影响操作有明确确认与可读结果。

## ADDED Requirements

### Requirement: 基础文件操作

面板 MUST 支持新建目录、重命名、删除与修改权限，操作 MUST 作用于当前选中的条目。

#### Scenario: 新建目录后立即可见

- **WHEN** 用户在 `/etc/nginx` 新建目录 `sites`
- **THEN** 列表中出现 `sites` 且可直接双击进入

### Requirement: 删除必须确认

删除操作 MUST 在执行前请求确认；删除非空目录时 MUST 明确提示将递归删除的内容。

#### Scenario: 删除非空目录有提示

- **WHEN** 用户删除一个包含文件的目录
- **THEN** 确认框说明将删除目录及其内容，取消后远端不变

### Requirement: 权限修改需校验

修改权限 MUST 接受八进制输入并校验合法性，非法输入 MUST 被拒绝且不发送到远端。

#### Scenario: 非法权限被拒绝

- **WHEN** 用户输入 `999` 作为权限
- **THEN** 系统提示非法权限，远端文件权限保持不变

### Requirement: 操作结果可观察

操作完成后面板 MUST 刷新受影响条目并显示结果；失败时 MUST 显示可读原因，MUST NOT 静默重试。

#### Scenario: 重命名失败提示原因

- **WHEN** 用户把文件重命名为已存在的名字
- **THEN** 面板显示目标已存在的错误，文件保持原名

```

## docs/openspec/changes/sftp-file-panel/specs/sftp-file-browser/spec.md

- Source: docs/openspec/changes/sftp-file-panel/specs/sftp-file-browser/spec.md
- Lines: 1-52
- SHA256: 4aa13d9a03630021856918712cabc52727ea508cd9b0ef69f5eeed6a753d9312

```md
# Spec Delta

## Purpose

在终端窗口内浏览远端文件系统，让文件访问成为当前会话的附属视图，而不是另开一条连接。

## ADDED Requirements

### Requirement: 面板绑定当前标签的主机

文件面板 MUST 绑定当前活动标签的主机，标签切换时 MUST 切到该主机的目录视图。同一主机的多个标签 MUST 共享同一条 SSH 连接，但各自保持独立的当前目录。

#### Scenario: 切换标签不串台

- **WHEN** 标签 1 是主机 A 的 `/etc`，标签 2 是主机 B 的 `/var/log`，用户来回切换
- **THEN** 面板分别显示 `/etc` 与 `/var/log`，且不产生新的 SSH 连接

### Requirement: 默认跟随终端工作目录

面板 MUST 默认跟随当前会话终端的工作目录变化；用户 MUST 能切换为锁定模式，锁定后目录 MUST 不随终端变化。

#### Scenario: cd 后面板自动跳转

- **WHEN** 用户在终端执行 `cd /var/log`
- **THEN** 面板自动切换到 `/var/log`；用户锁定后再 `cd /etc`，面板仍停留在 `/var/log`

### Requirement: 目录浏览与导航

面板 MUST 显示名称、大小、修改时间与权限，MUST 支持双击进入目录、面包屑跳转、返回上级、刷新与隐藏文件开关。

#### Scenario: 通过面包屑回退

- **WHEN** 用户位于 `/etc/nginx/conf.d`
- **THEN** 点击面包屑中的 `nginx` 后进入 `/etc/nginx`，且列表刷新为对应内容

### Requirement: 目录访问失败可恢复

目录不存在或权限不足时，面板 MUST 显示可读错误与失败路径，MUST NOT 停留在空白状态，并 MUST 提供返回上一级或刷新入口。

#### Scenario: 无权限目录不空白

- **WHEN** 用户进入一个当前用户无读权限的目录
- **THEN** 面板显示权限错误与该目录路径，用户可点击返回上一级继续浏览

### Requirement: 面板可折叠且状态被记住

面板 MUST 支持折叠与展开、宽度调整，折叠状态与宽度 MUST 在重启后恢复。

#### Scenario: 折叠状态持久化

- **WHEN** 用户折叠面板并调整宽度后重启应用
- **THEN** 面板保持折叠状态，展开后宽度与关闭前一致

```
