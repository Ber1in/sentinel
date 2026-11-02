---
change: persistent-ssh-terminal-core
design-doc: docs/openspec/changes/persistent-ssh-terminal-core/design.md
base-ref: 76e1a3b
---

# 实施计划：持久化跨主机 SSH 终端核心

<!-- comet-task-authority: docs/openspec/changes/persistent-ssh-terminal-core/tasks.md -->

本计划按 `tasks.md` 的任务 ID 组织，不复制第二套勾选框。设计依据见 `design.md`（D1–D13），行为契约见 `specs/*/spec.md`。

**执行约定**

- 执行方式：autonomous；TDD：每个实现任务先记录 RED（失败测试及其原因就是待实现行为），再记录 GREEN。
- 环境：WSL2 内开发与测试；本地 `sshd` + `tmux` 是集成靶机；`daemon.exe` 走 `x86_64-pc-windows-gnu` 交叉编译。
- 每个任务完成后用 `comet state task-complete` 勾选，并按需写 checkpoint。
- 不提前实现后续任务；相邻任务若共享同一测试夹具，可在前一个任务内建立夹具。

## 组 1：仓库与构建基线

### 1.1 初始化 Git 仓库
<!-- comet-task-ref: a2769419-e046-4971-b233-8401849c78de -->
- 范围：`git init`、`.gitignore`（`target/`、`node_modules/`、`dist/`、`out/`、构建产物）、首次提交
- 依赖：无
- 约束：不要把 `docs/` 产物排除；`.comet/` 需纳入版本控制
- 验收：`git status` 干净；`git log --oneline` 至少一条；随后把本计划 `base-ref` 回填为该提交

### 1.2 Cargo workspace 与 crate 骨架
<!-- comet-task-ref: 3d42d8ad-ede0-4d39-b428-8cd87a76266a -->
- 范围：安装 rustup（若缺失）、建立 workspace 与 6 个 crate（protocol/daemon/ssh/persistence/terminal/store），各含最小可编译入口
- 依赖：1.1
- 约束：统一 edition 2021+；共享依赖版本集中到 workspace `Cargo.toml`
- 验收：`cargo check --workspace` 通过（首次记录 RED 可用"缺少 crate 导致构建失败"作为起点）

### 1.3 Electron + React + Vite 骨架
<!-- comet-task-ref: 4c5c0959-86db-462e-9a43-faa0073bdfd8 -->
- 范围：`apps/desktop`，main/preload/renderer 三层，空白窗口可启动；用 corepack 或 npm 安装 pnpm
- 依赖：1.2
- 约束：渲染层禁用 nodeIntegration，开启 contextIsolation；preload 只暴露白名单 API
- 验收：`pnpm build` 通过；`pnpm dev` 在 WSLg 下能起窗口

### 1.4 协议类型导出管线
<!-- comet-task-ref: 555bea40-5d6d-4a7f-8fa2-174cae873252 -->
- 范围：`crates/protocol` 用 `ts-rs` 导出 TS 类型到 `apps/desktop/src/shared/protocol.ts`（生成物标记为 generated）
- 依赖：1.2、1.3
- 约束：生成物不允许手改；CI/脚本一条命令可重生成
- 验收：新增一个类型后重生成，TS 侧出现定义且 `tsc --noEmit` 通过

### 1.5 CI 配置
<!-- comet-task-ref: 9090fe9e-d9c3-4973-b75d-e23a9d18bdde -->
- 范围：Linux 上 `cargo test --workspace` + 前端 `pnpm build` 的 workflow；同时提供等价的本地脚本 `scripts/check.sh`
- 依赖：1.2、1.3
- 约束：不引入外部服务依赖；集成测试在 CI 中可用本地 sshd 容器或跳过标记
- 验收：本地 `scripts/check.sh` 全绿；workflow YAML 可解析

### 1.6 README
<!-- comet-task-ref: c7c19623-c09a-4de5-b874-a75d1074f3c2 -->
- 范围：架构概览、目录说明、构建与运行、tmux ≥3.0 要求
- 依赖：1.1–1.5
- 验收：空环境照 README 能启动空白窗口（用记录命令验证）

## 组 2：协议与 IPC

### 2.1 协议消息定义
<!-- comet-task-ref: 8520d21c-4a39-465f-882e-b31988946d42 -->
- 范围：控制消息（会话创建/关闭/接回/结束、resize、状态事件、错误）与二进制帧 `[type][sid][seq][bytes]`
- 依赖：1.4
- 约束：协议带 `protocolVersion`；错误统一 `{code,message,detail}`
- 验收：round-trip 单测覆盖全部消息类型（含非法输入）

### 2.2 单实例 + WS + token
<!-- comet-task-ref: ad972c26-962a-42f8-b439-852e98c8d02d -->
- 范围：Windows 命名互斥单实例；`127.0.0.1:0` 监听；`daemon.json` 写 pid/port/token/version；文件 ACL 限当前用户
- 依赖：2.1
- 约束：仅 loopback；token 校验失败立即断开；WSL 下用文件权限模拟
- 验收：第二实例退出码非零；无 token 连接被断；token 文件权限正确

### 2.3 Electron broker
<!-- comet-task-ref: 07f06a47-74b5-4fd6-b423-fe3c175f1d68 -->
- 范围：main 进程连接 daemon、按 16ms 合并转发、preload 白名单 API、renderer 侧调用
- 依赖：2.2
- 约束：renderer 不能直接碰 socket；断线自动重连
- 验收：跨端 ping/echo 与"会话列表"拉取成功

### 2.4 版本协商
<!-- comet-task-ref: 55c7bcb3-c663-41d8-a418-c3c215f26e82 -->
- 范围：握手交换版本、不兼容拒绝、GUI 提示含双方版本与"重启守护进程"
- 依赖：2.2、2.3
- 验收：单测覆盖兼容/不兼容；提示文案包含双方版本号

### 2.5 协议文档
<!-- comet-task-ref: 96233eb7-f593-4e0e-a076-656272e421eb -->
- 范围：`docs/protocol.md` 消息表、帧格式、错误码
- 依赖：2.1
- 验收：文档示例帧被单测解析通过

## 组 3：SSH 连接层

### 3.1 四种认证与错误分类
<!-- comet-task-ref: dd55726f-be0c-4bc1-acdf-476b376d2604 -->
- 范围：密码、私钥+passphrase、keyboard-interactive、OpenSSH agent；错误映射为凭据/主机密钥/网络/超时
- 依赖：2.1
- 约束：错误分类是状态机分支的唯一依据，禁止用字符串匹配
- 验收：错误映射单测；对本地 sshd 各认证方式跑通一次

### 3.2 主机密钥
<!-- comet-task-ref: 9e07e1ad-c6d5-486a-a37f-aff3a28ba5da -->
- 范围：指纹展示、确认、记忆；变更阻断
- 依赖：3.1
- 约束：首次确认前不得静默信任
- 验收：单测覆盖首次/匹配/不匹配；不匹配时连接被拒

### 3.3 keepalive
<!-- comet-task-ref: 6945b3ee-ee21-4721-9c90-464552b5e4c0 -->
- 范围：可配间隔与失败阈值（默认 30s × 3）
- 依赖：3.1
- 验收：单测确认参数生效；真实连接空闲 30 分钟不断线

### 3.4 单连接多通道
<!-- comet-task-ref: d1fe0943-e195-491d-998e-c9f8168874d1 -->
- 范围：PTY 通道 + 预留子系统通道（先用 `none` 子系统验证通道复用）
- 依赖：3.1
- 验收：同主机同时开两通道时 `ss` 显示仅 1 条到目标端口的连接

### 3.5 吞吐 spike
<!-- comet-task-ref: 4369e342-229c-41a0-8b45-3157a4da6e25 -->
- 范围：`cat` 10MB 与高频日志；记录优化结论
- 依赖：3.4
- 约束：基线 ≥10MB/s 且无丢字节；不达标先调缓冲与批处理
- 验收：`docs/perf.md` 有数据与结论，脚本可复现

## 组 4：tmux provider

### 4.1 Provider trait 与命名
<!-- comet-task-ref: 664161f3-b50a-40bd-936a-b4789c536a4f -->
- 范围：trait 定义、`st_<hostId>_<paneUuid>` 生成
- 依赖：3.4
- 验收：命名幂等与冲突单测

### 4.2 tmux ensure/attach/detach/resize
<!-- comet-task-ref: 1bbb0bcd-6806-4b34-968d-b9567c70889e -->
- 范围：注入配置（status off / mouse off / history-limit / default-terminal）、`new-session -A`、resize
- 依赖：4.1
- 约束：不读用户 `~/.tmux.conf`
- 验收：关闭客户端后 `tmux ls` 仍见会话，重开可接回；`vim` resize 正确

### 4.3 capture-pane 回放
<!-- comet-task-ref: 660adccd-16d2-4563-9a4d-c9c5b3116c21 -->
- 范围：`has-session` 判定接回/重建；`capture-pane -p -e -S -N` 重建历史；attach 后触发重绘
- 依赖：4.2
- 验收：`vim`/`htop` 重连后可用；普通输出颜色抽查一致

### 4.4 tmux 缺失与降级
<!-- comet-task-ref: 658833b9-1ad4-4a8e-8cd9-1edacd76482b -->
- 范围：检测、安装提示、无持久化降级模式与持续标注
- 依赖：4.2
- 验收：无 tmux 环境触发提示；降级后界面持续标注

### 4.5 provider 文档
<!-- comet-task-ref: 1b994678-8b3d-49cd-ab4d-44f6dda69518 -->
- 依赖：4.1–4.4
- 验收：文档参数与代码常量一致（用测试断言）

## 组 5：重连状态机

### 5.1 状态机与退避
<!-- comet-task-ref: 2c2a88b2-7674-4c33-8d8f-dc1aa2c62eae -->
- 范围：六状态 + 1s..30s 退避；认证/主机密钥进 `failed` 不重试
- 依赖：3.2、3.3
- 约束：状态迁移表是唯一事实来源，单测逐条覆盖
- 验收：迁移表单测 100% 覆盖，含非法迁移被拒

### 5.2 输入队列
<!-- comet-task-ref: e0e6063e-1e5d-4df1-9fae-dc1a84bb020a -->
- 范围：64KB 有界队列，满则拒绝并提示
- 依赖：5.1
- 验收：断线输入半行、恢复后仍在且可编辑

### 5.3 重连 + 回放
<!-- comet-task-ref: 50980990-2772-416a-8283-0ac1605df269 -->
- 范围：重连后 reattach、seq 连续性校验、回放去重
- 依赖：5.1、4.3、6.4
- 验收：断网 30s 后日志连续，字节数比对无丢失无重复

### 5.4 会话消失分支
<!-- comet-task-ref: e2bfe591-1bed-4f61-9434-e8da4a18f988 -->
- 范围：`has-session` false 时新建 + 注入 cwd + 标注
- 依赖：5.1、4.3
- 验收：重启 tmux server 后走通该路径，文案正确

## 组 6：终端缓冲

### 6.1 环形缓冲
<!-- comet-task-ref: e755d08a-f9be-4402-aca8-f952d17adcf8 -->
- 范围：100k 行上限、可配置、seq 递增
- 依赖：2.1
- 验收：回绕与上限单测

### 6.2 落盘与裁剪
<!-- comet-task-ref: 4802ef6a-616c-4447-95fa-07fa858b4da1 -->
- 范围：段落文件 + 256 行检查点 + 50MB 裁剪 + 销毁删除 + 写失败降级
- 依赖：6.1
- 验收：单测 + 磁盘文件大小与生命周期检查

### 6.3 回放窗口
<!-- comet-task-ref: e98dfe1d-32fc-4422-a0d9-48bb3cfb81d9 -->
- 范围：attach 回放最近 2k 行、向上按需加载
- 依赖：6.2
- 验收：20k 行后可上翻到最早内容

### 6.4 顺序与去重
<!-- comet-task-ref: 137af141-8bfd-43c1-9c49-8e77c4d979fb -->
- 范围：seq 连续性、缺口检测、快照重建
- 依赖：6.1
- 验收：模拟 5s 卡顿后回放完整无重复

## 组 7：存储与凭据

### 7.1 SQLite schema 与迁移
<!-- comet-task-ref: da3d8efb-3e41-4a00-b7fc-4a811809c640 -->
- 依赖：1.2
- 验收：空库迁移到当前版本的单测

### 7.2 仓储与事务
<!-- comet-task-ref: ce200004-2381-442e-8e47-fc89574973f8 -->
- 依赖：7.1
- 验收：写入中被杀后重开数据为上一完整版本

### 7.3 Credential Manager
<!-- comet-task-ref: 2949f3b2-bcf1-49be-b650-bf8f976247cd -->
- 范围：密码/passphrase/粘贴私钥读写
- 依赖：7.1
- 约束：WSL 下无 Credential Manager，用 trait + 测试替身；Windows 实机验证
- 验收：可读写；配置与日志扫描无明文

### 7.4 配置与校验
<!-- comet-task-ref: 63252ee9-19ce-4901-9fd1-81dbc2c758c9 -->
- 依赖：7.1
- 验收：手改字体/快捷键重启生效；非法字段可读报错

### 7.5 损坏恢复
<!-- comet-task-ref: 4333e0a3-e540-4508-8e60-d894d8b440d5 -->
- 依赖：7.1
- 验收：截断库后提示并可备份重建

### 7.6 配置文档
<!-- comet-task-ref: 34cc9885-8669-4912-ad36-e6c9d3ccd5f4 -->
- 依赖：7.4
- 验收：文档字段与 schema 单测一致

## 组 8：GUI

### 8.1 终端 pane
<!-- comet-task-ref: 2c84a031-0e94-4217-80e5-cf474e3345ca -->
- 范围：xterm + WebGL(+canvas 回退) + unicode11 + search + 链接
- 依赖：2.3
- 验收：中文/emoji 混排、`vim` resize、搜索高亮三项手工通过

### 8.2 标签与跨主机分屏
<!-- comet-task-ref: a757ed33-7721-4e55-aa70-f9ec66cdc886 -->
- 依赖：8.1
- 验收：验收场景 1

### 8.3 侧边栏
<!-- comet-task-ref: afbf0741-4e74-4dbc-971c-e800c16f0d20 -->
- 依赖：8.1、5.1
- 验收：验收场景 4

### 8.4 快速连接
<!-- comet-task-ref: 3a24c951-986a-4f1d-8a1d-cd85e29c5264 -->
- 依赖：8.3、3.1
- 验收：连接成功、主机落库、重启仍在

### 8.5 状态指示
<!-- comet-task-ref: 39f3d16b-4f55-4047-bc07-1f72b0903ea5 -->
- 依赖：5.1、8.3
- 验收：断网 5s 内出现重连提示，恢复后消失

### 8.6 惰性恢复
<!-- comet-task-ref: 2dc2602e-5219-4629-a400-b9a028ffca56 -->
- 依赖：8.2、7.2
- 验收：10 标签重启后 0 条 SSH 连接，点击才建

### 8.7 shell 集成
<!-- comet-task-ref: e9706967-7e03-44a6-8164-24de4016a2f5 -->
- 范围：OSC 7/133 注入与 cwd 记录
- 依赖：8.1
- 验收：`cd` 后目录记录变化；注入失败会话仍可用

### 8.8 复制粘贴
<!-- comet-task-ref: bf1f9b1e-a60d-4e00-80ac-569cc7ec1d30 -->
- 依赖：8.1
- 验收：三行粘贴不逐行执行

## 组 9：daemon 生命周期

### 9.1 自启与拉起
<!-- comet-task-ref: faa94635-46d7-4702-94c7-ac4454699123 -->
- 范围：HKCU Run 指向 `--supervise`；GUI 兜底拉起；单实例
- 依赖：2.2
- 验收：登录后守护在跑；重复启动仅一个实例

### 9.2 看门狗
<!-- comet-task-ref: 45c95353-20cb-4487-9576-cc503aa0eea1 -->
- 依赖：9.1、5.1
- 验收：结束 daemon 后 10s 内恢复并标注正确

### 9.3 优雅重启（升级）
<!-- comet-task-ref: 4f83965e-783a-4435-8715-1e5367e13087 -->
- 依赖：9.2、4.2
- 约束：重启前 detach 全部会话，不得 kill tmux
- 验收：重启后全部标签自动接回

### 9.4 排错文档
<!-- comet-task-ref: a883184b-1811-4ec9-8b93-112c43f91adc -->
- 依赖：9.1–9.3
- 验收：按文档定位一次人为制造的连接失败

## 组 10：集成与交付

### 10.1 验收场景
<!-- comet-task-ref: 8a7aff68-a3e0-4835-9f4d-5f6f7cc83ba8 -->
- 依赖：组 1–9
- 验收：6 条验收场景逐条记录结果

### 10.2 资源基线
<!-- comet-task-ref: 1b4c45f0-16a3-4b0f-8719-c42ce17d5e07 -->
- 依赖：8.x、9.x
- 验收：冷启动内存、10 会话 CPU、回放延迟数据可复现

### 10.3 打包与干净机验证
<!-- comet-task-ref: 92881d9a-5081-4439-be39-40b2af5755a2 -->
- 依赖：10.1
- 约束：Windows 11 真机；卸载需清理自启项与凭据引用
- 验收：安装/首启/卸载全流程无残留

## 风险与回退

- 交叉编译失败 → 切换到 Windows rustup + MSVC，任务 1.2 与 9.x 的验证改在 Windows 上做，不改设计。
- russh 吞吐不达标 → 只调缓冲/批处理，不换 SSH 栈。
- WSLg 下 IME 异常 → 记录为已知限制，IME 回归推迟到 10.1 真机环节。
- `capture-pane` 保真度不足 → 降低承诺为"可见屏 + 历史文本"，不改 spec。
