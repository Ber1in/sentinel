# Spec Delta

## Purpose

提供可观察、可取消的文件上传与下载，让用户在终端旁完成取证、改配置、传文件，而不必切换到独立 FTP 工具。

## ADDED Requirements

### Requirement: 上传入口

系统 MUST 支持右键上传文件、右键上传目录与从 Windows 资源管理器拖拽文件到面板三条上传路径，目标目录 MUST 为面板当前目录。

#### Scenario: 拖拽上传到当前目录

- **WHEN** 用户把 `nginx.conf` 从资源管理器拖到面板的 `/etc/nginx` 视图
- **THEN** 文件被上传到 `/etc/nginx/nginx.conf`，队列出现该任务

### Requirement: 下载入口

系统 MUST 支持右键下载到本地，MUST 弹出 Windows 目录选择对话框；选择单个文件时 MUST 允许指定文件名。

#### Scenario: 下载文件并保留原名

- **WHEN** 用户对 `/var/log/syslog` 选择"下载到…"并确认目录 `D:\tmp`
- **THEN** 文件被保存为 `D:\tmp\syslog`，队列显示完成

### Requirement: 传输队列可见可控

队列 MUST 显示每个任务的进度、速度、已传/总量，MUST 支持取消，并 MUST 限制并发数。

#### Scenario: 取消进行中的传输

- **WHEN** 用户取消一个正在传输的大文件
- **THEN** 传输停止，队列该项标注为已取消，远端不留下不完整的同名正式文件

### Requirement: 失败可读且可重试

传输失败时 MUST 显示可读原因（权限、磁盘空间、连接中断）与失败文件，MUST 提供重试入口。

#### Scenario: 连接中断后重试成功

- **WHEN** 上传过程中网络中断导致失败，网络恢复后用户点击重试
- **THEN** 该任务重新开始并最终完成，队列状态从失败变为完成

### Requirement: 覆盖前确认

目标位置已存在同名文件时，系统 MUST 在覆盖前请求确认，并 MUST 支持"全部覆盖"与"跳过"。

#### Scenario: 同名文件需确认

- **WHEN** 用户上传的文件在远端已存在
- **THEN** 出现覆盖确认，选择跳过后远端文件保持不变

### Requirement: 不保留传输历史

传输队列 MUST 只在本次运行期间存在，应用重启后 MUST 清空，MUST NOT 写入历史记录。

#### Scenario: 重启后队列为空

- **WHEN** 用户完成若干传输后重启应用
- **THEN** 队列为空，界面上不出现任何历史传输记录
