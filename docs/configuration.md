# 配置

配置是 `%APPDATA%\sentinel\config.json` 下的普通 JSON，可以手工编辑，下次启动生效。文件不存在时使用默认值；字段缺失时按字段取默认值；未知字段被忽略，方便旧版本读取新版本写出的文件。

Linux/WSL 开发环境使用 `$XDG_CONFIG_HOME/sentinel/config.json`（默认 `~/.config/sentinel/config.json`）；设置 `SENTINEL_CONFIG_DIR` 可覆盖整个目录，测试用它隔离。

## 默认值

```json
{
  "version": 1,
  "font_family": "Cascadia Mono",
  "font_size": 14.0,
  "scrollback_lines": 100000,
  "keepalive_seconds": 30,
  "tmux": {
    "status": false,
    "mouse": false,
    "history_limit": 100000,
    "default_terminal": "tmux-256color"
  },
  "keybindings": {
    "close_pane": "Ctrl+Shift+W",
    "new_tab": "Ctrl+Shift+T",
    "next_tab": "Ctrl+Tab",
    "previous_tab": "Ctrl+Shift+Tab",
    "reconnect_all": "Ctrl+Shift+R",
    "search": "Ctrl+F",
    "toggle_sidebar": "Ctrl+Shift+B"
  }
}
```

## 字段

| 字段 | 约束 | 说明 |
| --- | --- | --- |
| `version` | 必须为 `1` | 配置格式版本；不匹配时拒绝启动并提示 |
| `font_family` | 非空 | 终端字体，必须已安装在系统上 |
| `font_size` | 6 到 72 | 单位 pt |
| `scrollback_lines` | 1000 到 1000000 | 每会话保留的行数，超出后环形覆盖 |
| `keepalive_seconds` | 5 到 600 | SSH 保活间隔；连续 3 次无响应判定断线 |
| `tmux` | 对象 | tmux 注入配置，见下列子字段 |
| `tmux.status` | 布尔 | 是否显示 tmux 状态栏；默认关闭，避免与界面重复 |
| `tmux.mouse` | 布尔 | 是否让 tmux 接管鼠标；默认关闭，鼠标事件透传给应用 |
| `tmux.history_limit` | 1000 到 1000000 | 远端 tmux 的历史行数 |
| `tmux.default_terminal` | 非空 | 注入配置里的 `default-terminal` |
| `keybindings` | 动作 → 快捷键 | 见下表 |

## 快捷键动作

| 动作 | 默认 | 说明 |
| --- | --- | --- |
| `new_tab` | `Ctrl+Shift+T` | 打开快速连接 / 新标签 |
| `close_pane` | `Ctrl+Shift+W` | 关闭当前分屏（= detach，不杀远端） |
| `next_tab` | `Ctrl+Tab` | 下一个标签 |
| `previous_tab` | `Ctrl+Shift+Tab` | 上一个标签 |
| `reconnect_all` | `Ctrl+Shift+R` | 重连所有尚未连接的标签 |
| `toggle_sidebar` | `Ctrl+Shift+B` | 折叠/展开侧边栏 |
| `search` | `Ctrl+F` | 在当前会话内搜索 |

## 校验与错误

校验失败时启动中止并给出字段名与原因，例如：

```
invalid config field `font_size`: must be between 6 and 72, found 200
```

JSON 语法错误时报告文件路径与解析错误位置。系统不会自动"修正"越界值，避免用户以为自己改的设置生效了。

## 凭据

密码、passphrase 与粘贴的私钥**不在这个文件里**。配置只保存 `auth_ref`（指向 Windows Credential Manager 条目的标识）；删除配置不会删除凭据，需要在系统凭据管理中单独清理。
