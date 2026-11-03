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
| 目录不可写 / noexec | 换备用目录；仍失败则回退 |
| 校验失败 | 删除临时文件，回退，提示 |
| helper 崩溃 | 下次 attach 时重新拉起；缓冲丢失，界面标注"进程可能已丢失" |
| socket 权限异常 | 报错并建议重新安装，不静默降级 |
| 版本不兼容 | 延迟升级或提示重启会话 |

## 风险 / 权衡

- 自研 PTY 管理引入内存与信号处理复杂度 → 只做单进程多会话，会话数上限 32，超出拒绝。
- 静态二进制体积（~8MB）在慢速链路上首次上传较慢 → 提供进度提示；后续可加 zstd 压缩。
- 同一账号的安全边界 → 文档明确，不做虚假承诺。
- helper 与 tmux 并存期间的语义差异 → 只影响新会话，旧会话保持不变。

## 测试策略

- 单元：帧编解码、命名空间隔离、空闲判定、版本比较、探测结果解析。
- 集成（WSL + 本地 sshd）：部署到临时 HOME、启动、创建/attach/detach/resize/销毁、断开 SSH 后进程存活、重连接回、校验失败路径。
- 回归：tmux provider 不受影响；降级链在 helper 缺失时仍工作。
