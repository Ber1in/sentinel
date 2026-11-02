# 本地 IPC 协议

守护进程在 `127.0.0.1` 的随机端口上监听 WebSocket，GUI 是唯一客户端。协议版本由 `crates/protocol/src/lib.rs` 的 `PROTOCOL_VERSION` 定义，任何不兼容改动都必须递增它。

## 连接与握手

1. daemon 启动时把 `{pid, port, token, protocol_version, daemon_version, started_at}` 写入 `<data dir>/daemon.json`（Unix 下权限 `0600`）。
2. GUI 读取该文件，连接 `ws://127.0.0.1:<port>`，在 5 秒内发送 `hello`。
3. daemon 校验 `protocol_version` 与 `token`：任一不匹配就发送一条 `event`（`daemon_shutting_down`，reason 前缀为错误码）后以 1008 关闭连接。
4. 校验通过后 daemon 回 `hello`，携带 `ServerInfo`。

握手失败不会留下半开连接；GUI 侧必须区分「版本不匹配」与「token 错误」，前者提示重启守护进程，后者说明 `daemon.json` 已过期。

## 控制面消息

文本帧，UTF-8 JSON，`type` 字段区分种类。单条消息上限 1 MiB。

### 客户端 → 守护进程

| type | 字段 | 说明 |
| --- | --- | --- |
| `hello` | `protocol_version`, `token`, `client_version` | 必须是第一条消息 |
| `request` | `id` (u32), `op` | `id` 由客户端自增，用于关联 `reply` |

`op` 取值：

| op | 参数 | 说明 |
| --- | --- | --- |
| `list_sessions` | — | 返回全部会话摘要 |
| `create_session` | `host`, `pane_id`, `cwd` | 建立连接并绑定持久化会话 |
| `attach` | `session_id` | 接回已脱离的会话 |
| `detach` | `session_id` | 从界面脱离，远端保留 |
| `destroy` | `session_id` | 结束会话并销毁远端会话 |
| `resize` | `session_id`, `cols`, `rows` | 同步 PTY 尺寸 |
| `resync` | `session_id`, `from_seq` | 请求从指定序号重建画面 |
| `restart_daemon` | — | 优雅重启（先 detach，不 kill 远端） |

### 守护进程 → 客户端

| type | 字段 | 说明 |
| --- | --- | --- |
| `hello` | `info` | 握手成功 |
| `reply` | `id`, `result` | 对应一次 `request` |
| `event` | `event` | 异步通知，无需请求 |

`result` 取值：`ok`、`sessions`（带 `sessions`）、`session`（带 `session_id`、`provider`、`recreated`）、`error`（带 `error`）。

`event` 取值：`session_state`（`session_id`、`state`、`detail`）、`session_created`（`session_id`、`provider`、`recreated`、`cwd`）、`session_destroyed`、`daemon_shutting_down`（`reason`）。

## 数据面帧

二进制帧，固定 13 字节头 + 原始终端字节：

```
offset  size  field
0       1     kind       1 = output (daemon -> GUI), 2 = input (GUI -> daemon)
1       4     session    u32 little-endian，daemon 分配的会话号
5       8     seq        u64 little-endian，每会话单调递增
13      n     payload    原始字节，原样写入 PTY 或 xterm
```

示例（`kind=1, session=7, seq=1000000, payload="hello"`）：

```
frame-hex: 01 07 00 00 00 40 42 0f 00 00 00 00 00 68 65 6c 6c 6f
```

### 顺序与背压

- `seq` 从 0 开始，每帧 +1，缺口即表示丢帧。
- GUI 发现 `seq` 跳变时必须发送 `resync`，daemon 返回该会话最近的快照帧并重置序号；不允许逐字节补发。
- daemon 按 16 ms 合并同一会话的输出；GUI 侧积压时允许丢弃中间帧，最终以 `resync` 收敛。
- 输入方向不做合并，逐帧转发；重连期间的输入进入 64 KiB 有界队列。

## 错误码

| code | 可重试 | 含义 |
| --- | --- | --- |
| `auth_failed` | 否 | 密码/私钥被拒 |
| `host_key_mismatch` | 否 | 主机密钥与记录不一致 |
| `network` | 是 | 传输层失败 |
| `timeout` | 是 | 操作或握手超时 |
| `protocol_version` | 否 | 版本不兼容 |
| `not_found` | 否 | 会话或资源不存在 |
| `invalid_request` | 否 | 消息格式或状态不允许 |
| `tmux_missing` | 否 | 远端缺少 tmux |
| `internal` | 视情况 | 未分类的内部错误 |

错误分类是状态机的唯一分支依据，禁止通过匹配 `message` 文本决定重试。
