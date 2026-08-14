# 繁體中文 OCR 生产说明

POE Alarm 从 0.4.4 起将繁中识别与 POE1 English 路径完全分开：POE1 English 继续使用
已经验证过的 `WindowsOcrRecognizer`；繁中由 `WindowsChineseOcrRecognizer` 处理整张
蓝字掩膜，只有局部疑难候选或系统没有 Windows 中文 OCR 能力时才使用内置 PP-OCRv5。
0.6.0 又为 POE2 English 增加独立复核层，两者不会改写 POE1 English 的识别规则。

程序优先选择 Windows `zh-TW`/`zh-Hant` OCR，其次尝试 `zh-Hans`/`zh-CN`。本轮测试机已
安装并实际启用 `zh-Hant-TW`。若系统没有任何中文 OCR，程序会自动退回内置离线兼容
引擎；该路径不需要 Python、Paddle 框架或联网，但速度与覆盖保证低于 Windows 快速路径。

安装 `zh-TW` 的收益主要是稳定性和直接识别覆盖，而不是保证每一帧都更快：同一批 8.11
截图中，中位数可能与 `zh-Hans` 同档或略慢，但最坏长尾从约 256 ms 降至约 83 ms；163 条
PoEDB 合成语料中直接命中从 130 条提高到 154 条，依赖局部辅助的案例从 33 条降至 9 条。
因此 English 用户无需安装，繁中用户建议安装。无该组件时软件仍可工作，但不同系统环境
不可能承诺完全相同的延迟与识别覆盖。

## 0.4.4 修复的根因

旧繁中主路径使用单行 PP-OCRv5 recognition 模型，并先用整个监控区域的横向像素投影
切行。用户实际框选的 `1131 × 928` 区域内，提示框背后的仓库图标与堆叠数量也含蓝色
像素。这些像素填满了词缀之间本来存在的空白行，使三条紧凑词缀被误合并为一张
`907 × 111` 多行图片；单行模型随后只输出一个“傷”，恢复扫描又把一次失败拖到数百
毫秒。按住 Alt 并不是让截图更清晰，而是 POE 的高级词缀说明行碰巧把蓝字撑得更开，
因此掩盖了这个分行缺陷。

0.4.4 不再让繁中主路径依赖这套物理分行：

```text
框选区域 -> 蓝色字形掩膜与内容指纹 -> Windows 中文多行 OCR
         -> 完整词缀严格匹配
         -> 未严格命中时，只对最多数个相近候选做原色局部复核
         -> 极少数仍有歧义的局部行才调用内置 Paddle/CTC 证据
         -> 命中后升级输入盾为红色告警
```

Windows OCR 自带多行版面分析，因此宽选区里的蓝色图标不再把多条词缀作为单行压缩。
局部 Paddle 只接收 Windows 已经定位出的少量原色候选，不再对整个 ROI 做渐进式全框
恢复。English 的构造、语言选择、分行、匹配与鼠标行为均未接入这些繁中特有规则。

## 匹配与纠错边界

用户粘贴的完整 PoEDB 词缀仍是唯一目标。数值、区间和高级说明中的数值映射为带类型的
数值槽；所有繁中词义字符和顺序仍参与比较，纯样式标点与空白会被归一化。程序不会用
关键词、任意编辑距离、全局繁简转换或“长得像就算对”的汉字表触发告警。

Windows OCR 有时会把阿拉伯数字区间中的短横识别为汉字“一”。0.4.4 只在“一”两边的
最近非空白字符均为阿拉伯数字时把它恢复为 `-`；普通词语中的“一”不受影响。其他疑难
字必须经过独立的原色局部重识别，或在 Windows 定位出的少量相近候选内获得 Paddle CTC
同位置 top-3 概率证据。原色复核必须通过完整 matcher；最后一种 CTC 路径只允许最多两个
目标汉字替换，并同时受固定概率、排名、距离和“整帧只能有一个辅助候选”约束，不是全局
模糊匹配。

Windows OCR 不会返回空白行，程序会根据文字坐标在明显的纵向空间断层处补回 matcher
边界，避免把相距很远的蓝色 UI 文本拼成一个复合词缀。紧邻物理行仍允许组成换行词缀；
如果两条独立且紧邻的词缀文字恰好完整组成用户输入的某个双行复合词缀，仅凭蓝字像素
无法判断它们是否来自同一个 affix group，这是当前边界。单行目标不受该歧义影响。

## 回归结果

真实截图位于 `tests/screenshots`：

- 2026-08-11 原生截图：5 张非 Alt 紧凑界面、1 张 Alt 高级界面，实际大 ROI 内
  `27/27` 命中；93 个跨截图目标负例误报 0。
- 旧微信/Alt 截图：7 张图、65 个目标，升级后 `65/65` 命中。
- 本机多次 Release 回归中，原生大 ROI 的 changed-frame 预检 p50 `3.0–3.1 ms`、
  p95 `3.3–4.8 ms`；目标判定 p50 `27.0–29.2 ms`、p95 `53.0–55.9 ms`。27 个目标中
  23 个由 Windows OCR 直接严格命中，4 个使用局部复核。首次懒加载内置模型的单次
  最坏判定为 `181–229.2 ms`。
- English 独立回归：目标行 `20/20`，完整装备区 `10/10`；warm 分别为
  `2.9–3.1 ms` 与 `26.2–28.1 ms`，没有发现相对旧基线的准确率或速度回归。

PoEDB 抽样语料位于 `tests/screenshots/poedb-traditional-affix-corpus.json`，包含 163 条
珠宝、深渊珠宝、武器和护甲词缀：

- 相似蓝字、多行和干扰词缀的合成目标语义命中：`163/163`，其中主结果 130、目标辅助
  结果 33、缓存命中 0。它验证生产 matcher 的最终决定，不代表 163 条 raw transcript
  都逐字相同。
- 完整生产 recognizer 的负例：153 个纯干扰目标、58 个逐行缺失复合目标、29 个远距
  行断层目标和 472 个有向相似词缀目标，共 712 次，误报 0。
- 13,203 组模板两两匹配中，只有 11 组跨装备域的原文完全重复会命中；153 个不同模板
  之间误命中 0。
- 472 个有向人工近邻通过完整 target-aware OCR 反向测试，误报 0。
- “匕首→上首”5/5 拒绝，“匕首↔爪”9/9 拒绝，“杖→仗”2/2 拒绝。

合成测试用于发现陌生词义字符、长句和近邻模板问题，字体为 Microsoft JhengHei，数值、
正号和样式标点按生产归一化规则处理；它不能替代真实游戏字体、背景、分辨率与 UI 缩放。
每次遇到新布局仍应把原生游戏截图加入 manifest。

## 点击与命中提醒

繁中 OCR 即使通常足够快，也不能保证每一帧都赶在下一次点击前完成。为避免监控影响制作
手感，快速模式在识别未决期间不再安装或武装低级鼠标闸门；按钮和滚轮始终直接交给游戏，
不会出现点两三次才生效的情况。这也意味着继续高速点击时，可能在几十毫秒的识别窗口内
点过目标，程序不会假装提供绝对保护。

只有严格命中后，居中的红色确认窗才会显示并阻挡后续输入，直到用户手动确认。红窗必须先
通过可见、可点击和覆盖范围验证；显示失败会明确停止本次监控。确认后仍等待约 300 ms 且
所有鼠标键抬起，避免双击尾部落入游戏。

## 内置兼容资产

单文件 EXE 仍嵌入以下资产，用于局部复核、无 Windows 中文 OCR 时的兼容路径，以及
发布自检：

| 资产 | 大小 | SHA-256 |
|---|---:|---|
| `PP-OCRv5_mobile_rec.onnx` | 16,534,782 B | `DA72DC72CA4DC220DF0DFDE68C1DEDC31C58D3E76A25871122E5056227D50092` |
| `ppocrv5_dict.txt` | 74,012 B | `D1979E9F794C464C0D2E0B70A7FE14DD978E9DC644C0E71F14158CDF8342AF1B` |

隐藏入口 `--ocr-self-test <json-path>` 会验证嵌入资源哈希、18,383 项字典边界、ONNX
输入输出形状及一次真实推理。它只验证兼容资产，不验证 Windows 中文语言能力；正式
回归仍必须运行真实截图 manifest。

模型来源为 PaddlePaddle 官方
[PP-OCRv5 mobile recognition ONNX](https://huggingface.co/PaddlePaddle/PP-OCRv5_mobile_rec_onnx)，
许可归属见仓库根目录 `THIRD-PARTY-NOTICES.md`。

## 开发回归命令

```powershell
.\.tools\dotnet\dotnet.exe run --project tools\PoeAlarm.EndToEndProbe\PoeAlarm.EndToEndProbe.csproj -c Release --no-build -- --manifest tests\screenshots\8.11\traditional-ocr-8.11.json

.\.tools\dotnet\dotnet.exe run --project tools\PoeAlarm.EndToEndProbe\PoeAlarm.EndToEndProbe.csproj -c Release --no-build -- --legacy-manifest tests\screenshots\traditional-ocr-cases.json

.\.tools\dotnet\dotnet.exe run --project tools\PoeAlarm.UiSnapshot\PoeAlarm.UiSnapshot.csproj -c Release --no-build -- --poedb-corpus tests\screenshots\poedb-traditional-affix-corpus.json
```

三份 legacy manifest 都应运行；上面只展示其中一份命令。截图回归验证 Windows-first
生产路径，PoEDB 命令验证合成目标与近邻负例，`RecognizerProbe` 与 `TransientReplay` 则保留给
内置 Paddle 兼容路径的独立诊断。
