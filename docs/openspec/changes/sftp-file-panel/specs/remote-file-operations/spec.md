# Spec Delta

## Purpose

在面板内完成常见的远端文件管理操作，并保证删除、改权限这类不可逆或高影响操作有明确确认与可读结果。

## ADDED Requirements

### Requirement: 基础文件操作

面板 MUST 支持新建目录、重命名、删除与修改权限，操作 MUST 作用于当前选中的条目。

#### Scenario: 新建目录后立即可见

- **WHEN** 用户在 `/etc/nginx` 新建目录 `sites`
- **THEN** 列表中出现 `sites` 且可直接双击进入

### Requirement: 删除必须确认

删除操作 MUST 在执行前请求确认；删除非空目录时 MUST 明确提示将递归删除的内容。

#### Scenario: 删除非空目录有提示

- **WHEN** 用户删除一个包含文件的目录
- **THEN** 确认框说明将删除目录及其内容，取消后远端不变

### Requirement: 权限修改需校验

修改权限 MUST 接受八进制输入并校验合法性，非法输入 MUST 被拒绝且不发送到远端。

#### Scenario: 非法权限被拒绝

- **WHEN** 用户输入 `999` 作为权限
- **THEN** 系统提示非法权限，远端文件权限保持不变

### Requirement: 操作结果可观察

操作完成后面板 MUST 刷新受影响条目并显示结果；失败时 MUST 显示可读原因，MUST NOT 静默重试。

#### Scenario: 重命名失败提示原因

- **WHEN** 用户把文件重命名为已存在的名字
- **THEN** 面板显示目标已存在的错误，文件保持原名
