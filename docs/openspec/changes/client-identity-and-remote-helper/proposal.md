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
