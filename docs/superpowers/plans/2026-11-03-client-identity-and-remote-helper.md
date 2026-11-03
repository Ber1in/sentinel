---
change: client-identity-and-remote-helper
design-doc: docs/openspec/changes/client-identity-and-remote-helper/design.md
base-ref: 2bce5e4
---

# 实施计划：远端 helper 与自动安装

<!-- comet-task-authority: docs/openspec/changes/client-identity-and-remote-helper/tasks.md -->

按 `tasks.md` 的 18 项推进；设计见 `design.md`（D1–D7）。执行方式 autonomous，TDD 开启，审查 standard。

## 交付顺序

1. **协议与代理**（1.1–1.3）：先让 `helper proxy` 能用，否则后面无法测。
2. **运行时**（2.1–2.5）：PTY、缓冲、attach/detach、命名空间、空闲退出。
3. **部署**（3.1–3.5）：探测 → 上传校验 → 启动 → 升级。
4. **客户端接入**（4.1–4.4）：`HelperProvider`、优先级、确认框、界面标注。
5. **端到端与文档**（5.1–5.4）。

## 任务映射

<!-- comet-task-ref: ace6ce33-80d1-4fde-9ade-76a07b9d2bdc -->
<!-- comet-task-ref: 6f2e705e-28ce-4ef6-a4e3-9ffa97a027aa -->
<!-- comet-task-ref: a0b5b119-90ed-4daa-8651-5cb8dea48f84 -->
<!-- comet-task-ref: b5d924e8-68f6-4632-9ffe-e1aa652500cd -->
<!-- comet-task-ref: 60be20d9-633b-4149-bfa2-a290c93a8b79 -->
<!-- comet-task-ref: 3230b8ab-b73d-4b24-b3d6-856cab4b4fd2 -->
<!-- comet-task-ref: a6459d41-67a1-4a16-a5f1-a1b2e56afb55 -->
<!-- comet-task-ref: 599294c8-342a-4433-b7fd-2ed89ce32103 -->
<!-- comet-task-ref: f174267d-d590-4e72-864a-d2512147fd6a -->
<!-- comet-task-ref: fc383ecb-5d0e-4605-a6ae-9b6f099af2df -->
<!-- comet-task-ref: 284242ea-0440-4629-9b55-2cad955bc318 -->
<!-- comet-task-ref: 629d1654-a416-401a-823a-0315d304da38 -->
<!-- comet-task-ref: 4f33edbe-50ec-4973-9244-4db7e72abed7 -->
<!-- comet-task-ref: dd25e037-8563-4397-aa93-da13c4450fce -->
<!-- comet-task-ref: 967f5f61-5a62-4017-a36c-3945ff873660 -->
<!-- comet-task-ref: c9f03733-b707-4fa2-a806-36358b89e726 -->
<!-- comet-task-ref: 8b8a268d-cffc-4267-850b-320fba5c77af -->
<!-- comet-task-ref: 24944ec2-ffc3-4a06-955d-19bc6f3171d5 -->
<!-- comet-task-ref: 289ec526-0ce6-4aec-b406-bc5a2fca8329 -->
<!-- comet-task-ref: 2754287c-31a8-4787-8685-d9f49dcbbdf7 -->
<!-- comet-task-ref: 33d10095-549b-49cc-8bd0-c46a9cdd948d -->

