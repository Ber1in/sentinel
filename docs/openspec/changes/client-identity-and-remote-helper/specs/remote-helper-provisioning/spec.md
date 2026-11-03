# Spec Delta

## Purpose

让客户端在首次连接时把远端 helper 自动装好，用户不需要登录服务器做任何手工操作。

## ADDED Requirements

### Requirement: 首次连接自动部署

远端不存在可用 helper 时，系统 MUST 在上传任何内容前向用户展示一次确认（主机、目标路径、文件大小、sha256），确认后自动完成上传、校验、安装与启动。同一主机确认后 MUST 不再重复询问。

#### Scenario: 首次连接自动安装

- **WHEN** 用户第一次连接一台没有 helper 的主机并确认安装
- **THEN** 无需再次操作，连接建立后会话由 helper 接管，界面标注"helper"

#### Scenario: 拒绝安装时回退

- **WHEN** 用户在确认框中选择取消
- **THEN** 系统不写入任何文件，并按 provider 优先级回退到 tmux 或无持久化

### Requirement: 部署前的能力探测

部署前 MUST 检测远端架构、libc 类型与版本、目标目录可写性与挂载是否允许执行；不满足任一条件时 MUST 说明具体原因并回退。

#### Scenario: noexec 挂载被识别

- **WHEN** 目标目录位于 `noexec` 挂载点
- **THEN** 系统报告"目录不可执行"并改用备用目录或回退，而不是反复失败

### Requirement: 完整性校验

上传完成后 MUST 校验文件 sha256 与客户端内置值一致，不一致 MUST 删除文件并中止安装。

#### Scenario: 传输损坏被拒绝

- **WHEN** 上传后的文件哈希与期望值不符
- **THEN** 文件被删除，界面提示校验失败，会话回退到 tmux

### Requirement: 不依赖 root

安装 MUST 只写用户可写目录（默认 `~/.local/share/sentinel`），MUST NOT 请求 sudo 或写入系统路径。

#### Scenario: 普通用户可安装

- **WHEN** 用户以普通账号登录（无 sudo 权限）
- **THEN** 安装成功且不产生任何权限错误

### Requirement: 版本升级

客户端版本高于远端 helper 时 MUST 触发升级；无活跃会话时立即替换，有活跃会话时 MUST 延迟到全部会话结束并提示用户。

#### Scenario: 有会话时延迟升级

- **WHEN** 远端 helper 版本较旧且存在活跃会话
- **THEN** 升级被推迟，界面提示"空闲时升级"，现有会话不受影响
