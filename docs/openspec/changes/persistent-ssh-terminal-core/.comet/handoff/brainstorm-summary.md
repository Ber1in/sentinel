# Brainstorm Summary

- Change: persistent-ssh-terminal-core
- Date: 2026-11-02

## 确认的技术方案

用户已确认正式设计（见 `design.md`，含 frontmatter `role: technical-design`）：

- supervisor（同二进制 `--supervise`）+ daemon + GUI 三段式进程模型；GUI 只是 WS 客户端
- loopback WebSocket + 一次性 token 作为 IPC；控制面 JSON、数据面二进制帧 `[type][sid][seq][bytes]`
- russh 一条连接多通道；tmux provider 用注入配置（status off / mouse off / history-limit）、确定性会话名、`has-session` 判定接回或重建
- 重连状态机：认证与主机密钥错误进终态 `failed`，网络错误退避重试，输入进 64KB 有界队列
- 环形缓冲 100k 行 + 段落化落盘（256 行检查点、50MB 上限、写入失败降级为内存 + 警告）
- 布局用分屏二叉树存 SQLite（rusqlite bundled + WAL），凭据走 Credential Manager，配置与状态分离
- GUI：pane registry + portal 承载 xterm，main 侧 16ms 合并再 send，未激活 pane 不实例化

## 关键取舍与风险

- mingw 交叉编译的 Windows API 兼容性（首个验证点：keyring + 单实例互斥 + 文件 ACL）
- WSLg 与真实 Windows 的 IME/字体差异，中文回归必须真机
- `capture-pane` 无法完整还原全屏程序状态，只保证历史文本与可见屏
- daemon 是单点：supervisor + 自动重连 + 如实标注，不承诺无感
- tmux 注入配置可能改变用户习惯，用独立 `-f` 且可覆盖

## 测试策略

- 单元：协议、状态机、缓冲回绕与索引、路径校验、错误映射、配置 schema
- 集成（WSL 本机 sshd + tmux）：attach/detach、强杀连接、重启 tmux server、回放、resize、吞吐与丢字节检测
- GUI 冒烟（WSLg + Playwright-Electron）：启动、连接、回显、断线恢复、布局持久化
- 真机验收（Windows 11）：IME、字体、凭据、自启、打包与卸载

## Spec Patch

无。
