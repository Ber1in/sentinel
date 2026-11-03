# Spec Delta

## Purpose

在远端以用户态常驻进程管理 PTY 与会话，使断开客户端后进程继续运行，并在重连时按客户端标识隔离接入。

## ADDED Requirements

### Requirement: 常驻与自动启动

helper MUST 在首次使用时自行脱离终端常驻；客户端断开后 MUST 继续持有会话。系统重启后 helper MUST 在下次连接时被重新拉起，并按需恢复会话。

#### Scenario: 客户端退出后进程仍存活

- **WHEN** 用户关闭 Windows 客户端
- **THEN** 远端 helper 与其中的 shell 继续运行，`ps` 可见其常驻进程

### Requirement: 只通过 SSH 通道通信

helper MUST 只监听 Unix socket，MUST NOT 监听任何 TCP 端口；客户端通过 SSH exec 通道运行 `sentinel-helper proxy` 完成桥接。

#### Scenario: 不暴露网络端口

- **WHEN** helper 运行后检查服务器监听端口
- **THEN** 除 sshd 外没有新增监听端口

### Requirement: socket 权限

socket 文件 MUST 仅当前用户可读写（0600），位于不可被其他用户遍历的目录内。

#### Scenario: 其他用户无法访问

- **WHEN** 另一个用户尝试连接该 socket
- **THEN** 因文件权限被拒绝

### Requirement: 按客户端命名空间隔离

helper MUST 按客户端标识隔离会话集合，不同客户端的会话列表 MUST 互不可见，MUST NOT 允许跨命名空间接入。

#### Scenario: 两台客户端互不可见

- **WHEN** 两个不同客户端标识连接同一 helper
- **THEN** 各自只看到并接回自己的会话

### Requirement: 会话生命周期

helper MUST 支持创建、接入、脱离、销毁会话与调整 PTY 尺寸，并在销毁时释放对应缓冲。

#### Scenario: 脱离后重新接入

- **WHEN** 客户端 detach 后再次 attach 同一会话
- **THEN** 会话保持原 shell 状态并回放最近的输出

### Requirement: 空闲回收

当某命名空间下没有任何会话且无客户端连接超过配置时长后，helper MUST 自行退出，避免长期占用内存。

#### Scenario: 空闲后退出

- **WHEN** 所有会话销毁且超过空闲阈值
- **THEN** helper 进程退出，下次连接时按需重新拉起
