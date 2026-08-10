# POE Alarm

POE 制作时的本地 OCR 告警工具。它读取选定屏幕区域中的蓝色装备词缀，命中用户输入的完整目标词缀后立即停止扫描，并显示会阻挡后续鼠标点击的红色锁定窗、循环播放告警声。

当前版本：`0.3.0` 原型。目标环境为 Windows 10/11 x64、POE1 英文客户端。

## 直接试用

自包含单文件位于：

```text
artifacts/publish/win-x64/PoeAlarm.exe
```

约 71 MB，不要求另外安装 .NET。Windows 必须已安装任一英文 OCR 语言能力；缺少时程序会明确拒绝开始监控，不会静默改用中文 OCR。当前是未签名开发版本；正式分发前仍需代码签名与自动更新通道。

使用流程：

1. 从 PoEDB 复制完整目标词缀，粘贴到“目标整句”。
2. 回到游戏并让鼠标悬停在待制作装备上。
3. 按 `Ctrl + Shift + F11`，只框装备提示框中的词缀区域。区域越小，OCR 越快。
4. 点击“开始监控”，程序会最小化；游戏屏幕角落显示深灰色小浮窗，包含当前目标词缀和运行时间，不使用黄灯或绿灯。
5. 命中后灰色浮窗切换为同位置的红色确认卡片，扫描停止，卡片背后的透明全屏输入盾接收后续鼠标点击。真正停手 1.2 秒后确认按钮才会启用；检查装备后点击“我已检查，解除鼠标锁定”，也可按 `Ctrl + Shift + F12`。

“用截图测试”会用相同的预处理、OCR 和整句匹配管线分析存档截图，适合在进游戏前验证模板。

## 匹配规则

程序不猜关键词，也不需要内置全量词缀库。用户粘贴的完整 PoEDB 词缀就是本次监控的一条临时记录。

例如：

```text
(6—8)% increased Attack Speed if you've dealt a Critical Strike Recently
```

会归一化为：

```text
<PCT> increased attack speed if you've dealt a critical strike recently
```

`#`、实际数值、数值区间以及高级描述中的 `8(6-8)%` 会映射到同一种数值槽；所有非数字单词及其顺序仍必须完整一致。因此 Attack/Cast、Dagger/Claw、Cold/Fire、dealt/killed 等不会互相命中。OCR 只对 `I/l/1`、`O/0`、`rn/m` 等限定字形混淆做小范围容错，并能恢复 OCR 丢失的英文单词边界；这不会退化成关键词匹配。

逻辑词缀可以由 1–4 条相邻 OCR 物理行组成，所以提示框宽度和前置词缀换行不会改变匹配语义。

## 已验证结果

- 整句归一化与反例测试：`26/26` 通过。
- 用户提供的真实 POE1 截图：目标词缀被完整识别并命中。
- 半透明提示框后的仓库图标和堆叠数量会在送入 OCR 前由蓝色字形掩膜剔除；已验证背景数字压住词缀的截图仍能完整识别。
- 同图将模板改成 Cast Speed：正确拒绝。
- 856 × 380 词缀区域：预处理约 6 ms，五条候选行 OCR 约 25 ms。
- 画面未变化时：复用 OCR 结果，总处理约 5 ms，降低空闲 CPU 占用。
- 没有识别到蓝色词缀时会继续等待，不再自动停止；监控只由用户手动停止或在命中后锁停。
- 监控期间的深灰浮窗点击穿透、不抢焦点，每秒更新一次运行时间；会优先避开所选 OCR 区域，并请求 Windows 将自身排除在截屏内容之外。
- 红色告警包含全屏粗边框、角落确认卡、静默 1.2 秒后启用的手动确认按钮和循环内存 WAV；继续连点会重新计时，确认后的 300 ms 也继续吸收双击尾部。告警窗会取得前台并接收整个桌面的鼠标点击。
- Windows x64 自包含单文件发布成功，并通过无交互启动冒烟测试。

这些数字来自当前机器与提供的截图，不是对所有分辨率、缩放和显卡的保证。正式使用前应在实际游戏画面中做一轮持续制作测试。

## 安全边界

程序只读取屏幕像素，不合成、代替、替换或延迟游戏输入。命中后会显示独立的前台窗口，由该窗口接收后续鼠标点击；它不使用全局低级鼠标钩子，也不会向游戏发送输入。目标画面出现到 OCR 和阻挡窗完成之间已经送达的点击无法追回，因此仍不能承诺在任何点击速度下绝对阻止点过头。

当前没有联网、账号、PoEDB 抓取或遥测。设置保存在：

```text
%LOCALAPPDATA%/PoeAlarm/settings.json
```

## 工程结构

- `src/PoeAlarm.Core`：词缀归一化、整句匹配。
- `src/PoeAlarm.App`：WPF 界面、选区截屏、Windows OCR、监控循环、红色告警。
- `tests/PoeAlarm.Core.Tests`：无外部测试框架的 matcher 回归测试。
- `tools/PoeAlarm.OcrProbe`：Windows OCR ROI/缩放基准工具。
- `tools/PoeAlarm.EndToEndProbe`：真实截图的产品管线端到端探针。
- `tools/PoeAlarm.UiSnapshot`：主界面、监控浮窗与红色告警的离线视觉快照及窗口行为断言。

OCR、截屏和告警均位于独立接口后。首版的小区域 GDI/DIB 截屏已经避免每帧重建原生资源；如果实机基准证明截屏成为瓶颈，可直接替换为 DXGI Desktop Duplication，而无需改动匹配与界面。

## 开发与验证

仓库由 `global.json` 固定 .NET SDK `10.0.302`。本工作区的 SDK 位于 `.tools/dotnet`。

```powershell
.\.tools\dotnet\dotnet.exe restore PoeAlarm.slnx -p:NuGetAudit=false
.\.tools\dotnet\dotnet.exe build PoeAlarm.slnx --no-restore -m:1
.\.tools\dotnet\dotnet.exe run --project tests\PoeAlarm.Core.Tests\PoeAlarm.Core.Tests.csproj --no-build
.\.tools\dotnet\dotnet.exe publish src\PoeAlarm.App\PoeAlarm.App.csproj -p:PublishProfile=PortableWinX64 --no-restore -m:1
```

`-m:1` 是当前受管执行环境的规避项：这里的并行 MSBuild 节点通信会留下孤儿进程；项目与 `.slnx` 本身没有并行配置错误。

## 下一阶段

1. 在 POE1 内做长时间制作与不同 UI 缩放的漏报/误报采样。
2. 根据实测调整蓝色行定位阈值，并加入可保存的识别配置档。
3. 若 GDI 在特定全屏模式下不可用，再加入 DXGI 捕获实现。
4. 增加签名安装包、更新清单和可回滚版本。
5. POE2 作为独立颜色/字体配置档接入；整句模板和 matcher 可以直接复用。
