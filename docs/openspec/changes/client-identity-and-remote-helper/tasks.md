# 任务

## 1. helper crate 与协议

- [x] 1.1 新建 `crates/helper`，实现 `version` 与 `install-check`（架构/libc/目录可写/noexec），验证：在本机与一个 noexec 目录下输出符合预期 <!-- comet-task:ace6ce33-80d1-4fde-9ade-76a07b9d2bdc -->
- [x] 1.2 定义帧协议（长度 + JSON 头 + 二进制体）与控制消息（hello/list/create/attach/detach/resize/destroy/data/event），验证：round-trip 单测覆盖全部消息与非法帧 <!-- comet-task:6f2e705e-28ce-4ef6-a4e3-9ffa97a027aa -->
- [x] 1.3 实现 `proxy` 子命令：stdio ↔ Unix socket 双向桥接，验证：本地起 helper 后用 `printf` 管道完成一次 hello 往返 <!-- comet-task:a0b5b119-90ed-4daa-8651-5cb8dea48f84 -->

## 2. 会话运行时

- [x] 2.1 用 `openpty` 实现会话创建与进程启动（登录 shell、可指定 cwd），验证：创建后能在 pty 中执行命令并读到输出 <!-- comet-task:b5d924e8-68f6-4632-9ffe-e1aa652500cd -->
- [x] 2.2 实现环形缓冲与 attach 回放，验证：输出 20k 行后 attach 能取回最近窗口 <!-- comet-task:60be20d9-633b-4149-bfa2-a290c93a8b79 -->
- [x] 2.3 实现 detach/attach/resize/destroy 与进程组清理，验证：detach 后进程仍在，destroy 后进程消失 <!-- comet-task:3230b8ab-b73d-4b24-b3d6-856cab4b4fd2 -->
- [x] 2.4 实现按 `client_id` 的命名空间隔离，验证：两个 namespace 的 list 互不可见，跨 namespace attach 被拒绝 <!-- comet-task:a6459d41-67a1-4a16-a5f1-a1b2e56afb55 -->
- [x] 2.5 实现单实例锁与空闲退出，验证：第二个 serve 退出；无会话超时后进程结束 <!-- comet-task:599294c8-342a-4433-b7fd-2ed89ce32103 -->

## 3. 自动部署

- [x] 3.1 实现远端探测（uname/ldd/目录写权限/noexec），验证：普通用户目录、只读目录、noexec 目录三种情况判定正确 <!-- comet-task:f174267d-d590-4e72-864a-d2512147fd6a -->
- [x] 3.2 实现上传与原子安装（临时文件 → chmod → sha256 → mv），验证：正常安装成功；哈希不符时删除且报错 <!-- comet-task:fc383ecb-5d0e-4605-a6ae-9b6f099af2df -->
- [x] 3.3 实现 SFTP 上传与 `base64 -d` 回退，验证：禁用 SFTP 子系统时仍能安装 <!-- comet-task:284242ea-0440-4629-9b55-2cad955bc318 -->
- [ ] 3.4 实现启动与就绪等待（socket 出现 ≤3s），验证：安装后 3 秒内可完成一次 attach <!-- comet-task:629d1654-a416-401a-823a-0315d304da38 -->
- [ ] 3.5 实现版本比对与延迟升级，验证：有活跃会话时标记 pending，会话结束后替换 <!-- comet-task:4f33edbe-50ec-4973-9244-4db7e72abed7 -->

## 4. 集成到客户端

- [ ] 4.1 在 `crates/persistence` 实现 `HelperProvider`（attach/detach/resize/snapshot），验证：真实主机上创建会话并回放 <!-- comet-task:dd25e037-8563-4397-aa93-da13c4450fce -->
- [ ] 4.2 在 manager 中实现 provider 优先级（helper → tmux → none）与"选中原因"，验证：三种路径各走通一次 <!-- comet-task:967f5f61-5a62-4017-a36c-3945ff873660 -->
- [ ] 4.3 首次连接确认框（主机、路径、大小、sha256）与"该主机不再询问"，验证：确认后安装、拒绝后回退且不写文件 <!-- comet-task:c9f03733-b707-4fa2-a806-36358b89e726 -->
- [ ] 4.4 界面展示 provider 与降级原因，验证：helper/tmux/none 三种标注正确 <!-- comet-task:8b8a268d-cffc-4267-850b-320fba5c77af -->

## 5. 端到端与文档

- [ ] 5.1 端到端：全新服务器（无 tmux）上首次连接自动安装并创建会话，验证：全程无手工命令 <!-- comet-task:24944ec2-ffc3-4a06-955d-19bc6f3171d5 -->
- [ ] 5.2 断线恢复：attach 后杀掉 SSH 连接再重连，验证：shell 状态与输出保留 <!-- comet-task:289ec526-0ce6-4aec-b406-bc5a2fca8329 -->
- [ ] 5.3 编写 `docs/helper.md`：部署路径、socket 权限、安全边界（同账号风险）、卸载与清理 <!-- comet-task:2754287c-31a8-4787-8685-d9f49dcbbdf7 -->
- [ ] 5.4 回归：helper 不可用时 tmux 路径仍正常，验证：删除 helper 后连接走 tmux <!-- comet-task:33d10095-549b-49cc-8bd0-c46a9cdd948d -->

## Workflow follow-up

- 项目审查要求满足后归档本 change。
