# Spec Delta

## Purpose

在终端窗口内浏览远端文件系统，让文件访问成为当前会话的附属视图，而不是另开一条连接。

## ADDED Requirements

### Requirement: 面板绑定当前标签的主机

文件面板 MUST 绑定当前活动标签的主机，标签切换时 MUST 切到该主机的目录视图。同一主机的多个标签 MUST 共享同一条 SSH 连接，但各自保持独立的当前目录。

#### Scenario: 切换标签不串台

- **WHEN** 标签 1 是主机 A 的 `/etc`，标签 2 是主机 B 的 `/var/log`，用户来回切换
- **THEN** 面板分别显示 `/etc` 与 `/var/log`，且不产生新的 SSH 连接

### Requirement: 默认跟随终端工作目录

面板 MUST 默认跟随当前会话终端的工作目录变化；用户 MUST 能切换为锁定模式，锁定后目录 MUST 不随终端变化。

#### Scenario: cd 后面板自动跳转

- **WHEN** 用户在终端执行 `cd /var/log`
- **THEN** 面板自动切换到 `/var/log`；用户锁定后再 `cd /etc`，面板仍停留在 `/var/log`

### Requirement: 目录浏览与导航

面板 MUST 显示名称、大小、修改时间与权限，MUST 支持双击进入目录、面包屑跳转、返回上级、刷新与隐藏文件开关。

#### Scenario: 通过面包屑回退

- **WHEN** 用户位于 `/etc/nginx/conf.d`
- **THEN** 点击面包屑中的 `nginx` 后进入 `/etc/nginx`，且列表刷新为对应内容

### Requirement: 目录访问失败可恢复

目录不存在或权限不足时，面板 MUST 显示可读错误与失败路径，MUST NOT 停留在空白状态，并 MUST 提供返回上一级或刷新入口。

#### Scenario: 无权限目录不空白

- **WHEN** 用户进入一个当前用户无读权限的目录
- **THEN** 面板显示权限错误与该目录路径，用户可点击返回上一级继续浏览

### Requirement: 面板可折叠且状态被记住

面板 MUST 支持折叠与展开、宽度调整，折叠状态与宽度 MUST 在重启后恢复。

#### Scenario: 折叠状态持久化

- **WHEN** 用户折叠面板并调整宽度后重启应用
- **THEN** 面板保持折叠状态，展开后宽度与关闭前一致
