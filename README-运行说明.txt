POE Alarm 1.0.0

1. 直接运行 PoeAlarm.exe；不需要另外安装 .NET。
2. 选择 POE1 / POE2 与游戏语言。
3. 单词缀模式：从 PoEDB 粘贴一条完整词缀，数值大小会自动忽略。
4. 多词缀组合：设置一种或多种可接受结果，可选择数量和数值条件。
5. 框选装备提示框区域，点击“开始监控”或按 Ctrl + Shift + F10。
6. 命中后检查装备并确认；下一件装备需要再次启动监控。

1.0.0 不包含谨慎模式或 Mirror Tier 自动保护模式。
程序不会替玩家点击、补发、重放或排队输入。

繁體中文建议（管理员 PowerShell）：
Add-WindowsCapability -Online -Name "Language.OCR~~~zh-TW~0.0.1.0"
Get-WindowsCapability -Online -Name "Language.OCR~~~zh-TW~0.0.1.0"
看到 State : Installed 即成功，之后重启 POE Alarm。

作者：SouNd
联系：soundmys1994@gmail.com
项目：https://github.com/SouNdmys/POE-Alarm

本目录的 THIRD-PARTY-NOTICES.md 与 licenses 文件夹属于发布包的一部分，
二次分发时请一并保留。
