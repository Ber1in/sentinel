# 任务

## 1. 仓库与构建基线

- [x] 1.1 `git init` 并提交 `.gitignore`（Rust `target/`、Node `node_modules/`、`dist/`、构建产物），验证：`git status` 干净且 `git log` 有一条初始提交 <!-- comet-task:a2769419-e046-4971-b233-8401849c78de -->
- [x] 1.2 建立 Cargo workspace 与 `crates/{protocol,daemon,ssh,persistence,terminal,store}` 骨架，每个 crate 一个最小可编译入口，验证：`cargo check --workspace` 通过 <!-- comet-task:3d42d8ad-ede0-4d39-b428-8cd87a76266a -->
- [x] 1.3 建立 `apps/desktop`（Electron + React + Vite + TypeScript）骨架，能启动空白窗口，验证：`pnpm build` 与 `pnpm dev` 均可启动 <!-- comet-task:4c5c0959-86db-462e-9a43-faa0073bdfd8 -->
- [x] 1.4 建立协议类型导出管线：`crates/protocol` 为单一事实来源，构建期生成 TS 类型文件，验证：新增一个消息类型后重新生成，TS 侧出现对应定义且类型检查通过 <!-- comet-task:555bea40-5d6d-4a7f-8fa2-174cae873252 -->
- [x] 1.5 配置 CI：在 Linux 上跑 `cargo test --workspace` 与前端 `pnpm build`，验证：等价命令在本地全部通过，workflow 文件语法校验通过 <!-- comet-task:9090fe9e-d9c3-4973-b75d-e23a9d18bdde -->
- [x] 1.6 写 `README.md` 的架构概览、目录说明、构建与运行步骤、远端 tmux 版本要求，验证：按 README 从空环境走一遍能启动空白窗口 <!-- comet-task:c7c19623-c09a-4de5-b874-a75d1074f3c2 -->

## 2. 协议与 IPC

- [x] 2.1 定义控制消息（会话创建/关闭/接回/结束、resize、状态事件、错误）与二进制终端帧，含版本字段，验证：round-trip 单测覆盖全部消息类型 <!-- comet-task:8520d21c-4a39-465f-882e-b31988946d42 -->
- [x] 2.2 实现守护进程单实例互斥 + loopback WebSocket 监听 + 一次性 token 文件（限制 ACL），验证：第二个实例启动被拒绝且退出码非零；无 token 的连接被断开 <!-- comet-task:ad972c26-962a-42f8-b439-852e98c8d02d -->
- [ ] 2.3 Electron main 作为唯一 broker 连接 daemon，renderer 经 preload 暴露的最小 API 通讯，验证：跨端 ping/echo 与一次会话列表拉取成功 <!-- comet-task:07f06a47-74b5-4fd6-b423-fe3c175f1d68 -->
- [ ] 2.4 实现协议版本协商：版本不兼容时拒绝连接并给出可读提示，验证：人为降低客户端版本时被拒且提示包含双方版本 <!-- comet-task:55c7bcb3-c663-41d8-a418-c3c215f26e82 -->
- [ ] 2.5 在 `docs/protocol.md` 记录消息表与帧格式，验证：文档中的示例帧能被单测解析 <!-- comet-task:96233eb7-f593-4e0e-a076-656272e421eb -->

## 3. SSH 连接层

- [ ] 3.1 实现四种认证（密码、私钥含 passphrase、keyboard-interactive、Windows OpenSSH agent）并映射错误类别（凭据/主机密钥/网络/超时），验证：错误映射单测 + 对真实主机各跑通一次 <!-- comet-task:dd55726f-be0c-4bc1-acdf-476b376d2604 -->
- [ ] 3.2 实现主机密钥指纹展示、确认与记忆，验证：指纹变更时连接被拒且告警，单测覆盖首次/匹配/不匹配三种路径 <!-- comet-task:9e07e1ad-c6d5-486a-a37f-aff3a28ba5da -->
- [ ] 3.3 实现可配置 keepalive，验证：单测确认间隔与超时参数被正确应用，并在真实连接上观察空闲 30 分钟不断线 <!-- comet-task:6945b3ee-ee21-4721-9c90-464552b5e4c0 -->
- [ ] 3.4 实现单连接多通道（PTY 通道 + 预留 SFTP 子系统通道），验证：同一主机同时打开终端与测试用子系统通道时 TCP 连接数仍为 1 <!-- comet-task:d1fe0943-e195-491d-998e-c9f8168874d1 -->
- [ ] 3.5 吞吐基线 spike：用 `cat` 输出 10MB 与高频日志刷屏，验证：≥10MB/s、缓冲回放无丢字节，把结果与优化结论写入 `docs/perf.md` <!-- comet-task:4369e342-229c-41a0-8b45-3157a4da6e25 -->

## 4. 持久化 provider（tmux）

- [ ] 4.1 定义 `PersistenceProvider` trait 与确定性会话名生成，验证：同名生成幂等、不同标签名冲突的单测 <!-- comet-task:664161f3-b50a-40bd-936a-b4789c536a4f -->
- [ ] 4.2 实现 tmux provider 的 ensure/attach/detach/resize，验证：真实主机上关闭客户端后 `tmux ls` 仍可见会话，重开后接回 <!-- comet-task:1bbb0bcd-6806-4b34-968d-b9567c70889e -->
- [ ] 4.3 实现 `capture-pane` 快照回放并在回放后触发重绘，验证：`vim`/`htop` 重连后画面可用；普通输出颜色抽查一致 <!-- comet-task:660adccd-16d2-4563-9a4d-c9c5b3116c21 -->
- [ ] 4.4 实现 tmux 缺失检测、安装提示与「无持久化」降级模式，验证：无 tmux 的容器中触发提示，选择降级后界面持续标注无持久化 <!-- comet-task:658833b9-1ad4-4a8e-8cd9-1edacd76482b -->
- [ ] 4.5 在 `docs/providers.md` 记录 provider 契约、tmux 参数选择（`-f` 最小配置、`-u`）与降级语义，验证：文档参数与代码常量一致 <!-- comet-task:1b994678-8b3d-49cd-ab4d-44f6dda69518 -->

## 5. 重连状态机

- [ ] 5.1 实现状态机（connecting/connected/reconnecting/detached/lost/failed）与指数退避，认证类错误立即终止，验证：状态迁移表单测全覆盖 <!-- comet-task:2c2a88b2-7674-4c33-8d8f-dc1aa2c62eae -->
- [ ] 5.2 实现重连期间的本地输入队列，验证：断线时输入半行、恢复后内容仍在且可继续编辑 <!-- comet-task:e0e6063e-1e5d-4df1-9fae-dc1a84bb020a -->
- [ ] 5.3 实现重连后 reattach + 缓冲回放，验证：断网 30 秒后日志连续、无重复无丢失（集成测试比对字节数） <!-- comet-task:50980990-2772-416a-8283-0ac1605df269 -->
- [ ] 5.4 实现「会话已不存在」分支：新建会话、注入上次 cwd、标注进程可能丢失，验证：重启目标主机后走通该路径且界面文案正确 <!-- comet-task:e2bfe591-1bed-4f61-9434-e8da4a18f988 -->

## 6. 终端缓冲

- [ ] 6.1 实现内存环形缓冲（默认 100k 行上限，可配置），验证：单测覆盖回绕与行数上限 <!-- comet-task:e755d08a-f9be-4402-aca8-f952d17adcf8 -->
- [ ] 6.2 实现增量落盘、50MB 上限裁剪与会话销毁时删除，验证：单测 + 检查磁盘文件大小与存活周期 <!-- comet-task:4802ef6a-616c-4447-95fa-07fa858b4da1 -->
- [ ] 6.3 实现 attach 时回放最近 2k 行与向上按需加载更早片段，验证：输出 20k 行后重开 GUI 可上翻到最早内容 <!-- comet-task:e98dfe1d-32fc-4422-a0d9-48bb3cfb81d9 -->
- [ ] 6.4 实现回放顺序与去重保证（带序号），验证：模拟 GUI 卡顿 5 秒后回放序列完整且无重复 <!-- comet-task:137af141-8bfd-43c1-9c49-8e77c4d979fb -->

## 7. 状态存储与凭据

- [ ] 7.1 实现 SQLite schema（host/tab/pane/session）与版本化迁移，验证：迁移单测可从空库升级到当前版本 <!-- comet-task:da3d8efb-3e41-4a00-b7fc-4a811809c640 -->
- [ ] 7.2 实现主机、标签、分屏树、会话的仓储与事务写入，验证：写入过程中被杀后重开，数据为上一次完整版本 <!-- comet-task:ce200004-2381-442e-8e47-fc89574973f8 -->
- [ ] 7.3 实现 Windows Credential Manager 存取（密码、passphrase、粘贴私钥），验证：可读写；配置文件与日志扫描不到明文 <!-- comet-task:2949f3b2-bcf1-49be-b650-bf8f976247cd -->
- [ ] 7.4 实现配置 JSON 读取与 schema 校验（字体、快捷键、缓冲上限、tmux 选项、keepalive），验证：手改字体与快捷键后重启生效，非法字段给出可读错误 <!-- comet-task:63252ee9-19ce-4901-9fd1-81dbc2c758c9 -->
- [ ] 7.5 实现损坏检测与「备份后重建」流程，验证：截断状态库后启动给出提示，确认后生成新库且原文件被保留为备份 <!-- comet-task:4333e0a3-e540-4508-8e60-d894d8b440d5 -->
- [ ] 7.6 在 `docs/configuration.md` 记录全部配置字段与默认值，验证：文档字段与配置 schema 单测一致 <!-- comet-task:34cc9885-8669-4912-ad36-e6c9d3ccd5f4 -->

## 8. GUI：会话与布局

- [ ] 8.1 实现终端 pane 组件（xterm.js + WebGL + unicode11 + 搜索 + 链接），验证：中文/emoji 混排样例、`vim` 改尺寸重绘、搜索高亮三项手工通过 <!-- comet-task:2c84a031-0e94-4217-80e5-cf474e3345ca -->
- [ ] 8.2 实现标签与跨主机横向/纵向分屏（含拖拽与尺寸持久化），验证：验收场景 1 手工通过 <!-- comet-task:a757ed33-7721-4e55-aa70-f9ec66cdc886 -->
- [ ] 8.3 实现侧边栏（主机分组、最近连接、后台会话、30 天清理提示、「全部重连」入口），验证：验收场景 4 手工通过 <!-- comet-task:afbf0741-4e74-4dbc-971c-e800c16f0d20 -->
- [ ] 8.4 实现快速连接 `user@host[:port]` 并保存为主机，验证：输入后连接成功且主机出现在侧边栏，重启后仍在 <!-- comet-task:3a24c951-986a-4f1d-8a1d-cd85e29c5264 -->
- [ ] 8.5 实现会话状态指示与重连提示条，验证：拔网线后 5 秒内出现重连中，恢复后自动消失 <!-- comet-task:39f3d16b-4f55-4047-bc07-1f72b0903ea5 -->
- [ ] 8.6 实现启动惰性恢复与「全部重连」，验证：10 个标签重启后流量抓包显示 0 条 SSH 连接，点击标签才建立 <!-- comet-task:2dc2602e-5219-4629-a400-b9a028ffca56 -->
- [ ] 8.7 实现 shell 集成注入（OSC 7 / OSC 133）与 cwd 记录，验证：`cd` 后会话记录目录变化；注入失败时会话仍可正常使用 <!-- comet-task:e9706967-7e03-44a6-8164-24de4016a2f5 -->
- [ ] 8.8 实现复制粘贴与 bracketed paste，验证：三行命令一次粘贴后不产生分段执行回显 <!-- comet-task:bf1f9b1e-a60d-4e00-80ac-569cc7ec1d30 -->

## 9. 守护进程生命周期

- [ ] 9.1 实现登录自启与 GUI 侧自动拉起，验证：注销重登后守护进程在运行；GUI 在守护进程缺失时能拉起且只产生一个实例 <!-- comet-task:faa94635-46d7-4702-94c7-ac4454699123 -->
- [ ] 9.2 实现看门狗：守护进程崩溃后自动重启并重连全部可恢复会话，验证：任务管理器结束守护进程后 10 秒内恢复且会话标注正确 <!-- comet-task:45c95353-20cb-4487-9576-cc503aa0eea1 -->
- [ ] 9.3 实现按需重启守护进程（升级场景）且不丢远端会话，验证：重启守护进程后所有标签自动接回，无远端会话被销毁 <!-- comet-task:4f83965e-783a-4435-8715-1e5367e13087 -->
- [ ] 9.4 在 `docs/troubleshooting.md` 记录日志位置、协议版本查看方式、守护进程重启方法与凭据清理步骤，验证：按文档可定位一次人为制造的连接失败 <!-- comet-task:a883184b-1811-4ec9-8b93-112c43f91adc -->

## 10. 集成与交付

- [ ] 10.1 端到端跑通 proposal 中的 6 条验收场景，输出 `docs/reports/acceptance.md` 记录每条的实测结果与截图/日志，验证：6 条全部通过或明确记录失败原因 <!-- comet-task:8a7aff68-a3e0-4835-9f4d-5f6f7cc83ba8 -->
- [ ] 10.2 采集资源基线（冷启动内存、10 会话 CPU、回放延迟）写入 `docs/perf.md`，验证：数据可复现，脚本随仓库提交 <!-- comet-task:1b4c45f0-16a3-4b0f-8719-c42ce17d5e07 -->
- [ ] 10.3 打包安装包与便携 zip，在干净的 Windows 11 上验证安装、首启、卸载后自启项与凭据引用被清理，验证：干净机器全流程无残留 <!-- comet-task:92881d9a-5081-4439-be39-40b2af5755a2 -->

## Workflow follow-up

- 项目审查要求满足后归档本 change。
- 归档后开始 `sftp-file-panel`。
