# Spec Delta

## Purpose

在 helper、tmux 与无持久化之间做出可解释的选择，并向用户如实展示当前会话由谁保持。

## ADDED Requirements

### Requirement: 固定优先级

provider 选择 MUST 按 helper → tmux → 无持久化 的顺序尝试，只有前一个不可用时才降级。

#### Scenario: helper 可用时优先

- **WHEN** 远端 helper 已安装且版本兼容
- **THEN** 新会话由 helper 承载，界面标注"helper"而不是"tmux"

### Requirement: 降级可解释

发生降级时 MUST 说明原因（未安装、架构不支持、校验失败、目录不可执行、版本不兼容），MUST NOT 静默切换。

#### Scenario: 降级原因可见

- **WHEN** helper 因架构不支持而降级到 tmux
- **THEN** 界面显示具体原因，而不是只显示"tmux"

### Requirement: 已有会话不被强制迁移

provider 选择只影响新建会话；已存在的会话 MUST 保持原 provider，直到用户显式重建。

#### Scenario: 升级后旧会话仍在 tmux

- **WHEN** helper 安装完成时存在由 tmux 承载的旧会话
- **THEN** 该会话继续使用 tmux，只有新会话才用 helper
