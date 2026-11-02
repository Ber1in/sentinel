# Spec Delta

## Purpose

安全保存登录凭据与用户配置，并在重启后可靠恢复主机、标签、布局与会话元数据。

## ADDED Requirements

### Requirement: 凭据不落明文

密码与 passphrase MUST 存储在 Windows Credential Manager，MUST NOT 以明文写入配置文件、状态数据库、日志或崩溃转储。

#### Scenario: 配置与日志中无明文密码

- **WHEN** 用户保存一个带密码的主机后检查配置目录与日志文件
- **THEN** 文件内容中不出现该密码，凭据仅存在于 Credential Manager

### Requirement: 私钥按引用保存

用户选择私钥文件时系统 MUST 只保存路径；用户直接粘贴的私钥内容 MUST 存入凭据库而非配置文件。

#### Scenario: 私钥路径与内容分别存放

- **WHEN** 用户分别通过文件选择和粘贴方式添加私钥
- **THEN** 前者在配置中只有路径，后者在配置中无内容且凭据库中存在加密条目

### Requirement: 配置可手工编辑

系统 MUST 把用户配置保存为 `%APPDATA%` 下的 JSON 文件，至少覆盖默认 shell、字体、快捷键、滚动缓冲上限与 tmux 选项。手工修改后 MUST 在下次启动生效。

#### Scenario: 手工修改字体生效

- **WHEN** 用户在应用退出状态下修改配置文件中的字体与字号并重新启动
- **THEN** 新字体与字号在新会话中生效

### Requirement: 状态库原子写入

主机、标签、布局与会话索引 MUST 存入本地 SQLite，写入 MUST 具备原子性，进程异常退出 MUST NOT 留下半写状态。

#### Scenario: 写入过程中断不损坏

- **WHEN** 守护进程在更新布局时被强制终止
- **THEN** 重启后状态库可正常打开，布局为上一次完整写入的版本

### Requirement: 版本校验与损坏恢复

系统 MUST 在启动时校验状态库与配置的 schema 版本。发现不可解析的损坏时 MUST 报错并提供备份后重建的选项，MUST NOT 静默丢弃数据。

#### Scenario: 状态库损坏时给出可恢复路径

- **WHEN** 状态库文件被截断导致无法解析
- **THEN** 系统提示损坏、保留原文件备份，并在用户确认后重建空库，不直接覆盖原文件
