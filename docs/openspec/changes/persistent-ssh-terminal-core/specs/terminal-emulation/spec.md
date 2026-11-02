# Spec Delta

## Purpose

正确渲染远端终端输出并处理用户输入，并在 GUI 重开或重连后保留可回溯的会话历史。

## ADDED Requirements

### Requirement: 终端渲染能力

系统 MUST 正确渲染真彩色与 256 色、光标形状、备用屏幕切换、鼠标上报与 bracketed paste，MUST 正确处理中文等宽字符与输入法组合输入。

#### Scenario: 中文输入法正常上屏

- **WHEN** 用户在会话中使用中文输入法输入并上屏多字词
- **THEN** 字符顺序与宽度正确，不出现重复字符、半角错位或光标位置错误

### Requirement: 尺寸同步

窗口或分屏尺寸变化时，系统 MUST 把新的行列数同步到远端 PTY，使全屏程序按新尺寸重绘。

#### Scenario: 调整窗口后 vim 重绘正确

- **WHEN** 用户在 `vim` 中拖动窗口边缘改变尺寸
- **THEN** `vim` 按新尺寸重绘，底部状态行位置正确，无残影

### Requirement: 滚动缓冲持久化

每个会话 MUST 在本地保留滚动缓冲，默认上限 100,000 行且磁盘占用不超过 50MB，超出后按环形覆盖。GUI 重开后 MUST 恢复可滚动内容。

#### Scenario: 重开 GUI 后仍能向上翻历史

- **WHEN** 会话输出 20,000 行后用户关闭并重新打开 GUI
- **THEN** 该会话可向上滚动查看此前的输出，行内容与关闭前一致

### Requirement: 会话内搜索

系统 MUST 支持在当前会话的滚动缓冲内搜索并高亮匹配项，MUST 支持在匹配项之间跳转。

#### Scenario: 搜索历史输出

- **WHEN** 用户在某会话中搜索 `error`
- **THEN** 所有匹配被高亮，用户可在匹配之间前后跳转

### Requirement: 复制粘贴

系统 MUST 提供复制与粘贴快捷键；多行粘贴 MUST 通过 bracketed paste 发送，避免被远端误判为逐条执行。

#### Scenario: 多行粘贴不逐行执行

- **WHEN** 用户把三行命令一次性粘贴到 bash 提示符
- **THEN** 三行作为整体粘贴，不产生三段独立的执行回显

### Requirement: shell 集成上报工作目录

系统 MUST 通过 shell 集成识别当前工作目录与命令边界，用于会话恢复与后续文件面板定位；集成 MUST 在无法注入时不影响会话可用性。

#### Scenario: 识别到目录变化

- **WHEN** 用户在会话中执行 `cd /etc/nginx`
- **THEN** 系统记录当前目录为 `/etc/nginx`，并在会话恢复时使用该目录

### Requirement: 输出缓冲不丢字节

GUI 短暂卡顿、重连或重开期间，守护进程 MUST 缓冲终端输出，GUI 恢复后 MUST 按原顺序完整回放。

#### Scenario: 卡顿期间的输出不丢

- **WHEN** GUI 因渲染压力卡顿 5 秒，期间远端持续输出日志
- **THEN** 卡顿结束后所有输出按原顺序出现，无缺失与重复
