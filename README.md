# POE Alarm

POE 制作(Craft)时的本地 OCR 告警工具。它持续读取你框选的屏幕区域,识别装备提示框中的蓝色词缀;命中你设定的目标词缀组合后立即停止扫描、循环播放提示音,并弹出会阻挡后续鼠标点击的红色锁定窗——防止手速把刚洗出来的词缀点掉。

当前正式版本:**1.0.0(纯原生 Rust 版)**。目标环境为 Windows 10/11 x64,支持 POE1 / POE2 的 English 与繁體中文客户端。不使用 .NET、Tauri 或 WebView;不联网、无账号、无遥测,只读取屏幕像素,绝不向游戏合成或补发任何输入。

> 本仓库曾以 .NET (WPF) 实现 1.0;该实现已退役并被本 Rust 版完整替代,历史可在 git 记录中查阅。

## 下载与运行

从 [Releases](https://github.com/SouNdmys/POE-Alarm/releases) 下载 ZIP,解压后直接运行 `poe-alarm-app.exe`,无需安装任何运行时。首次运行 Windows 可能显示未签名安全提醒。

从源码构建:

```powershell
cargo build --manifest-path rust\Cargo.toml --release -p poe-alarm-app
```

产物在 `rust\target\release\poe-alarm-app.exe`。

## 使用流程

1. 在标题栏选择游戏(POE 1 / POE 2)和与客户端一致的识别语言(繁体中文 / English)。两款游戏分别保存词缀规则、框选区域和语言。
2. 从游戏或 PoEDB / PoE2DB 复制完整词缀,粘贴到"完整词缀模板"。数值自动识别为占位,数值条件默认**不限制**;需要卡数值时把该行比较方式改成 范围 / ≥ / ≤ / =。
3. 需要多个可接受结果时用"+方案"(方案之间是"或者"),同方案内用"+词缀"添加多条,配合"什么时候提醒"选择 任意 / 全部 / 指定条数。所有编辑即时自动保存。
4. 回到游戏,按 `Ctrl+Shift+F11` 只框选装备提示框中的词缀区域——区域越小,识别越快。
5. 按 `Ctrl+Shift+F10` 开始监控,正常洗装备。状态浮窗显示监控状态与计时;识别未出结果时鼠标完全直通,不会打断制作点击。
6. 命中后红色锁定窗接管全部鼠标点击(外围透明、不遮挡画面),先检查装备,再点"确认"或按 `Ctrl+Shift+F12` 解除;确认后约 300ms 仍会吸收双击尾击。下一轮需重新按 F10。

程序内"使用说明"页包含同样的引导、繁中 OCR 安装方法与作者联系方式。"识别截图"可用存档截图回放整套识别与规则管线,适合进游戏前验证模板。

### 全局热键

| 热键 | 作用 |
| --- | --- |
| `Ctrl+Shift+F10` | 开始监控(命中或停止后需重新按) |
| `Ctrl+Shift+F11` | 框选识别区域(Esc 取消) |
| `Ctrl+Shift+F12` | 停止监控 / 解除命中锁定 |

### 状态浮窗与提醒

- 浮窗未监控时可直接拖动,位置自动保存;监控中变为点击穿透、不抢焦点。冷灰=未监控,墨青=监控中。
- "提醒与显示"页可设置浮窗显隐、程序浮层是否出现在录屏中(默认可见),以及自定义命中提示音(本地 PCM WAV,路径只保存在本机)。内置音效为程序原创合成,不含游戏音频素材。

## 繁中识别加速(强烈建议)

繁体中文客户端建议安装 Windows 的 `zh-TW` OCR 能力,识别走更快更准的系统路径。管理员身份打开 PowerShell:

```powershell
Add-WindowsCapability -Online -Name "Language.OCR~~~zh-TW~0.0.1.0"
```

验证(看到 `State : Installed` 即成功,然后重启 POE Alarm):

```powershell
Get-WindowsCapability -Online -Name "Language.OCR~~~zh-TW~0.0.1.0"
```

也可以走系统设置:设置 → 时间和语言 → 语言和区域 → 添加"中文(台灣)"。未安装时程序自动退回 EXE 内置的 PP-OCRv5 离线兼容引擎,功能完整但速度与覆盖略低。两种路径都不需要 Python、Paddle 框架或联网。实测数据与边界见[繁體中文 OCR 生产说明](docs/traditional-chinese-ocr.md)。

## 匹配规则

程序不猜关键词,也不需要内置全量词缀库。你粘贴的完整词缀就是本次监控的临时记录。例如:

```text
(6—8)% increased Attack Speed if you've dealt a Critical Strike Recently
```

归一化为:

```text
<PCT> increased attack speed if you've dealt a critical strike recently
```

`#`、实际数值、固定数字、数值区间以及高级描述中的 `8(6-8)%` 都映射为带类型的数值占位符;百分比/普通数与正/负作为结构保留。除数值外,所有文字及顺序必须完整一致,Attack/Cast、Cold/Fire、dealt/killed 等语义近邻不会互相命中;OCR 掉字的行单帧不判命中(防误报),监控中靠下一帧重扫自愈。数值条件只比较屏幕上实际显示的值(催化剂、品质或特殊效果会改变显示值),不推算基础值。POE1 逻辑词缀可跨 1–4 条相邻物理行,POE2 支持最多 8 行以覆盖长碑牌词缀。同一条实际词缀在一种结果内最多计数一次。

## 安全边界

- 只读取屏幕像素;绝不合成、补发、重放或排队任何输入。
- 命中前不安装任何低级鼠标闸门,识别期间所有点击直通游戏——这保证手感,也意味着极快连点可能在几十毫秒识别窗口内点过目标。
- 严格命中后,程序先呈现并验证红色锁定层确实可见、可点击且覆盖整个虚拟桌面,才开始拦截输入;无法可靠显示时明确报错并停止,不把隐藏窗口当保护。
- 设置保存在 `%LOCALAPPDATA%/PoeAlarm/settings.json`。从 Rust 预览版升级时首次启动自动迁移预览设置;旧 .NET 设置留档为同目录 `settings.json.dotnet-1.0.bak`。

## 工程结构

`rust/` 工作区按层拆分:

- `poe-alarm-core` — 词缀归一化、整句匹配、结构化规则引擎与数值约束。
- `poe-alarm-vision` / `poe-alarm-ocr-win` / `poe-alarm-ocr-paddle` — 截屏解码、蓝字掩膜与分行、Windows OCR 与内置 PP-OCRv5 兼容路径。
- `poe-alarm-recognition` / `poe-alarm-monitoring` / `poe-alarm-runtime` — 识别编排、监控循环、生产运行时。
- `poe-alarm-platform-win` / `poe-alarm-alert-win` — 热键、HUD 浮窗、框选、WAV 播放、红色锁定层与鼠标防护。
- `poe-alarm-app` — GPUI 前端(Ledger 设计),规则台单窗口。
- `poe-alarm-settings` — 设置模型、schema 兼容与迁移。

验证入口:

```powershell
cargo fmt --manifest-path rust\Cargo.toml --all -- --check
cargo clippy --manifest-path rust\Cargo.toml --workspace --all-targets --locked -- -D warnings
cargo test --manifest-path rust\Cargo.toml --workspace --all-targets --locked --release --no-fail-fast
```

截图回归工具(`recognition-manifest-probe` / `recognition-screenshot-probe`)可对真实游戏截图批量验证正例与语义近邻负例;公开仓库只保存 JSON manifest,原始截图因体积与隐私不入库。

发布 ZIP 附带 `THIRD-PARTY-NOTICES.md` 与 `licenses/`,是内置离线 OCR 运行时和模型的许可文件,请勿从二次分发包中删除。

## 作者与支持

- 作者:SouNd
- 联系邮箱:[soundmys1994@gmail.com](mailto:soundmys1994@gmail.com)
- 项目主页:[SouNdmys/POE-Alarm](https://github.com/SouNdmys/POE-Alarm)
