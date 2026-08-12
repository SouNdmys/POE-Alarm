# POE Alarm 0.6.1

## 更轻、更顺手

- 主界面收紧为更聚焦的单列布局，日间玻璃风格、字体和信息层级重新整理；常用操作无需在大窗口中来回寻找。
- “使用说明”改为独立窗口，包含三步使用流程、繁體中文 OCR 安装命令、常见提醒、作者与联系方式。
- 加入可配置的全局“开始监控”热键，默认 `Ctrl + Shift + F10`；命中确认后无需切回程序，可直接在游戏内启动下一轮。
- POE1 / POE2、目标词缀、选区和 OCR 语言仍按游戏分别保存。

## 告警修复

- 修复部分 Windows 音量/混音器配置下内置音效与自定义 WAV 均无声的问题；告警音现在进入 POE Alarm 自己的多媒体音频会话。
- 保留自定义 PCM WAV 校验与内置原创音效回退，提示音不会被复制、上传或随公开包分发。
- 红色命中窗继续居中显示，确认按钮立即可用；确认后仍吸收约 300 ms 的双击尾击。

## 繁體中文建议

建议使用管理员 PowerShell 安装 Windows `zh-TW` OCR：

```powershell
Add-WindowsCapability -Online -Name "Language.OCR~~~zh-TW~0.0.1.0"
Get-WindowsCapability -Online -Name "Language.OCR~~~zh-TW~0.0.1.0"
```

第二条命令显示 `State : Installed` 即成功。未安装时程序仍可使用内置离线兼容引擎，但低延迟与覆盖能力不如 Windows OCR 快速路径。

## 作者

- 作者：SouNd
- 联系：soundmys1994@gmail.com
- 项目：https://github.com/SouNdmys/POE-Alarm

本版没有修改 POE1 English 的识别与匹配主路径。
