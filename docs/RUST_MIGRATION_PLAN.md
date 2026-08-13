# POE Alarm 原生 Rust 全量迁移规格

- 状态：阶段 A–D 已闭环，阶段 E 实机预览验收中
- 迁移基准：`.NET 1.0.0`（Git 标签 `v1.0.0`）
- 目标平台：Windows 10/11 x64
- 架构限制：原生 Win32 / WinRT，不使用 Tauri、WebView、浏览器运行时或 .NET 运行时

本文是 Rust 版本的验收合同，不是“先做一个能打开的界面”的愿望清单。Rust 版本只有在功能、识别安全性、输入释放和性能门禁全部通过后，才可以替代 .NET 1.0。迁移期间 `.NET 1.0.0` 的标签、发布资产和设置文件都必须可随时回退。

## 1. 边界与不可变原则

1. 保留单词缀快速模式、多词缀组合、数值约束、POE1 / POE2、English / 繁體中文、截图测试、状态浮窗、红色命中告警、提示音、全局热键和 1.0 设置迁移。
2. 完整词缀和物理行身份是报警依据。不得为了速度改成关键词、任意编辑距离、全局繁简转换或同一词缀重复计数。
3. 识别准确率与输入安全优先于延迟；只接受“准确率不退步且延迟下降”的优化。
4. 所有 OCR、模型推理和设置均留在本机；不得加入网络、遥测、账号或自动抓取。
5. Rust 预览版不得覆盖 `.NET 1.0` 的设置。正式切换前必须自动备份，且旧版 EXE 和发布 ZIP 不删除。
6. **不移植已经删除的谨慎模式 / Mirror Tier 安全模式**：不实现黄色暂停、每次点击确认、逐次点击放行或其状态机。

这里需要特别区分两个名称相近但含义不同的功能：

- 必须迁移：繁中与 POE2 在画面变化后使用的短时 `WH_MOUSE_LL` 输入闸门。它只在 OCR 未决期间成对丢弃后续按键，未命中、停止、异常或超时立即放行。
- 明确不迁移：已删除的“谨慎模式”。内部语料名 `MirrorCorpus` 只是高端装备的复合规则测试；瞬态回放参数 `--roll-model guarded` 只是模拟上述短时输入闸门，均不代表谨慎模式仍然存在。

## 2. .NET 1.0 权威基线

基线源码位于 `src/PoeAlarm.Core` 与 `src/PoeAlarm.App`，公开行为以 `README.md`、`RELEASE_NOTES_1.0.0.md` 和标签 `v1.0.0` 为准。2026-08-13 在仓库随附的 .NET 10.0.302 工具链上重新运行了纯规则与公开语料基线：

| 基线 | 当前结果 | Rust 切换要求 |
|---|---:|---:|
| 完整词缀 matcher | 31/31 | 逐例相同，新增 Rust 反例也必须通过 |
| 多词缀与数值规则 | 32/32 | 逐例相同；20 条规则 warm < 1 ms |
| 规则编辑器数据/文案 | 10/10 | 行为与中英文语义相同 |
| 高端装备复合规则语料 | 36/36 | 15 正例、21 负例全部一致 |
| POE2 中英 PoE2DB 语料 | 334/334 正例 | 562/562 缺行负例、298/298 实值、2/2 八行换行均一致 |
| 规则热路径（本次 .NET 对照） | 20 条 `0.318 ms`；稠密 32×31 `6.570 ms` | Rust 不得更慢；普通热路径目标 ≤ 0.25 ms |

Rust 核心现已直接读取与 .NET 相同的两份数据库语料，而不是复制成一套较小的 Rust fixture。2026-08-13 的 Release 回归结果为：POE1 PoEDB 繁中 163/163 正例、58/58 缺行反例、472/472 有向近邻反例和 13,203 组模板交叉检查全部通过；POE2DB 中英 334/334 正例、562/562 缺行反例、334/334 数值模板、298/298 范围实值和 2/2 八行物理换行全部通过。该结果只证明 matcher/规则层等价，不能代替真实图片 OCR。

首个原生热路径检查点（同机、Release、合成 BGRA 输入 `1433×1117`、预热 200 次、采样 3,000 次）已经建立。两端生成的蓝字语义指纹均为 `CEC2A8DDB1A49BCB`。交换运行顺序复测得到：

| 同口径流程 | .NET 1.0 p50 / p95 | Rust p50 / p95 | 当前差异 |
|---|---:|---:|---:|
| 蓝字筛选 + 语义指纹 | `1.222 / 1.382 ms` | `0.696 / 1.168 ms` | p50 快 `43.0%`；p95 快 `15.5%` |
| 筛选 + 指纹 + 物理行 + 全部 BGRA 裁剪 | `1.860 / 2.062 ms` | `1.293 / 1.842 ms` | p50 快 `30.5%`；p95 快 `10.7%` |

这证明像素/裁剪热路径已经超过切换门槛，但不代表最终产品已达到切换条件。探针源码分别位于 `tools/PoeAlarm.VisionBaselineProbe` 与 `rust/crates/poe-alarm-vision/src/bin/vision_hotpath_probe.rs`；已有私有真实截图和端到端 OCR 结果记录在 [Rust 验收报告](RUST_VALIDATION_REPORT.md)，仍缺 POE1 English 原始图片、60 秒未变化 CPU、2 小时增长和真实游戏使用门禁。

真实截图和瞬态输入的已发布基线为：

- POE2 English 真实截图 `38/38`，繁中 `38/38`；两套各 160 个跨截图目标负例误报 0。
- 2026-08-11 繁中原生大选区 `27/27`，跨截图负例 93 个误报 0；旧截图集 `65/65`。
- POE1 English 目标行与完整装备区各 `20/20`，5 条真实正例和 5 条语义近邻负例均正确。
- 163 条 PoEDB 繁中合成语料 `163/163`；153 个纯干扰、58 个缺行、29 个远距断层和 472 个有向近邻误报 0。
- POE2 中英 167 组语料：334 个显示正例、562 个缺行负例、334 个数值模板、298 个范围实值和 2 个八行换行均通过。
- 繁中 changed-frame 预检 p50 `3.0–3.1 ms`、p95 `3.3–4.8 ms`；warm 目标判定 p50 `27.0–29.2 ms`、p95 `53.0–55.9 ms`；首次模型懒加载最坏 `181–229.2 ms`。
- POE2 English 判定 p50 `4.0 ms`、p95 `75.3 ms`；繁中判定 p50 `34.5 ms`、p95 `59.6 ms`。
- 30/40/50 ms 繁中高速回放 750 次：及时拦截 `750/750`，点过头、截到却漏报和提前误报均为 0，捕获到判定 p95 `59.1 ms`。
- .NET 1.0 自包含单文件 EXE 为 94,282,857 B（89.92 MiB），发布 ZIP 为 85,814,164 B（81.84 MiB）；Rust 体积以全部必需文件总和对比，不把系统外置依赖藏到统计之外。

2026-08-13 又在同机、同 ROI、同原图上重新运行了 .NET 1.0 的四份私有截图清单，作为 Rust 适配层的直接 A/B 起点：

| 私有真实截图清单 | .NET 1.0 结果 | 本轮判定延迟 |
|---|---:|---:|
| POE2 English，6 图 / 38 目标 | `38/38`，160 个跨图负例误报 0 | p50 `3.8 ms`，p95 `78.7 ms`，max `94.2 ms` |
| POE2 繁中，6 图 / 38 目标 | `38/38`，160 个跨图负例误报 0 | p50 `31.7 ms`，p95 `52.7 ms`，max `60.1 ms` |
| POE1 2026-08-11 繁中，6 图 / 27 目标 | `27/27`，93 个跨图负例误报 0 | p50 `43.7 ms`，p95 `69.2 ms`，max `80.4 ms` |
| POE1 旧繁中，3 图 / 26 目标 | `26/26` | p50 `66.1 ms`，p95 `101.4 ms` |

25 张原始图片仍保存在本机只读历史工作树，未提交到公开仓库。Rust manifest runner 必须通过显式 `--image-root` 读取这些图片；图片缺失时必须失败或明确跳过并让发布门禁失败，不能用合成文字顶替。

Rust 繁中最终五进程 Quick A/B 为：POE2 `190/190`，warm p50/p95
`31.0343/47.8532 ms`；POE1 8.11 `135/135`，warm p50/p95
`36.3542/57.2188 ms`。旧繁中只执行了一轮 `26/26`，p50/p95
`66.176/83.869 ms`，不得写成五进程数据。与同机 `.NET` 相比，POE2 p50 只快约
`2.4%`，POE1 p50 只快约 `9.3%`；准确率保持一致且延迟已追平或更快，但都没有达到
合同中 p50 快 10% 的理想门槛，因此只支持继续 Preview，不支持正式替换。

上述时间只用于同机、同截图、同 ROI 的 A/B 比较，不是跨机器承诺。迁移验收必须重新保存两端原始样本，不能只抄本文数字。

## 3. 功能迁移矩阵

| 功能 | .NET 1.0 契约与入口 | Rust 实现要求 | 主要证据 |
|---|---|---|---|
| 游戏配置档 | POE1 / POE2 分别保存目标、区域、OCR 语言和规则 | 两档独立，切换不串值 | 设置差分测试 |
| 单词缀快速模式 | `FullLineAffixMatcher`，数值自动忽略，POE1 1–4 行、POE2 最多 8 行 | 同一模板/文字得到相同 canonical tokens 与命中结果 | 31/31、POE2 corpus |
| 多词缀组合 | 结果组之间 OR；组内 Any / All / AtLeast | 最多 8 组、总计 32 条；一次共享 OCR，不可按目标重复整帧识别 | 32/32、36/36、batch OCR contract |
| 数值条件 | Ignore / Between（闭区间）/ AtLeast / AtMost / Exactly | 使用十进制定点数，禁止用 `f32/f64` 比较；比较屏幕显示值 | Rules tests、繁中 3.73% 实图 |
| 一对一计数 | 同一物理词缀/辅助转录只能分配给组内一个条件 | 保留 source band identity、跨行联合与最大匹配分配 | identity corpus、Rules tests |
| 英文归一化 | NFKC、限定字形混淆、数值类型、语义词序严格 | 与 .NET token 流逐例相同，不扩大容错 | Core + cross-negative |
| 繁中归一化 | 空白、换行、点状小数、数字间“一”恢复；字义严格 | `+ 3 · 73 % 暴 擊 率` 必须解析为单槽 `3.73` | Core + Rules + 实图 |
| GDI 截屏 | 复用 DC、top-down 32-bit DIB 和 BGRA 缓冲，`BitBlt(SRCCOPY|CAPTUREBLT)` | 首个版本先做字节等价 GDI 后端；DXGI 只能作为可切换后端单独验收 | 截屏 fixture + 资源泄漏测试 |
| 蓝字像素筛选 | 默认阈值 blue≥105、dominance≥18、`abs(R-G)≤72`；中文局部路径 100/14/72 | `poe-alarm-vision` 一次遍历产出 mask + fingerprint，避免逐帧分配 | mask golden + criterion 基准 |
| 画面指纹 | 蓝字语义 FNV-1a，包含像素位置/强度和 ROI 尺寸，忽略动画背景 | 同一 BGRA fixture 必须与 .NET 得到相同或版本化等价指纹；未变化不进 OCR | fingerprint golden、monitor assertion |
| 物理行分区 | 每行至少 3 个墨点、容忍 2 空行、最小高 5；常规横边 10/竖边 14，整区边 8 | source bounds、fallback 标志及稳定 band id 必须保留 | bands golden、跨行 corpus |
| POE1 English OCR | Windows English OCR，分区 scale 2，严格完整匹配 | 直接调用 WinRT `Windows.Media.Ocr`，不另造模糊算法 | POE1 manifest |
| POE2 English OCR | 独立双 Windows 引擎确认、变化帧复核、有限局部 PP-OCR | 保留确认与有界复核，不退回 POE1 单次路径 | POE2 English manifest / replay |
| 繁中 OCR | zh-TW/zh-Hant 优先，zh-Hans/zh-CN 其次；整张蓝字 mask 的 WinRT 多行 OCR，局部疑难才走 PP-OCRv5 | 语言选择、空间断层、候选上限、CTC 证据和回退顺序一致 | 繁中 manifest、PoEDB corpus |
| 离线兼容引擎 | ONNX Runtime + PP-OCRv5 mobile rec + 18,383 项字典 | 使用 ONNX Runtime 原生 API；无 Windows 繁中 OCR 时 Quick/Structured 进入完整 Paddle 物理行渐进路径；100/14/72 蓝字阈值、模型/字典哈希、形状和真实推理一致 | OCR self-test + 私有行门禁 26/26 |
| 识别缓存 | frame / band 指纹缓存，最多 256 band；变化时失效 | 有边界、可观测、无跨配置污染；相同画面必须跳过重复 OCR | cache assertions + benchmark |
| 监控循环 | 未命中持续等待；非缓存轮次约 4 ms、缓存轮次约 8 ms 让出；停止/命中/异常有代际隔离 | 单一监控 worker；旧任务不得在停止、重启或关闭后报警 | wait、race、dispose assertions |
| 短时输入闸门 | 繁中和 POE2 预装 `WH_MOUSE_LL`；画面变化后同步武装；按下/抬起成对处理 | 独立 Win32 消息线程；hook callback 不做 OCR、不加阻塞锁、不调用 UI；750 ms fail-open | input-guard + 30/40/50 ms replay |
| 红色告警 | 命中后红色全屏边框/屏幕中央卡片、循环 WAV、手动确认、确认后吸收约 300 ms 双击尾击 | 每次显示重查虚拟桌面/目标显示器并响应拓扑与 DPI 变化；先验证红窗可见、可点击、完整覆盖，再转交 hook；UI 卡住时 750 ms 独立放行 | blocking/live-blocking + topology assertions |
| 状态浮窗 | 灰色未监控、绿色监控、点击穿透、不抢焦点、可拖动保存、避开 ROI | 原生 topmost/no-activate/click-through 窗口；目标摘要、计时、显隐、DPI 缩放与拖动保存均接到真实设置 | HUD snapshot + Win32 behavior test |
| 选区与快捷键 | F11 框选；默认 F10 启动（3 组可选）；F12 确认/停止 | `RegisterHotKey`；冲突时明确报错；Esc 取消选区；框选结果写回当前游戏配置 | hotkey + selection tests |
| 截图测试 | 和实时监控使用同一预处理/OCR/matcher/规则链；关闭时取消并等待 | 渐进识别允许超过 3 轮，最多 128 轮后返回明确不收敛故障；取消优先，且不弹迟到告警 | screenshot replay + close/cancel race |
| 设置 | `%LOCALAPPDATA%/PoeAlarm/settings.json`，schema 3，旧字段迁移，未来 schema 只读，临时文件原子替换 | 预览版先写独立目录；解析和保存与 .NET 双向兼容 | settings profile assertions |
| 体验设置 | 中/英界面、状态浮窗、位置、启动热键、本地 PCM WAV、浮层是否进入录屏 | 不夹杂另一语言；WAV 路径仅本地；主窗按工作区反算布局缩放，覆盖 96/120/144 DPI 与 1024×768/1920×1080；用 `SetWindowDisplayAffinity` 实现录屏选择 | UI copy + DPI/layout + manual |
| 生命周期 | 快速开始/停止、截图分析并发、关闭程序均不死锁，不残留 hook 或迟到事件 | 所有 worker、COM/WinRT、GDI、窗口、hook 可取消并有明确所有权 | 1000-cycle soak + race suite |

## 4. 原生架构

推荐工作区位于 `rust/`，保持与 .NET 工程并列。在 Rust 正式切换前，不删除或重写 `src/`、`tests/`、`tools/`。

```text
poe-alarm-app-win（WinMain + 原生 Win32 UI）
  ├─ poe-alarm-platform-win（窗口、热键、鼠标 hook、WAV、DPI）
  ├─ poe-alarm-monitoring（代际状态机、取消、缓存、alert handoff）
  ├─ poe-alarm-recognition（四种生产识别配置与有界恢复编排）
  ├─ poe-alarm-ocr-win（Windows.Media.Ocr / SoftwareBitmap 常驻线程）
  ├─ poe-alarm-ocr-paddle（ONNX Runtime、CTC、局部复核）
  ├─ poe-alarm-vision（BGRA → mask/fingerprint/bands）
  ├─ poe-alarm-core（归一化、完整词缀、数值、复合规则）
  └─ poe-alarm-settings（schema 3、原子保存、预览隔离/导入）
```

实际 crate 可以在不破坏依赖方向的前提下合并。必须遵守以下边界：

- `poe-alarm-core` 不依赖 Windows、UI、OCR 或文件系统，所有语义可在 Linux/Windows 的普通 `cargo test` 中验证。
- `poe-alarm-vision` 只处理显式传入的 BGRA/stride/ROI 数据，不自行创建窗口、不持有全局 hook；返回 mask、语义指纹、物理 band bounds 和回退标志。
- OCR 只返回带时间、物理位置和来源身份的证据，不自行决定 UI 状态或吞鼠标。
- 规则引擎是唯一报警判定者；辅助 OCR 只能提供经过约束的转录证据。
- UI 线程只拥有 HWND 和展示状态；OCR、像素处理、模型推理不在 window procedure 中执行。
- hook 线程只维护无等待的原子按键状态并返回 pass/consume；不得经过 GUI channel 才决定是否吞键。
- 监控轮次带单调递增 generation。停止、重新开始、确认和关闭会使旧 generation 的 OCR 结果失效。

### 4.1 Windows 技术选择

- 使用 `windows`/`windows-core` 调用 Win32 与 WinRT。
- 主界面使用标准 Win32 控件配合 Direct2D/DirectWrite（或完全 owner-drawn）；禁止 WebView。
- 截屏首版使用与 1.0 等价的 GDI `CreateDIBSection + BitBlt`，保证可比。DXGI Desktop Duplication 放在独立后端，在 GDI 版本闭环后进行 A/B；DXGI 不得改变 mask 或 matcher。
- 图片测试使用 WIC 解码成明确的 BGRA8 top-down 格式。
- Windows OCR 使用 `Windows.Media.Ocr.OcrEngine`；线程必须初始化正确的 COM apartment，取消和关闭不得释放仍在使用的 `SoftwareBitmap`。
- PP-OCR 使用官方 ONNX 模型及 ONNX Runtime 原生库。允许发行包包含必要 DLL，但总发行体积按 EXE、DLL、模型、字典和运行所需资产的总和计算。
- GUI manifest 使用 per-monitor-v2 DPI awareness；ROI 使用物理桌面像素，不得被逻辑 DPI 二次缩放。

### 4.2 热路径所有权

一帧的标准路径为：

```text
复用 DIB 截图 → 单次 BGRA 扫描（mask + fingerprint）
             → 指纹未变：复用结果并短暂让出
             → 指纹变化：同步武装短时输入闸门
             → bands / WinRT OCR / 有界局部 ONNX 复核
             → 完整词缀或复合规则求值
             → 未命中：释放闸门；命中：原子转交红色阻挡窗
```

正常扫描不得复制整张 ROI 两次以上；mask、行墨水、band 图像和 OCR tensor 使用池化/复用缓冲。任何 `unsafe` 必须封装在小型 Windows/ONNX 适配层，公开 API 用长度、stride 和生命周期验证防止越界或悬垂。

## 5. 数据兼容

### 5.1 设置

最终 Rust 正式版必须读写 `AppSettings.CurrentSchemaVersion = 3` 的相同 JSON：

- `SelectedGameProfile`、`Profiles.Poe1/Poe2`、`TargetAffix`、`CaptureRegion`、`OcrLanguage`、`RuleEditorMode`、`StructuredRuleSet`；
- `UiLanguage`、`KeepHudVisible`、`HudPlacement`、`AllowOverlayCapture`、`CustomAlertSoundPath`、`StartMonitoringHotKey`；
- 老的 flat POE1 字段只读迁移，保存时移除；旧预览中的 `MonitoringPolicy` 忽略并在下次保存时移除；
- JSON 属性名读取时大小写不敏感。若有重复的 `SchemaVersion`（包括不同大小写），取最高值作为安全边界；大于 3 时返回安全默认界面并把原文件保持只读，绝不覆盖；
- 无效枚举、语言和热键按 .NET 1.0 的默认值归一化；数值约束使用十进制语义；
- 保存到同目录临时文件，序列化完成后再次检查未来 schema，再原子替换；失败时原文件仍为权威。

迁移阶段使用：

```text
%LOCALAPPDATA%/PoeAlarm-RustPreview/settings.json
```

“导入 1.0 设置”只能复制读取，不得修改 `%LOCALAPPDATA%/PoeAlarm/settings.json`。正式切换当天先生成带 UTC 时间戳的 `.net-1.0.backup.json`，然后才允许 Rust 使用正式路径。

### 5.2 模型与许可证

必须保留并自检：

| 资产 | 字节 | SHA-256 |
|---|---:|---|
| `PP-OCRv5_mobile_rec.onnx` | 16,534,782 | `DA72DC72CA4DC220DF0DFDE68C1DEDC31C58D3E76A25871122E5056227D50092` |
| `ppocrv5_dict.txt` | 74,012 | `D1979E9F794C464C0D2E0B70A7FE14DD978E9DC644C0E71F14158CDF8342AF1B` |

Rust 包必须附 `THIRD-PARTY-NOTICES.md` 和实际使用组件的许可证。不得仅为减小体积删除模型或中文兼容能力；可以在证明等价后压缩嵌入并于进程内/受控缓存中读取。

## 6. 性能与可靠性门禁

### 6.1 测量方法

所有 A/B 使用同一台机器、同一电源模式、同一桌面分辨率、同一 ROI 和同一原始截图。先运行 .NET 1.0，再运行 Rust，再交换顺序复测，避免热缓存和温度偏差。时间使用 QPC/`Instant`，每项报告原始 CSV、样本数、min/p50/p95/p99/max；warm 与首次模型加载分开统计。

- 像素/指纹 microbenchmark：预热 100 次，测量至少 2,000 次。
- 每个真实截图目标：预热后至少 20 次；完整 manifest 至少重复 5 轮。
- 瞬态回放：每个必测场景、每种 30/40/50/60/80/100 ms 节奏至少 20 次。
- 启动时间：冷启动 20 次，记录进程创建到主窗可交互。
- 内存：主窗空闲 30 秒、监控未变化 60 秒、WinRT OCR warm、ONNX warm 四个点分别记录 private bytes、working set、handle/thread 数。
- 稳定性：1,000 次开始/停止、500 次截图分析后立即停止/关闭、连续监控 2 小时。

### 6.2 切换硬门槛

| 指标 | Rust 候选门槛 |
|---|---|
| 准确率 | 所有 .NET 正例逐例保留；所有既有负例误报 0；不得用放宽 matcher 达成 |
| 蓝字 mask + fingerprint | 同机 p50 至少比 .NET 快 15%，p95 至少快 10%；1131×928 ROI 的目标为 p50 ≤ 2.6 ms、p95 ≤ 4.0 ms |
| 未变化帧 | 不调用 OCR；整轮 p95 不高于 .NET，60 秒空闲监控 CPU 不高于 .NET |
| warm 变化帧端到端 | capture→最终规则判定 p50 至少比 .NET 快 10%，p95 不得比 .NET 慢 5%；繁中目标为 p50 ≤ 26 ms、p95 ≤ 53 ms |
| 首次 ONNX 懒加载 | 同机不高于 .NET 对照的 105%，且绝对值 ≤ 240 ms |
| 规则求值 | 20 条普通规则平均 ≤ 0.25 ms 且 < 1 ms；稠密 32×31 ≤ .NET 对照 |
| 高频制作 | 30/40/50 ms 必测回放 hit/timely 100%，overroll=0、captured-miss=0、false-alert=0；60/80/100 ms 同样全部为 0 失败 |
| 输入 fail-open | 任意异常/停止/关闭后 750 ms 内释放；100% 保持 Down/Up 成对；不补发、不重放、不排队 |
| 启动 | 主窗可交互 p50 至少比自包含 .NET 1.0 快 30% |
| 内存 | UI 空闲 private bytes ≤ .NET 的 70%；ONNX warm ≤ .NET 的 85%；2 小时增长 ≤ 5 MiB、handle 增长 ≤ 2% |
| 发行体积 | 所有必需文件解压后总计 ≤ 50 MiB，ZIP ≤ 45 MiB；无需用户安装 Rust/.NET/VC 运行环境 |
| 无卡死 | soak/race 全部完成；关闭到进程退出 ≤ 1 秒；退出后无 hook、窗口、声音或子进程残留 |

若 Windows OCR 的系统噪声令端到端 10% 中位数提升不稳定，可以提交完整数据申请只以 hotpath 提升作为性能结论，但这只允许继续预览，不足以宣布全面弃用 .NET。最终切换必须同时有统计数据和实机手感确认。

当前数据正属于这一情况：POE2 繁中已经从早期落后优化为 p50/p95
`31.0343/47.8532 ms`，但相对 `.NET` 的 p50 提升只有约 `2.4%`；POE1 8.11 繁中
p50 提升约 `9.3%`。两条路径准确率均完整保留，但尚不能按本合同宣称“端到端快 10%”。

## 7. 对照与回归命令

在当前仓库中先固定 .NET 对照。若系统 `dotnet` 不满足 `global.json`，使用仓库内工具链：

```powershell
$env:DOTNET_CLI_HOME = "$PWD\.tools\dotnet-home"
$env:DOTNET_SKIP_FIRST_TIME_EXPERIENCE = "1"

.\.tools\dotnet\dotnet.exe restore PoeAlarm.slnx -p:NuGetAudit=false -m:1
.\.tools\dotnet\dotnet.exe build PoeAlarm.slnx -c Release --no-restore -m:1
.\.tools\dotnet\dotnet.exe run --project tests\PoeAlarm.Core.Tests -c Release --no-build
.\.tools\dotnet\dotnet.exe run --project tests\PoeAlarm.Rules.Tests -c Release --no-build
.\.tools\dotnet\dotnet.exe run --project tests\PoeAlarm.RulesUi.Tests -c Release --no-build
.\.tools\dotnet\dotnet.exe run --project tests\PoeAlarm.MirrorCorpusProbe -c Release --no-build
.\.tools\dotnet\dotnet.exe run --project tests\PoeAlarm.Poe2CorpusProbe -c Release --no-build
```

真实截图与生产管线：

```powershell
.\.tools\dotnet\dotnet.exe run --project tools\PoeAlarm.EndToEndProbe -c Release --no-build -- --manifest tests\screenshots\8.11\traditional-ocr-8.11.json
.\.tools\dotnet\dotnet.exe run --project tools\PoeAlarm.EndToEndProbe -c Release --no-build -- --legacy-manifest tests\screenshots\traditional-ocr-cases.json
.\.tools\dotnet\dotnet.exe run --project tools\PoeAlarm.EndToEndProbe -c Release --no-build -- --manifest tests\screenshots\poe2\poe2-ocr-manifest.en.json --poe2-en-recovery
.\.tools\dotnet\dotnet.exe run --project tools\PoeAlarm.EndToEndProbe -c Release --no-build -- --manifest tests\screenshots\poe2\poe2-ocr-manifest.zh-TW.json
.\.tools\dotnet\dotnet.exe run --project tools\PoeAlarm.UiSnapshot -c Release --no-build -- --poedb-corpus tests\screenshots\poedb-traditional-affix-corpus.json
```

监控、设置、UI、生命周期与输入闸门：

```powershell
.\.tools\dotnet\dotnet.exe run --project tools\PoeAlarm.UiSnapshot -c Release --no-build -- --assert-monitor-wait --assert-stop-match-race --assert-target-aware-monitor --assert-structured-monitor --assert-structured-replay --assert-batch-ocr-contract --assert-batch-ocr-synthetic
.\.tools\dotnet\dotnet.exe run --project tools\PoeAlarm.UiSnapshot -c Release --no-build -- --assert-concurrent-dispose --assert-close-during-stop --assert-input-guard --assert-settings-profiles --assert-start-hotkey --assert-plain-language-copy
.\.tools\dotnet\dotnet.exe run --project tools\PoeAlarm.UiSnapshot -c Release --no-build -- --assert-band-fingerprints
.\.tools\dotnet\dotnet.exe run --project tools\PoeAlarm.UiSnapshot -c Release --no-build -- --alert --assert-blocking --assert-live-blocking artifacts\baseline\ui-alert.png
.\.tools\dotnet\dotnet.exe run --project tools\PoeAlarm.UiSnapshot -c Release --no-build -- --hud --assert-hud artifacts\baseline\ui-hud.png
```

发布物和内置资产还必须单独验证；这两项不能由普通 `dotnet test` 代替：

```powershell
.\.tools\dotnet\dotnet.exe publish src\PoeAlarm.App -c Release -p:PublishProfile=PortableWinX64 --no-restore -m:1
artifacts\publish\1.0.0\win-x64\PoeAlarm.exe --ocr-self-test artifacts\baseline\ocr-self-test.json
artifacts\publish\1.0.0\win-x64\PoeAlarm.exe --audio-self-test artifacts\baseline\audio-self-test.json
```

`tools/PoeAlarm.OcrProbe` 用于隔离 Windows OCR 延迟；`tools/PoeAlarm.RecognizerProbe` 用于模型、字典、单行/整区、CER 和 manifest 诊断。它们不直接宣告产品命中，但 Rust 对照必须保留对应的 recognizer-only microbenchmark，才能区分“像素热路径变快”和“Windows OCR 本身波动”。

瞬态回放必须按 `tools/PoeAlarm.TransientReplay/README.md` 的四类路径运行：普通 top-1、真实像素换行、CTC-assisted、调度 rank-3。`guarded` 参数仍表示短时输入闸门模型，不表示已删除的谨慎模式。

Rust 当前工作区统一验证入口为：

```powershell
cargo fmt --manifest-path rust\Cargo.toml --all -- --check
cargo clippy --manifest-path rust\Cargo.toml --workspace --all-targets --locked -- -D warnings
cargo test --manifest-path rust\Cargo.toml --workspace --all-targets --locked --release --no-fail-fast
```

OCR、replay、runtime 和原生 app crate 均已落地。私有 manifest runner 必须继续读取与
`.NET` 相同的 JSON 和原始图片；不得只写新的 Rust fixture 而绕过现有截图和
PoEDB/PoE2DB 数据。差分时比较 canonical tokens、lines、physical ids、numeric values、
matched group 和最终报警决定。

2026-08-13 的用户繁中实图是强制回归：模板 `+(3.11—3.8)%暴擊率`，实际显示 `+3.73% 暴擊率`，数值条件“至少 3.1”必须命中；低于 3.1 必须不命中。该图片属于本地私有截图集，未放入公开仓库时，发布负责人必须在本地 manifest 中恢复它，不能用纯文本测试替代实图 OCR。

## 8. 分阶段交付、切换与回滚

### 阶段 A：冻结基线（已完成）

- 保持 `v1.0.0`、GitHub Release 和哈希不变。
- 保存 .NET 的逐例 decision trace、性能 CSV、机器/系统/OCR 语言包信息。
- Rust 使用独立 EXE 名、互斥锁和 `PoeAlarm-RustPreview` 设置目录。

### 阶段 B：纯逻辑和像素热路径（已完成）

- 移植 matcher、数值提取、多词缀分配、JSON schema。
- 移植 BGRA mask、fingerprint、band 切分，并对同一 frame 做 golden/differential test。
- 只有在功能 100% 等价后才采纳 SIMD、rayon 或 DXGI；优化前后各保留 benchmark。

### 阶段 C：OCR 生产路径（已完成）

- 已闭环 POE1 English、POE2 English、繁中 Windows-first 与完整 Paddle fallback；繁中
  fallback 保持 100/14/72 蓝字阈值并支持 Quick/Structured 渐进识别。
- 每条路径先跑真实 manifest，再跑跨目标负例和瞬态回放。
- Rust 与 .NET 不可同时安装低级 hook；A/B 应分进程、分轮运行。

### 阶段 D：监控、告警和原生 UI（已完成自动化闭环）

- 接入代际取消、短时闸门、红窗 handoff、HUD、选区、热键、WAV 和设置。
- 已通过代际竞态、close、fail-open、高 DPI 布局、多显示器拓扑和录屏兼容行为测试；
  POE2 八行、F10/F11/F12、HUD、框选、截图最多 128 轮和红窗所有权转交均接到生产链。
- UI 文案以用户能直接理解为准；中文不夹英文术语，English 不夹中文。

### 阶段 E：Rust 预览实机（进行中）

- 先提供并排可回退的预览 ZIP，不覆盖 .NET 设置、快捷方式或 GitHub `latest`。
- 至少完成 POE1/POE2 × English/繁中四种组合；在实际游戏中各运行 30 分钟高速制作。
- 至少三次独立使用时段无卡死、无漏报/误报、无 hook 残留，并记录用户对卡顿的主观体验。

### 阶段 F：正式切换

只有所有硬门禁通过后才：

1. 备份正式 settings；
2. 发布 Rust 候选及 SHA-256、许可证、完整 A/B 报告；
3. 先把 Rust 标为推荐版，保留 `.NET 1.0.0` 下载和回退说明至少一个正式发布周期；
4. 稳定期后才停止 .NET 功能开发。Git 历史、标签和 release 永不删除。

以下任一项会立即停止切换并回退 `.NET 1.0.0`：

- 任意既有正例漏报、既有负例误报或数值边界错误；
- 任意监控启动/停止/截图/关闭卡死，或退出后 hook/红窗/声音残留；
- 输入被阻挡超过 750 ms，Down/Up 不成对，或发生补发/重放；
- 设置丢失、跨 POE 配置串值、未来 schema 被覆盖；
- warm p95 连续两轮比同机 .NET 慢超过 5%，或体积/内存超过门禁；
- 只能通过降低 OCR 严格度、删除繁中 fallback 或移除现有功能来达标。

## 9. 完成定义

“Rust 已全量迁移”必须同时有下列证据，缺一项都只能称为预览：

- 功能矩阵逐项有实现文件和自动/人工测试记录；
- 公开 corpus、私有真实截图、PoEDB/PoE2DB、数值 3.73% 实图全部通过；
- 六档瞬态回放和输入闸门测试无过点、误报、漏报、卡键；
- 性能 CSV 证明像素/指纹与 warm 端到端达到门禁，而非只展示规则 microbenchmark；
- 1,000 次生命周期和 2 小时 soak 无卡死、泄漏、迟到报警；
- 原生 ZIP 无 Tauri/.NET 依赖，体积、许可证、模型自检和 SHA-256 完整；
- `.NET 1.0.0` 设置已备份，回滚步骤在干净 Windows 用户环境实际演练过；
- 谨慎模式相关黄色暂停和逐次放行代码不存在于 Rust 正式二进制。
