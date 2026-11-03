# 端到端验收记录（任务 10.1）

- 变更：`persistent-ssh-terminal-core`
- 文档性质：**待执行的验收清单**。六条场景的结果字段统一为 `待验证`，日期为占位符，等真机执行后回填。
- 编写日期：2026-11-03
- 执行人：`________`
- 执行机器（Windows 11）：`________`（版本 / 内存 / 是否有独显）
- 当前状态：全部 6 条 **未执行**（本仓库的 GUI 交互只能在 Windows 真机上做，见文末「必须人工完成的部分」）

## 关于「proposal 中的 6 条验收场景」

`proposal.md` 没有编号的「验收场景」小节；`specs/*/spec.md` 里是 55 条细粒度 Scenario。为了不凭空造需求，下面六条是从 proposal「变更内容」的六项承诺归纳出的**端到端可执行路径**，每条标注它实际覆盖的 spec 场景；完整映射见 [附录 A](#附录-a与-spec-场景的映射)。

| ID | 场景 | 覆盖的承诺 |
| --- | --- | --- |
| A1 | 一个窗口内跨主机标签与分屏 | 跨主机标签/分屏、单一窗口多主机 |
| A2 | 强制结束 GUI 后会话仍在，重开接回 | GUI 只是视图；关闭/崩溃不中断远端 |
| A3 | 断网 30 秒自动重连，输入不丢、回放不重复 | 断线自动恢复、输入队列、缓冲回放去重 |
| A4 | 守护进程被杀后自愈，且始终单实例 | 守护进程生命周期、崩溃恢复、单实例 |
| A5 | 关闭标签 = 脱离，从后台列表接回 | 会话语义、后台会话、30 天提示 |
| A6 | 十个标签惰性恢复，同主机只复用一条连接 | 布局持久化、惰性恢复、连接复用 |

## 结果总览

> 结论三选一：`通过` / `失败` / `阻塞`。失败或阻塞时必须在对应小节写清原因与下一步。

| ID | 场景 | 依赖任务 | 结果 | 执行日期 | 证据 |
| --- | --- | --- | --- | --- | --- |
| A1 | 跨主机标签与分屏 | 8.1、8.2 | 待验证 | `____-__-__` | `docs/reports/evidence/10.1/a1-*.png` |
| A2 | GUI 被杀后会话存活 | 8.1、2.2、9.x | 待验证 | `____-__-__` | `docs/reports/evidence/10.1/a2-*.png` |
| A3 | 断网 30 秒自动重连 | 5.1–5.3、8.5 | 待验证 | `____-__-__` | `docs/reports/evidence/10.1/a3-*.png` |
| A4 | 守护进程自愈与单实例 | 2.2、9.1、9.2 | 待验证 | `____-__-__` | `docs/reports/evidence/10.1/a4-*.png` |
| A5 | detach 与后台会话 | 4.2、8.3 | 待验证 | `____-__-__` | `docs/reports/evidence/10.1/a5-*.png` |
| A6 | 十标签惰性恢复与连接复用 | 3.4、8.6、8.13 | 待验证 | `____-__-__` | `docs/reports/evidence/10.1/a6-*.png` |

证据目录约定：截图与抓取的日志放到 `docs/reports/evidence/10.1/`（目录尚未创建，执行时新建；截图文件名按上表前缀）。日志摘录可直接贴进对应小节的「实测结果」，但必须带时间戳行。

## 通用前置条件

- [ ] Windows 11 x64，已安装 Sentinel（`/mnt/d/Downloads/Sentinel Setup 0.1.0.exe`，或直接运行便携版 `Sentinel 0.1.0.exe`）。
- [ ] 两台**独立** Linux 主机（下称 `host-A`、`host-B`），均满足：OpenSSH server 已启动、`tmux -V` ≥ 3.0、用户可登录、家目录可写。
      *只有一台远端时*：在同一台机器上再起一个 sshd（不同端口或不同用户）代替 `host-B`，并在证据里注明这是降级执行，A1 的「跨主机」结论要打折。
- [ ] `host-A`/`host-B` 上 `command -v tmux` 有输出；`tmux -L sentinel ls` 可执行（首次为空是正常的）。
- [ ] 已准备好两种认证：一个可用的密码，一把可用的私钥（含或不含 passphrase 均可）。A1 用密码，A2 用私钥，顺便覆盖两种路径。
- [ ] 记录基线：执行前先在两台远端上跑 `tmux -L sentinel ls`，确认没有残留的 `st_<客户端ID>_*` 会话（有则先 `tmux -L sentinel kill-server`）。
- [ ] 知道三个位置：远端 `tmux -L sentinel ls`、本机 `%LOCALAPPDATA%\sentinel\daemon.log`、本机 `%LOCALAPPDATA%\sentinel\state.sqlite3`。

### 执行前预检（2026-11-03，代码核对，不是实测结论）

以下四点是在 Linux 侧读代码时发现的**当前实现缺口**。验收时若行为与之一致，请把对应步骤记为 `失败` 并引用本小节，而不是当成偶发：

| 缺口 | 现象 | 位置 |
| --- | --- | --- |
| 「全部重连」只有文案、没有入口 | 侧边栏/命令面板里找不到该动作 | `apps/desktop/src/renderer/i18n.ts` 有 `action.reconnectAll`，`App.tsx` 未使用 |
| 30 天未访问的清理提示没有实现 | 打开应用不会出现清理提示 | 代码中无 30 天判定（`store` 只存 `last_active_at`） |
| 后台会话条目只显示标题 + provider | 不显示 spec 要求的主机名、最后活动时间、最后一行摘要 | `App.tsx` 侧边栏「已打开会话」列表 |
| 渲染层没有「显式结束会话」入口 | 只能关闭标签（detach），无法从界面销毁远端会话 | `preload` 暴露了 `destroy`，`App.tsx` 未调用 |
| 快速连接 `user@host[:port]` 未实现 | 命令面板只能搜已保存主机 + 新建连接 | 命令面板组件 |

这五项分别属于任务 8.3 / 8.4，与本任务并行推进；执行验收时以当时的 `tasks.md` 勾选状态为准，若已补上则忽略本表。

## A1 跨主机标签与分屏（一个窗口同时操作两台机器）

**目标**：验证「主机是会话属性而不是容器属性」——同一窗口里两个标签、一次分屏可以来自不同主机。

**覆盖**：multi-host-workspace「一个窗口内连接两台主机」「左右分屏同时操作两台机器」「重启后布局一致」；任务 8.2。

**前置**：通用前置条件；`host-A`、`host-B` 已在侧边栏（没有则用「新建连接」添加）。

**步骤**

1. 启动 Sentinel。侧边栏「主机」分组下应出现 `host-A`、`host-B`。
2. 点击 `host-A`，等待标签状态胶囊变为 **已连接**，执行 `hostname`。
3. 点标签条右侧的 `+`，连接 `host-B`，执行 `hostname`。此时应有两个标签，主机色标不同。
4. 在 `host-B` 标签上右键 → **向右分屏** → 在新分屏里连接 `host-A`，执行 `cd /tmp && pwd`。
5. 在左侧分屏输入 `echo LEFT`，右侧分屏输入 `echo RIGHT`，确认输入不会串到另一个分屏。
6. 拖动两个分屏之间的分隔条调整比例。
7. 完全退出 GUI（关闭窗口，不结束守护进程），重新启动 Sentinel。
8. 检查标签顺序、分屏方向与比例是否与退出前一致；逐一点击标签，确认接回原有会话（`hostname` 与上一步相同，且 `cd /tmp` 的目录还在）。

**预期结果**

- 一个窗口内同时存在两台主机的会话，标签与分屏互不影响；
- 分屏尺寸可拖拽，重启后布局一致；
- 逐标签惰性恢复：点击才连接，未点击的标签不发起到远端的 SSH 连接（A6 会量化这一点）。

**实测结果**：待验证（`____-__-__`，执行人 `____`）

```
结论：
备注：
```

**证据**

- [ ] `a1-1-tabs.png`：两个标签、不同主机色标，各自 `hostname` 输出并存
- [ ] `a1-2-split.png`：左右分屏同时显示两台主机
- [ ] `a1-3-restart-layout.png`：重启后布局一致
- [ ] 远端 `tmux -L sentinel ls` 输出（应看到两条 `st_*` 会话，名字不同）

## A2 强制结束 GUI 后会话仍在，重开接回

**目标**：验证 GUI 只是视图——进程被强杀不终止 SSH 连接与远端进程，滚动缓冲可回放。

**覆盖**：terminal-session-persistence「强制结束 GUI 后会话仍在」；terminal-emulation「重开 GUI 后仍能向上翻历史」；任务 2.2、8.1。

**前置**：`host-A` 已连接（用私钥认证，顺便覆盖私钥路径）。

**步骤**

1. 连接 `host-A`，运行 `top`（或 `htop`）；再运行一条会持续输出的命令，例如 `while true; do date; sleep 1; done`，让它刷 60 秒以上产生足够历史。
2. 在远端另开一个 SSH 会话（或本机 WSL）执行 `tmux -L sentinel ls` 并截图，记录会话名。
3. 打开任务管理器 → 详细信息 → 结束 `Sentinel.exe` 的所有进程（**不要**结束 `daemon.exe` 与 supervisor）。
4. 等待 10 秒，在远端再次执行 `tmux -L sentinel ls`，确认会话仍在。
5. 重新启动 Sentinel。
6. 打开对应标签，确认画面恢复：`top` 仍在刷新、`date` 循环仍在输出；向上滚动查看结束 GUI 之前的输出。
7. 在恢复后的终端里输入 `echo after-restart`，确认可继续交互。

**预期结果**

- GUI 被杀后守护进程与远端会话均不受影响（远端 `tmux ls` 仍有该会话，`top` 未被杀）；
- 重开 GUI 后自动接回，画面与滚动缓冲连续；
- 接回时先回放最近 2000 行（`REPLAY_LINES`），向上可继续翻更早的内容。

**实测结果**：待验证（`____-__-__`，执行人 `____`）

```
结论：
备注：若缓冲在 GUI 关闭期间丢失，记录"丢失起点"与远端 tmux 的输出行数以便定位
```

**证据**

- [ ] `a2-1-before-kill.png`：运行中的 `top` + 循环输出
- [ ] `a2-2-tmux-after-kill.png`：GUI 被杀后远端 `tmux ls` 仍有会话
- [ ] `a2-3-replay.png`：重开后画面恢复，能向上翻到杀进程之前
- [ ] `daemon.log` 中该会话的 `reattach` / `connected` 时间戳

## A3 断网 30 秒自动重连：输入不丢、回放不重复

**目标**：验证重连状态机、断线输入队列、回放去重三件事同时成立。

**覆盖**：ssh-connection-management「网络闪断后自动恢复」「断线时输入半行命令」；terminal-emulation「卡顿期间的输出不丢」；terminal-session-persistence「断网时状态变化可

见」；任务 5.1–5.3、8.5。

**前置**：`host-A` 已连接；知道 `host-A` 的 IP 用来写防火墙规则。

**步骤**

1. 连接 `host-A`，运行 `ping -i 1 127.0.0.1 | ts`（或 `while true; do date +%s; sleep 1; done`）产生带时间戳的连续输出，作为「无重复、无丢失」的判据。
2. 在终端里输入半行命令**但不要回车**，例如 `echo half-line-still-here`（让光标停在行尾）。
3. 切断到 `host-A` 的网络，二选一：
   - **防火墙**（推荐，可精确恢复）：`netsh advfirewall firewall add rule name="sentinel-block-A" dir=out action=block remoteip=<host-A-IP>`；
   - **物理**：拔网线或禁用网卡。
4. 观察界面：状态胶囊应在数秒内变为 **重连中**（≤5 秒，见任务 8.5）。记录出现时间。
5. 保持断网 30 秒。期间再敲几个字符（追加在那半行后面）。
6. 恢复网络：`netsh advfirewall firewall delete rule name="sentinel-block-A"`（或插回网线）。
7. 等待状态自动变回 **已连接**（无需点击任何按钮），观察终端内容。
8. 按回车执行那半行命令。

**预期结果**

- 断网 5 秒内出现「重连中」，恢复后自动「已连接」，全程无需人工操作；
- 断网期间敲的字符在恢复后仍在同一行、可继续编辑；回车后完整执行；
- 时间戳输出连续：断网 30 秒期间远端仍在跑（tmux 会话存活），恢复后补齐，**不重复**已显示过的行；
- 没有出现重复行、乱序行或半截 ANSI 序列。

**实测结果**：待验证（`____-__-__`，执行人 `____`）

```
状态变为"重连中"用时：____ 秒
恢复为"已连接"用时：____ 秒
重复行计数：____（应为 0）
丢失行计数：____
结论：
```

**证据**

- [ ] `a3-1-reconnecting.png`：断网后「重连中」状态截图（带系统时间）
- [ ] `a3-2-recovered.png`：恢复后「已连接」+ 时间戳连续
- [ ] `a3-3-half-line.png`：重连后那半行仍在
- [ ] `daemon.log` 中 reconnecting → connected 的状态迁移与原因（`link lost` / `connect failed`）

## A4 守护进程被杀后自愈，且始终单实例

**目标**：验证 supervisor 的看门狗与单实例约束；崩溃恢复不销毁远端会话。

**覆盖**：terminal-session-persistence「守护进程被杀后自动恢复」「重复启动不产生第二个守护进程」；任务 2.2、9.1、9.2。

**前置**：至少一个 `host-A` 会话在跑；已确认 Windows 登录自启项存在（注册表 `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` 下有 `SentinelDaemon`）。

**步骤**

1. 连接 `host-A`，运行 `sleep 600` 之类的长任务；远端 `tmux -L sentinel ls` 记录会话。
2. 任务管理器结束 `daemon.exe`（只结束这一个，**不要**动 supervisor 的父进程）。
3. 用秒表/日志时间戳记录守护进程重新出现的时间，并观察 GUI 是否自动接回会话。
4. 连续启动 Sentinel 两次（开始菜单点两次），然后在任务管理器里按名称排序，确认 `daemon.exe` 只有一个实例。
5. 注销并重新登录 Windows，确认守护进程随登录自启（进程存在，且 GUI 未启动也不影响）。
6. 远端再执行 `tmux -L sentinel ls`，确认 `sleep 600` 的会话没有被销毁。

**预期结果**

- 杀掉守护进程后 10 秒内自动重启（supervisor 退避 1–10 秒），GUI 自动重连并标注恢复结果；
- 无论启动几次 GUI，`daemon.exe` 只有一个实例，多个窗口共享同一批会话；
- 注销重登后守护进程在运行；
- 远端会话未被销毁（进程仍在跑）。

**实测结果**：待验证（`____-__-__`，执行人 `____`）

```
守护进程恢复用时：____ 秒
daemon.exe 实例数：____（应为 1）
注销重登后是否自启：是 / 否
远端 sleep 会话是否存活：是 / 否
结论：
```

**证据**

- [ ] `a4-1-killed.png`：结束 daemon 后的界面（离线提示或重连中）
- [ ] `a4-2-recovered.png`：恢复后自动接回
- [ ] `a4-3-single-instance.png`：任务管理器中只有一个 `daemon.exe`
- [ ] `a4-4-run-key.png`：注册表自启项截图
- [ ] `daemon.log` 中 supervisor 的 `daemon exited unexpectedly` / 重启记录

## A5 关闭标签 = 脱离；后台列表接回；30 天提示

**目标**：验证会话语义——关闭标签不杀进程，后台会话可见可接回，长期未访问只提示不删除。

**覆盖**：terminal-session-persistence「关闭标签后从后台列表接回」「后台会话列表内容正确」「提示清理但不自动删除」；任务 4.2、8.3。

**前置**：`host-A`、`host-B` 各有一个会话；知道如何修改本机时钟（**仅测试机**，见步骤 6 的风险提示）。

**步骤**

1. 在 `host-A` 标签里运行 `top`，在 `host-B` 标签里运行 `sleep 900`。
2. 关闭这两个标签（点标签上的 ×，或右键 → 关闭）。**不要**点「结束会话」。
3. 远端执行 `tmux -L sentinel ls`：两个会话都应仍在；`top` 进程未被杀（`tmux -L sentinel attach -t <name>` 可验证，验证后 `Ctrl-b d` 脱离）。
4. 看侧边栏「已打开会话」区：应出现两条脱离记录。核对主机名、标签名、最后活动时间、最后一行摘要是否与真实情况一致、排序是否按最后活动时间倒序。
5. 点击其中一条，确认重新接回：`top` 仍在刷新，且显示离开期间产生的输出。
6. 30 天提示：把某个后台会话的 `last_active_at` 手工改到 31 天前（`state.sqlite3` 的 `session` 表），重启 GUI，确认出现清理提示。
   - 风险提示：直接改库前先备份 `state.sqlite3`，改完验证完立即还原；不要在主力机上操作。
   - 若界面**没有**清理提示，按文末预检表记为 `失败`（当前实现缺口），并在结论里写明「提示未实现」，而不是记为通过。
7. 在提示中选择「取消 / 不清理」，确认会话仍在远端、后台列表仍显示。

**预期结果**

- 关闭标签只 detach：远端会话与进程存活，本地缓冲保留；
- 后台列表内容正确（主机名、最后活动时间、摘要、排序）；
- 点击后台条目可接回并补齐离开期间的输出；
- 超 30 天只提示，用户拒绝后不销毁任何远端会话。

**实测结果**：待验证（`____-__-__`，执行人 `____`）

```
关闭标签后远端会话数：____（应为 2）
后台列表条目数：____ / 字段完整度：主机名 有/无，最后活动时间 有/无，摘要 有/无
接回是否补齐离开期间输出：是 / 否
30 天提示：出现 / 未出现（未出现时引用预检表）
结论：
```

**证据**

- [ ] `a5-1-closed-tab.png`：关闭标签后侧边栏后台列表
- [ ] `a5-2-tmux-ls.png`：关闭标签后远端会话仍在
- [ ] `a5-3-reattach.png`：接回后 `top` 仍在跑
- [ ] `a5-4-stale-prompt.png`：30 天清理提示（未实现则附"没有提示"的截图）

## A6 十个标签惰性恢复，同主机只复用一条连接

**目标**：验证布局持久化 + 惰性恢复 + 连接复用，同时给出 10 会话的资源基线入口。

**覆盖**：multi-host-workspace「重启后布局一致」「十个标签不触发十次握手」；terminal-session-persistence「升级后仍能接回旧会话」；ssh-connection-management「同一会话不被重复创建」；任务 3.4、8.6、8.13。

**前置**：`host-A`、`host-B` 可用；能执行 `Get-NetTCPConnection`（PowerShell，无需管理员）与远端 `ss`。

**步骤**

1. 建立 10 个标签：`host-A` 上 6 个（其中 2 个可以是同主机的分屏），`host-B` 上 4 个。每个标签里 `echo tab-N` 并记住输出。
2. 在远端统计到该主机的 SSH 连接数：`ss -tn state established '( sport = :22 )' | tail -n +2 | wc -l`（记 `N_before`）。
3. 在同一台主机上开两个分屏，确认该主机的连接数只 +1：再统计一次（记 `N_split`），`N_split - N_before` 应为 1。
4. 完全退出 Sentinel（GUI 与 daemon 都退出：先用任务管理器结束 `daemon.exe`，再关 GUI，避免 supervisor 立即拉起）。
5. 重新启动 Sentinel。**在点击任何标签之前**统计两次连接数（间隔 5 秒）：应为 0（惰性恢复）。
6. 只点击一个标签，再统计：应恰好 +1 条，且不是 10 条。
7. 逐一点击其余标签，确认都能接回、`echo tab-N` 的历史还在；标签顺序与 A1 中的布局一致。
8. 十会话资源：全部连接后静置 5 分钟，在任务管理器记录 `daemon.exe` + 各 `Sentinel.exe` 进程的 CPU 与内存；同时用 `\\Get-Counter` 或资源监视器记录整机 CPU。
   把结果按 `docs/perf.md` 的「待真机」表格格式追加进去（10 个会话 CPU/内存），并注明采样时长与机器配置。
9. 升级路径：用新版本覆盖安装（保持 `%LOCALAPPDATA%\sentinel` 与 `client.json` 不变），重连同一主机，执行 `tmux -L sentinel ls`：会话名应不变、会话被接回而不是新建。

**预期结果**

- 重启后不点击标签时，到远端的 SSH 连接数为 0；
- 点击一个标签只建立 1 条连接；同主机多个分屏/标签复用同一条 TCP 连接；
- 10 个标签全部接回后布局与历史一致；
- 升级后靠 `client.json` 里的稳定标识接回旧会话，不产生新 `st_*` 名字。

**实测结果**：待验证（`____-__-__`，执行人 `____`）

```
重启后未点击时的连接数：____（应为 0）
点击 1 个标签后的连接数：____（应为 1）
同主机两分屏的连接增量：____（应为 1）
10 会话 daemon CPU：____%；内存：____ MiB；Sentinel 进程总数：____
升级后远端会话名是否变化：否 / 是
结论：
```

**证据**

- [ ] `a6-1-ten-tabs.png`：10 个标签
- [ ] `a6-2-zero-connections.png`：启动后未点击时 PowerShell 连接数为 0
- [ ] `a6-3-one-connection.png`：点击一个标签后为 1
- [ ] `a6-4-resource.png`：10 会话静置 5 分钟的任务管理器截图
- [ ] `a6-5-tmux-after-upgrade.png`：升级前后 `tmux ls` 会话名一致

## 必须人工完成的部分（Windows 真机）

| 环节 | 涉及场景 | 为什么自动化做不了 |
| --- | --- | --- |
| 全部 GUI 交互（标签、分屏、侧边栏、状态胶囊、命令面板、设置） | A1–A6 | xterm.js 渲染、WebGL、Electron 进程模型只存在于目标平台 |
| 任务管理器结束 GUI / 守护进程 | A2、A4、A6 | 需要真实进程与 supervisor 的父子关系 |
| 拔网线 / `netsh advfirewall` 断网 | A3 | 需要可控物理或防火墙层中断，且要观察界面状态变化 |
| 注销重登（登录自启、单实例） | A4 | 依赖 Windows 登录会话与 HKCU Run |
| Credential Manager 相关检查 | A2、A6 | Linux 侧是 `MemoryStore` 替身 |
| 10 会话 CPU/内存、冷启动内存、GUI 帧率 | A6 | 见 `docs/perf.md`「只能在 Windows 真机上采集」 |
| 安装 / 卸载 / 升级覆盖安装 | A6 | 任务 10.3、10.4 的范围，但 A6 步骤 9 需要它 |

## 本机（Linux）已做的自动化预检

这些命令不替代上面的手工验收，只是给出「非 GUI 部分」的基线证据。全部在 2026-11-03、WSL2 / 12 vCPU / 7 GiB 上执行。

| 命令 | 结果 | 说明 |
| --- | --- | --- |
| `eval "$(./scripts/test-sshd.sh start)" && cargo test --workspace --release` | **失败（阻塞）** | `crates/helper` 的测试目标编译失败：`crates/helper/src/frame.rs:106` 调用 `serde_json::Deserializer::byte_offset()`，当前 `serde_json` 版本没有该方法（E0599）。与本变更的验收逻辑无关，但会让「一条命令跑完全部测试」不可用，建议单独修 |
| `cargo test --workspace --release --exclude helper` | 见下 | 排除 helper 后的全量单测/集成测试 |
| `cargo test -p ssh --release --test live -- --test-threads=1` | 通过（8 项中的 `throughput` 已单独复跑） | 覆盖四种认证、主机密钥 TOFU、单 TCP 连接复用、keepalive |
| `cargo test -p daemon --test reconnect_live --release -- --test-threads=1` | 通过（2/2） | `reconnects_after_the_server_drops_and_replays_history`、`a_vanished_remote_session_is_recreated_and_reported` — 对应 A3 的「自动重连 + 回放」与「会话已不存在」分支 |
| `./scripts/perf.sh` | 通过 | 吞吐 / 回放 / 空闲 CPU 的可复现数据，见 `docs/perf.md` |

> 说明：上面这些通过项只证明「守护进程 + SSH + tmux + 缓冲」这条链路在 Linux 上行为正确；A1–A6 里的 GUI 部分必须在 Windows 上重跑，不能用它们替代。

## 附录 A：与 spec 场景的映射

| 本文件 | spec 场景 |
| --- | --- |
| A1 | `multi-host-workspace`：一个窗口内连接两台主机、左右分屏同时操作两台机器、重启后布局一致 |
| A2 | `terminal-session-persistence`：强制结束 GUI 后会话仍在；`terminal-emulation`：重开 GUI 后仍能向上翻历史 |
| A3 | `ssh-connection-management`：网络闪断后自动恢复、断线时输入半行命令；`terminal-emulation`：卡顿期间的输出不丢；`terminal-session-persistence`：断网时状态变化可见 |
| A4 | `terminal-session-persistence`：守护进程被杀后自动恢复、重复启动不产生第二个守护进程；`desktop-interface`/`ssh-connection-management` 无直接对应（自启在任务 9.1） |
| A5 | `terminal-session-persistence`：关闭标签后从后台列表接回、后台会话列表内容正确、提示清理但不自动删除 |
| A6 | `multi-host-workspace`：十个标签不触发十次握手、重启后布局一致；`terminal-session-persistence`：升级后仍能接回旧会话；`ssh-connection-management`：同一会话不被重复创建 |

未被上面六条覆盖的 spec 场景（认证错误分类、指纹变化、tmux 缺失降级、中文/emoji 渲染、搜索、粘贴、i18n、主题、安装包等）由任务 3.x / 4.x / 6.x / 7.x / 8.x 各自的验证项与对应单测覆盖，不在本文件的六条端到端路径里重复。

## 附录 B：一次验收的时间预算

| 场景 | 预计耗时 | 主要等待点 |
| --- | --- | --- |
| A1 | 10 分钟 | 两次连接 + 重启 GUI |
| A2 | 15 分钟 | 60 秒连续输出 + 重启 |
| A3 | 10 分钟 | 断网 30 秒 + 恢复 |
| A4 | 15 分钟 | 守护进程重启 + 注销重登 |
| A5 | 15 分钟 | 改库前后备份、接回 |
| A6 | 25 分钟 | 10 个标签 + 静置 5 分钟 + 覆盖安装 |
