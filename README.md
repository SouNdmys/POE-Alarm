# POE Alarm

POE 制作时的本地 OCR 告警工具。它读取选定屏幕区域中的蓝色装备词缀，命中用户输入的完整目标词缀后立即停止扫描，并显示会阻挡后续鼠标点击的红色锁定窗、循环播放告警声。

当前正式基线：`0.6.1`；当前工作树开发版本：`0.7.0-preview.2`。目标环境为 Windows 10/11 x64，支持 POE1 / POE2 的 English 与繁體中文客户端。两款游戏会分别保存目标词缀、框选区域和识别语言；POE1 English 保持原有 Windows OCR 路径，POE2 English 使用独立复核层，繁中使用 Windows 中文多行 OCR 快速路径并仅在局部疑难候选或系统没有中文 OCR 能力时使用内置 PP-OCRv5。实测数据与边界见[繁體中文 OCR 生产说明](docs/traditional-chinese-ocr.md)。

## 直接试用

开发者在本地执行本文的 `dotnet publish` 后，当前开发预览的自包含构建产物位于：

```text
artifacts/publish/0.7.0-preview.2/win-x64/PoeAlarm.exe
```

如果拿到的是发布 ZIP，解压后直接运行根目录的 `PoeAlarm.exe`。公开包内置的是程序原创提示音，不含游戏音频素材；想使用自己已有的音效，可在“体验设置 → 命中提示音”中选择本地 PCM WAV，路径只保存在本机，不会复制到程序目录或上传。

发布 ZIP 还会附带 `THIRD-PARTY-NOTICES.md` 与 `licenses/`；它们是内置离线 OCR 运行时和模型的许可文件，请勿从二次分发包中删除。

不要求另外安装 .NET。English 模式需要 Windows 英文 OCR；繁中快速模式建议安装 Windows `zh-TW` OCR，程序其次会尝试 `zh-Hant`、`zh-Hans` 或 `zh-CN`。如果系统没有中文 OCR，仍会自动使用 EXE 内置的离线兼容引擎，但速度与覆盖保证会低于快速路径。两种模式都不需要 Python、Paddle 框架或联网。当前是未签名开发版本；正式分发前仍需代码签名与自动更新通道。

用管理员身份打开 PowerShell，运行：

```powershell
Add-WindowsCapability -Online -Name "Language.OCR~~~zh-TW~0.0.1.0"
```

安装后验证：

```powershell
Get-WindowsCapability -Online -Name "Language.OCR~~~zh-TW~0.0.1.0"
```

看到 `State : Installed` 即成功。安装完成后重新启动 POE Alarm；该能力会让繁中优先走更快、更准的 Windows OCR 路径，也已写进程序内的“使用说明”。

使用流程：

1. 选择 POE1 或 POE2，再选择与游戏客户端一致的识别语言。
2. 从 PoEDB 复制完整目标词缀，粘贴到“目标整句”。
3. 回到游戏并让鼠标悬停在待制作装备上。
4. 按 `Ctrl + Shift + F11`，只框装备提示框中的词缀区域。区域越小，OCR 越快。
5. 点击“开始监控”，或在游戏内按可配置的全局启动热键（默认 `Ctrl + Shift + F10`）。程序会最小化；状态浮窗变为淡绿色，并显示当前目标词缀和运行时间。命中、确认或停止后，浮窗会切回冷灰色“未监控”，提醒下一轮需要重新启动。
6. 繁中以及 POE2 模式发现蓝色词缀画面变化后，会让原生按键闸门先放行当前这一下鼠标的完整抬起，再从下一次按下开始短暂阻挡；OCR 未命中便立即放行，期间多点的左键会被丢弃而不会排队或补发。
7. 命中后淡绿色浮窗消失，同一个输入盾会原地升级，并在当前显示器中央弹出红色确认卡片；扫描停止。确认按钮立即可用；检查装备后点击“我已检查，解除鼠标锁定”，也可按 `Ctrl + Shift + F12`。确认后的约 300 ms 仍会吸收双击尾击。

界面支持简体中文与 English，且界面语言和游戏 OCR 语言互不影响。“使用说明”提供内置三步引导、繁中 OCR 安装方法、作者与联系方式。体验设置中可以修改全局启动热键、选择常驻状态浮窗、拖动保存位置、选择本地 PCM WAV 命中音，并决定程序浮层是否出现在录屏中；默认允许录制浮窗和红色命中窗。内置音效是程序合成的原创提示，不包含游戏音频资产。

“用截图测试”会用相同的预处理、OCR 和整句匹配管线分析存档截图，适合在进游戏前验证模板。

## vNext 开发预览

当前开发版本标识为 `0.7.0-preview.2`，以 `0.6.1` 为冻结基线，已加入与快速模式隔离的“多词缀组合”：可保存多种值得停手的结果，命中任意一种就会报警。每种结果可要求命中任意一条、全部或指定条数，也能为数值设置范围、至少、至多或精确值。同一条实际词缀在一种结果内最多计数一次。催化剂、品质与特殊效果可能改变装备提示框中的数值；程序只比较屏幕上看到的值，不计算装备的原始数值。

快速模式仍直接调用 0.6.1 的单目标识别接口，旧 `settings.json` 会迁移为快速模式且无需手工操作。结构化规则按 POE1 / POE2 分别保存；真实制作回放仍属于正式发布门禁。POE1 English 使用单次严格批量 OCR，POE2 English 保留变化帧的双 Windows 引擎确认；繁中 Windows OCR 与内置 Paddle 路径均已实现共享多目标局部复核。每个扫描批次的恢复工作单元有固定上限，目标数量不会导致重复整帧 OCR；普通与辅助转录共享物理行身份，不能把同一词缀重复计数。快速模式原有路径、阈值与鼠标状态机不变。Mirror Tier 安全模式作为独立 `MonitoringPolicy` 接入：当前仅在具备 changed-frame 指纹链路的 POE2 English 与繁中识别器上开放；命中时红停，目标相关但数值缺失/冲突或局部复核未决时黄停，稳定帧严格判定为安全不命中后，输入闸门只放行下一次完整制作点击，并在该次抬起时原子拦截后续点击。POE1 English 仍保持 0.6.1 无预判闸门的合同，不用未验证的单次 OCR 冒充 Mirror 安全。完整范围、延后项与验收标准见 [vNext 规则引擎与安全监控计划](docs/VNEXT_RULE_ENGINE_PLAN.md)。

PoEDB / PoE2DB 与 GGG 官网 Mirror Service 组合语料当前覆盖 14 个来源、7 个场景（其中 6 个真实镜装组合），共 36/36：15 个正例、21 个负例，包含 OR 第二分支、普通非 Alt tooltip、少一条、近邻词缀、数值边界与 Hybrid 同一 modifier 不重复计数。详情和来源链接见 [Mirror 复合规则语料](docs/MIRROR_RULE_CORPUS.md)。这证明规则表达与识别结果物理身份契约可覆盖这些制作目标，不等同于每个装备都做过实机 OCR 回放。

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

POE1 与 POE2 使用同一条数值原则：`#`、实际数值、固定数字、数值区间以及高级描述中的 `8(6-8)%` 都映射为带类型的数值占位符。程序只追踪词缀语义，不区分 2/3、词缀等级或 roll 高低；如果目标只在数字大小上不同，它们会被视为同一条词缀。百分比/普通数以及正/负仍作为结构保留，避免 OCR 丢失 `%` 或负号时误报。除此以外，所有文字及顺序仍必须完整一致，因此 Attack/Cast、Dagger/Claw、Cold/Fire、dealt/killed 等不会互相命中。英文 OCR 只对 `I/l/1`、`O/0`、`rn/m` 等限定字形混淆做小范围容错；繁中首先要求 Windows OCR 的整句严格命中，疑难行也必须经过原色局部重识别或 CTC 位置证据复核。这些都不会退化成关键词匹配。

POE1 逻辑词缀可以由 1–4 条相邻 OCR 物理行组成；POE2 支持最多 8 条，以覆盖较长的碑牌组合词缀。无需选择额外的装备类别模式。

## 已验证结果

- 整句归一化与反例测试通过，覆盖繁中逐字、统一数值占位、八行换行及近义反例。
- POE2 真实截图：English `38/38`、繁中 `38/38`；两套各 160 个跨截图目标负例误报 0。English 判定 p50 `4.0 ms`、p95 `75.3 ms`；繁中判定 p50 `34.5 ms`、p95 `59.6 ms`。
- POE2 PoEDB 中英语料共 167 组，其中碑牌 48 组：334/334 正例、562/562 缺行负例、334/334 数值模板、298/298 范围实值和 2/2 八行换行通过。
- POE2 繁中 30/40/50 ms 高速回放共 750 次：及时阻拦 750/750，点过头、截到却漏报、提前误报均为 0；捕获到判定 p95 `59.1 ms`。
- 2026-08-11 原生 POE 截图：5 张非 Alt 紧凑界面和 1 张 Alt 高级界面，在用户实际 `1131 × 928` 大选区内共 `27/27` 命中；93 个跨截图目标负例误报 0。
- 旧的微信/Alt 截图集：7 张图、65 条目标在新版 Windows-first 管线中仍为 `65/65`。
- 本机多次 Release 回归中，原生大选区的 changed-frame 预检 p50 `3.0–3.1 ms`、p95 `3.3–4.8 ms`；目标判定 p50 `27.0–29.2 ms`、p95 `53.0–55.9 ms`。27 个目标中 23 个直接命中，4 个使用局部复核；首次懒加载内置兼容模型的单次最坏判定为 `181–229.2 ms`，仍处于已武装鼠标闸门的保护期内。
- PoEDB 抽样语料覆盖珠宝 36、深渊珠宝 42、武器 39、护甲 46，共 163 条。相似蓝字、多行与干扰词缀的合成目标语义测试为 `163/163`（130 个主结果、33 个目标辅助结果、缓存 0）；153 个纯干扰、58 个缺失复合行、29 个远距行断层和 472 个有向近邻 OCR 负例误报 0。该测试不要求数值、正号或样式标点逐字相同，也不能替代真实游戏截图。
- 163 条 PoEDB 模板做 13,203 组两两严格匹配：只有 11 组跨装备域的原文完全重复会命中，153 个不同模板之间误命中 0；完整 target-aware OCR 另对 472 个有向人工近邻做反向测试，误报 0。
- POE1 English 路径独立回归：目标行与完整装备区各 `20/20`，warm p50 分别为 `3.2 ms` 与 `29.0 ms`；5 条真实词缀正例和 5 条语义近邻负例全部正确，与 0.4.0 基线同档。
- Windows 中文 OCR 直接处理整张蓝字掩膜并自行做多行布局，不再依赖宽选区的全宽投影分行。半透明提示框后的仓库图标与堆叠数字因此不会再把数条紧凑词缀合并后塞给单行模型。
- 画面未变化时复用 OCR 结果，降低空闲 CPU 占用。
- 没有识别到蓝色词缀时会继续等待，不再自动停止；监控只由用户手动停止或在命中后锁停。
- 状态浮窗点击穿透、不抢焦点；默认常驻，冷灰表示未监控、淡绿色表示监控中。它会优先贴近并避开 OCR 区域，也可在停止时拖动保存位置。默认允许录屏捕获浮窗；绿/灰配色不会进入 POE 蓝字掩膜。
- 红色告警包含全屏粗边框、显示器中央确认卡、立即可用的手动确认按钮和循环 WAV；确认后的 300 ms 继续吸收双击尾部。输入盾会在开始监控时预热并跨轮复用；隐藏态同时在 WPF 与 Win32 层释放鼠标。
- 繁中 changed-frame 保护由短时 `WH_MOUSE_LL` 按键状态机完成。状态机在监控开始时以完全放行状态预装，直接跟踪按下/抬起配对；武装不依赖 WPF 定时器。OCR 未决期间不显示全屏 WPF 窗口，因此不会打断游戏的装备 hover。它不会吞掉已经交给游戏的当前 `MouseUp`，而一旦吞掉下一次 `MouseDown`，也会成对吞掉对应 `MouseUp`。
- Windows x64 自包含单文件内置兼容模型发布，并通过资源哈希、字典边界、模型形状和真实 ONNX 推理自检；Windows 中文主路径另由真实截图 manifest 验证。

这些数字来自当前机器与提供的截图，不是对所有分辨率、缩放和显卡的保证。正式使用前应在实际游戏画面中做一轮持续制作测试。

## 安全边界

程序只读取屏幕像素，绝不向游戏合成、补发、重放或排队输入。POE1 English 只在命中后显示独立的前台阻挡窗，不安装预判闸门。繁中和 POE2 为了不让短暂目标帧在复核完成前被下一次点击越过，会在监控期间短时安装系统低级鼠标钩子：平时完全放行，只在词缀画面未决时返回“已处理”以丢弃后续按钮/滚轮输入。未决阶段保持红色窗口原生隐藏，真实命中后才显示并接管阻挡。钩子运行在独立消息线程，回调不做 OCR、WPF、日志或等待，停止监控后自动卸载。

按键闸门不会在当前鼠标仍按下时突然接管，而是先等待该次按键释放；未命中、手动停止、异常、关闭、保护看门狗都会优先恢复放行。若在未决阶段按 `Ctrl + Shift + F12`，程序会同时解除保护并停止本次监控，避免下一片 OCR 立即重新武装。钩子无法安装或运行中无法重新武装时，程序会拒绝开始或停止本次监控，不会静默进入无保护状态；命中后若 UI 线程迟迟无法显示红窗，系统级闸门也会独立超时放行。为了不破坏装备 hover，不使用全屏透明窗口兜底；若钩子被系统意外移除，仍无法宣称数学上的绝对保证。

当前没有联网、账号、PoEDB 抓取或遥测。设置保存在：

```text
%LOCALAPPDATA%/PoeAlarm/settings.json
```

## 工程结构

- `src/PoeAlarm.Core`：词缀归一化、整句匹配。
- `src/PoeAlarm.App`：WPF 界面、选区截屏、独立的 English/繁中 Windows OCR、繁中局部 PP-OCRv5/ONNX 复核与兼容路径、监控循环、红色告警。
- `tests/PoeAlarm.Core.Tests`：无外部测试框架的 matcher 回归测试。
- `tests/PoeAlarm.Rules.Tests`：vNext 数值槽、可接受结果、一对一计数、设置序列化与规则热路径回归测试。
- `tests/PoeAlarm.RulesUi.Tests`：多词缀编辑器的数据隔离、数值输入、上限和 POE2 换行模板回归测试。
- `tests/PoeAlarm.GuardedPolicy.Tests`：Mirror Tier Guarded 的稳定帧、严格判定、单次制作点击放行、黄色暂停、watchdog、手动继续/停止和输入成对释放测试。
- `tests/PoeAlarm.Poe2CorpusProbe`：POE2 中英 PoEDB 语料、数值占位、长碑牌组合及交叉误报审计。
- `tests/PoeAlarm.MirrorCorpusProbe`：来自 PoEDB、PoE2DB 与官网实际镜像服务物品的复合规则语料，覆盖 OR、至少 N 条、数值边界、Hybrid 一对一计数与近邻负例；详见 [Mirror Tier 语料报告](docs/MIRROR_RULE_CORPUS.md)。
- `tools/PoeAlarm.OcrProbe`：Windows OCR ROI/缩放基准工具。
- `tools/PoeAlarm.RecognizerProbe`：内置 Paddle 兼容路径的 recognizer-only 基准工具。
- `tools/PoeAlarm.TransientReplay`：内置 Paddle 路径的瞬态目标与点击节奏实验工具。
- `tools/PoeAlarm.EndToEndProbe`：English 与 Windows-first 繁中真实截图的产品管线端到端探针。
- `tools/PoeAlarm.UiSnapshot`：主界面、监控浮窗与红色告警的离线视觉快照、窗口行为断言及 PoEDB 合成目标/近邻负例测试。

OCR、截屏和告警均位于独立接口后。首版的小区域 GDI/DIB 截屏已经避免每帧重建原生资源；如果实机基准证明截屏成为瓶颈，可直接替换为 DXGI Desktop Duplication，而无需改动匹配与界面。

## 开发与验证

仓库由 `global.json` 固定 .NET SDK `10.0.302`。全新 clone 请先安装该 SDK，再使用系统
`dotnet`；本工作区也可把下列 `dotnet` 替换为 `.\.tools\dotnet\dotnet.exe`。

公开仓库只保存体积很小的 JSON manifest/语料，原始游戏截图因体积和桌面隐私不进入 Git。下面两个 `EndToEndProbe --manifest` 命令需要先在对应目录放回本地截图；Core、POE2 corpus、build 与 publish 不受影响。

```powershell
dotnet restore PoeAlarm.slnx -p:NuGetAudit=false -m:1
dotnet build PoeAlarm.slnx -c Release --no-restore -m:1
dotnet run --project tests\PoeAlarm.Core.Tests\PoeAlarm.Core.Tests.csproj -c Release --no-build
dotnet run --project tests\PoeAlarm.Rules.Tests\PoeAlarm.Rules.Tests.csproj -c Release --no-build
dotnet run --project tests\PoeAlarm.RulesUi.Tests\PoeAlarm.RulesUi.Tests.csproj -c Release --no-build
dotnet run --project tests\PoeAlarm.GuardedPolicy.Tests\PoeAlarm.GuardedPolicy.Tests.csproj -c Release --no-build
dotnet run --project tests\PoeAlarm.MirrorCorpusProbe\PoeAlarm.MirrorCorpusProbe.csproj -c Release --no-build
dotnet run --project tests\PoeAlarm.Poe2CorpusProbe\PoeAlarm.Poe2CorpusProbe.csproj -c Release --no-build
dotnet run --project tools\PoeAlarm.EndToEndProbe\PoeAlarm.EndToEndProbe.csproj -c Release --no-build -- --manifest tests\screenshots\8.11\traditional-ocr-8.11.json
dotnet run --project tools\PoeAlarm.EndToEndProbe\PoeAlarm.EndToEndProbe.csproj -c Release --no-build -- --manifest tests\screenshots\poe2\poe2-ocr-manifest.en.json --poe2-en-recovery
dotnet run --project tools\PoeAlarm.UiSnapshot\PoeAlarm.UiSnapshot.csproj -c Release --no-build -- --poedb-corpus tests\screenshots\poedb-traditional-affix-corpus.json
dotnet run --project tools\PoeAlarm.UiSnapshot\PoeAlarm.UiSnapshot.csproj -c Release --no-build -- --assert-batch-ocr-contract --assert-batch-ocr-synthetic
dotnet restore src\PoeAlarm.App\PoeAlarm.App.csproj -r win-x64 -p:NuGetAudit=false -m:1
dotnet publish src\PoeAlarm.App\PoeAlarm.App.csproj -c Release -p:PublishProfile=PortableWinX64 --no-restore -m:1
```

`-m:1` 是当前受管执行环境的规避项：这里的并行 MSBuild 节点通信会留下孤儿进程；项目与 `.slnx` 本身没有并行配置错误。

## 下一阶段

1. 在 POE1 / POE2 内做长时间高速制作与不同分辨率、UI 缩放的漏报/误报采样。
2. 持续把新繁中与碑牌截图加入 manifest，特别覆盖真实八行换行和不同字体缩放。
3. 根据实测加入可保存的识别配置档，而不是修改 English 的稳定阈值。
4. 若 GDI 在特定全屏模式下不可用，再加入 DXGI 捕获实现。
5. 增加代码签名、安装包、更新清单和可回滚版本。
6. 按[Trade Tracker OCR 改造计划](docs/TRADE_TRACKER_OCR_REFACTOR_PLAN.md)将已验证的限定词库、分区识别和数字专用识别器抽成可复用 OCR 内核，供 POE1/POE2 Trade Tracker 使用。

## 作者与支持

- 作者：SouNd
- 联系邮箱：[soundmys1994@gmail.com](mailto:soundmys1994@gmail.com)
- 项目主页：[SouNdmys/POE-Alarm](https://github.com/SouNdmys/POE-Alarm)

如果它帮你少点过了一条好词缀，给仓库点一个 Star 就是很直接的支持。当前没有内置付款、二维码或联网捐赠功能；后续若作者提供正式赞助链接，再以不打扰核心操作的方式放进“使用说明”。
