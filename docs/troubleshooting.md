# 排错

## 日志位置

| 组件 | 路径 |
| --- | --- |
| 守护进程 | `%LOCALAPPDATA%\sentinel\daemon.log` |
| 状态库 | `%LOCALAPPDATA%\sentinel\state.sqlite3` |
| 已知主机 | `%LOCALAPPDATA%\sentinel\known_hosts` |
| 客户端身份 | `%LOCALAPPDATA%\sentinel\client.json` |
| 用户配置 | `%APPDATA%\sentinel\config.json` |

守护进程默认只输出 info 级别。需要更详细日志时，结束 `daemon.exe` 后以环境变量启动：

`daemon.exe` 是窗口子系统程序（不会弹控制台），因此不能直接在终端里看输出。需要调试日志时，把标准错误重定向到文件：

```powershell
$env:RUST_LOG = "debug"
Start-Process -NoNewWindow -FilePath "$env:LOCALAPPDATA\Programs\Sentinel\resources\daemon.exe" `
  -ArgumentList "--supervise" -RedirectStandardError "$env:TEMP\sentinel-debug.log"
```

## 连接失败怎么定位

1. **先看界面状态**：`连接中` 一直不变多为网络不可达；`重连中` 表示连上过又断了；`失败` 表示不可重试的错误（认证或主机密钥）。
2. **看日志**：`daemon.log` 里搜 `connect failed` / `attach failed` / `ensure failed`，后面会带具体原因。
3. **看远端**：`tmux -L sentinel ls`。会话名形如 `st_<客户端ID前8位>_<主机ID>_<面板ID>`；没有说明会话没建起来。

## 常见问题

**提示 `tmux is not installed`**
远端没有 tmux。安装后重试：`sudo apt install tmux`（或 `yum install tmux`）。

**提示 `host key mismatch`**
服务器重装或换了密钥。确认无误后，删除 `known_hosts` 中对应行，或用"清除记录"重新信任。

**升级后旧会话没接回**
升级若改变了会话命名规则（例如加入客户端命名空间），旧名字的会话不会被接回。它们仍在服务器上，确认不需要后执行 `tmux -L sentinel kill-server`。

**看不到任何输出但会话在跑**
检查是否为旧版本客户端（旧版把二进制帧当 Blob 丢弃）。升级到 0.1.0 之后的版本。

**密码保存后重启就没了**
旧版本使用内存凭据库。当前版本已切换到 Windows Credential Manager，重新输入一次即可。

## 重启守护进程

- **正常重启**（保留远端会话）：在应用内触发 `restart_daemon`（后续会提供菜单项），守护进程先 detach 全部会话再退出，supervisor 立即拉起新进程，GUI 自动接回。
- **强制重启**：任务管理器结束 `daemon.exe`。supervisor 会在 1–10 秒退避内拉起；正在运行的远端 tmux 会话不受影响。
- **连 supervisor 一起重启**：结束 `daemon.exe` 与 supervisor（进程树中同名父进程），然后重新启动应用。

## 协议版本

`daemon.json` 中的 `protocol_version` 与客户端内置版本必须一致。不一致时界面会提示双方版本并要求重启守护进程；远端会话不受影响。

## 清理

| 目标 | 操作 |
| --- | --- |
| 自启项 | 注册表 `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` 下的 `SentinelDaemon` |
| 本地数据 | 删除 `%LOCALAPPDATA%\sentinel` 与 `%APPDATA%\sentinel` |
| 凭据 | Windows 凭据管理器里删除服务名为 `sentinel` 的条目 |
| 远端会话 | 服务器上 `tmux -L sentinel kill-server` |
| 远端配置 | 删除 `~/.cache/sentinel/tmux.conf` |
