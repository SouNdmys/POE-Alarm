# POE1 / POE2 Trade Tracker 低延迟 OCR 改造计划

> 状态：待实施计划，不代表已经修改两款 Trade Tracker
> 编写日期：2026-08-12
> 目标版本：先完成离线验证，再分阶段替换现有 PaddleOCR 热路径

## 1. 结论

这项改造可行，但不能把它理解为“把 PaddleOCR 换成另一个通用 OCR 模型”。Trade Tracker 的画面结构、通货名称、比例、库存和比较符号都属于有限范围，最合适的方案是按字段拆分识别：

- 通货身份以图标和固定词库为主；
- 比较符号使用模板分类器；
- 比例只识别数字、分隔符和允许的格式；
- 库存使用独立的数字识别器；
- 通用 OCR 只作为固定词库文字的辅助证据或失败后的复核手段；
- 完整 PaddleOCR 先退出实时热路径，保留为影子对照和可回滚后备，证据充分后再决定是否从安装包移除。

计划目标是把一次已稳定截图从裁剪开始到生成可复核 Draft 的暖机端到端延迟控制在 **p95 小于 300 ms**，同时维持 fail-closed：不确定就重试或交给用户确认，不能为了快而猜值。

## 2. 当前证据与判断边界

本计划依据 POE1 Trade Tracker 现有本地实验文档：

- `ENGLISH_OCR_BENCHMARK.md`：recognition-only ONNX 单字段 p50/p95 为 5.64/7.41 ms，Windows OCR 为 2.40/10.33 ms，完整 PaddleOCR 为 117.58/394.04 ms；同一探索 sweep 分别约为 88、36 和 2067 ms。
- `OCR_TECH_DECISION_FINAL.md`：单一通用文字路径不足；原始字段中 ONNX 为 94/272、Paddle 为 185/272。字段专用方案已得到更好的结果：比较符号 p95 0.08 ms，比例 p95 161.55 ms，库存 p95 197.74 ms，且该 Pilot 没有 incorrect accept。
- POE Alarm 已证明“固定目标 + 小范围 ROI + 词库/语法约束 + 时序保护”适合低延迟识别，但不能直接把词缀匹配的成绩当成交易表格整行识别成绩。

这些数据足以支持启动工程改造，不足以支持现在就删除 PaddleOCR、承诺跨机器准确率或直接公开“全自动识别”。正式替换必须经过真实截图语料和冻结测试门槛。

## 3. 目标与非目标

### 3.1 目标

1. POE1 与 POE2 共用同一套识别内核、协议、字段类型、指标定义和测试工具。
2. 两款游戏分别使用独立的布局、语言、通货目录、图标模板和阈值 Profile，不互相继承坐标或经验补丁。
3. 暖机状态下，一次稳定画面的 OCR/解析端到端 p95 小于 300 ms。
4. 完整 PaddleOCR 不再阻塞正常实时监控，也不再决定关键字段的唯一结果。
5. 通货、比例和库存均保存原始证据、识别来源、Profile/模型版本以及接受或拒绝原因。
6. 保留现有 Draft 与人工确认边界；OCR 输出不能直接成为已确认行情。
7. 安装包不要求用户安装 Python 或 Paddle；语言组件缺失时仍有随包 fallback。

### 3.2 非目标

- 不自动点击游戏、选择通货、翻页或发送输入。
- 不把所有分辨率、DPI、UI 缩放和语言一次性声明为已支持。
- 不用模糊修正把 `O` 猜成 `0`、把相似名称强行映射到最近通货，或从其他字段反推缺失数值。
- 不在缺少真实语料时追求“任何画面都能识别”的通用 OCR。
- 不在第一阶段重写 Trade Tracker 的行情算法、数据库和 UI。
- 不把完整 Paddle 的移除作为第一阶段完成条件。

## 4. 总体架构

```text
游戏窗口捕获（各 Tracker 负责 exact HWND）
  -> Layout/Profile 与锚点校验
  -> 画面变化与稳定性门控
  -> 固定字段 ROI 裁剪
  -> trade-vision-worker（常驻、启动时预热）
       |- 通货图标分类 / 模板匹配
       |- 固定通货名称识别与目录约束
       |- 比较符号模板分类
       |- 比例数字与冒号分割识别
       |- 库存数字识别
       |- 字段证据融合与严格 parser
       `- 时序共识 / Retry / Review / Rejected
  -> 未确认 Draft
  -> 用户复核确认
  -> MarketSnapshot / QuoteEdge
```

### 4.1 共用内核

新增独立的 `trade-vision-core` 与常驻 `trade-vision-worker`：

- 核心建议使用 Rust 实现，避免 Python 冷启动和大型运行时；
- worker 通过版本化 JSON Lines 或命名管道协议接收已裁剪字段，进程在应用启动或首次进入捕获页时预热；
- POE1 可先通过 worker 接口接入，后续若有必要可直接链接 Rust crate；
- POE2 Electron 通过 Node 侧 supervisor 调用同一 worker，不要求维护一套独立 TypeScript OCR 算法；
- worker 崩溃、超时和重启沿用现有 generation/job cancellation 思路，旧任务结果不得覆盖新截图；
- 协议中必须携带 `game`、`profile_id`、`profile_hash`、`capture_revision` 和每个字段的 ROI/用途，禁止 worker 自行猜游戏。

不建议第一步就做 Node 原生 ABI 插件。常驻 worker 的 IPC 开销相对 300 ms 预算很小，进程隔离也更容易回滚和处理模型崩溃。

### 4.2 独立 Profile

共用内核不等于共用配置。至少拆成：

- `GameProfile`：POE1 / POE2；
- `LayoutProfile`：分辨率、DPI、窗口模式、游戏 UI 缩放、锚点、ROI；
- `LanguageProfile`：English / 繁体中文及字体、预处理参数；
- `CatalogProfile`：通货 ID、允许名称/别名、图标 hash/模板、版本；
- `RecognitionProfile`：每种字段的模型、阈值、parser 语法和证据融合规则。

Profile 必须版本化、可 hash 固定并写入每次识别证据。POE1 的坐标、字号、目录和阈值不能复制成 POE2 默认值，繁中也不能只作为英文 Profile 的翻译标签。

### 4.3 字段专用识别器

#### 通货身份

1. 图标模板/特征匹配作为主要证据；
2. 名称 ROI 只在当前游戏、当前语言的固定目录内识别；
3. Windows OCR 或轻量 recognition-only ONNX 作为文字证据，不接受开放字符串直接生成 `MarketAssetId`；
4. 图标与名称冲突时返回 `Rejected`，不得选置信度更高的一边；
5. 图标暂缺的新增通货保持 `manual_only`，通过目录发布流程升级。

这样可以让装有 zh-TW Windows OCR 的用户获得更好的原生识别，同时以随包模型/图标保证未安装语言组件时仍可用。两种环境可以做到功能路径一致，但必须分别测量延迟和准确率，不能假定结果完全相同。

#### 比较符号

- 延续 `comparator-template-fusion` 思路；
- 输出域只允许 `LessThan`、`GreaterThan`、`Empty`、`NeedsReview`、`Rejected`；
- 不再调用通用 OCR 识别 `<` / `>`。

#### 比例

- 先检测冒号位置，再独立识别左右数字；
- 字符集限制为数字、小数点、逗号和一个冒号；
- 用严格语法校验千位分组、小数格式、正数范围和字段完整性；
- 多个阈值候选只能作为证据家族，相关性很高的多个二值化结果不能伪装成独立共识；
- 不通过价格常识、另一行价格或库存反推缺失数字。

#### 库存

- 与比例使用不同的裁剪、阈值和模型；
- 只允许非负整数及严格千位分组；
- 空槽必须稳定输出 Empty/Review，不能识别成邻行数字；
- 对表格行边界和上下行粘连做专门负例。

#### 整行组装

- 每行字段必须来自同一 capture revision；
- 行号、Available/Competing 侧和 ROI 不能通过 OCR 文本推断；
- 关键字段任一 Review/Rejected 时，整行不能自动 Accepted；
- 保存每个字段的原图 crop hash、原始输出、归一化结果、证据来源和耗时。

### 4.4 时序与变化检测

- 只在目标 ROI 发生有效变化后启动字段识别；
- 相邻帧先做低成本指纹/像素差，跳过完全相同画面；
- 刷新动画、鼠标遮挡或表格滚动尚未稳定时返回 Retry；
- 识别任务并行处理图标、比例和库存，最终按 capture revision 汇合；
- 不为了满足 300 ms 而删除稳定性门控，应优化帧间隔和只比较必要 ROI。

## 5. 性能预算

性能口径从“稳定画面已取得”开始，到生成结构化 Draft 为止；游戏刷新等待时间单独记录，不能混入 OCR 数字掩盖，也不能拿单字段模型推理时间代替端到端时间。

| 环节 | 暖机 p95 预算 | 说明 |
|---|---:|---|
| ROI 裁剪、缩放、颜色转换 | 20 ms | 一次转换复用给多个字段 |
| 锚点/状态与变化门控 | 20 ms | 重帧直接跳过 |
| 通货图标 + 名称证据 | 60 ms | 两侧并行 |
| 比较符号 | 2 ms | 模板路径 |
| 全部比例字段 | 180 ms | 批量/并行，不按行串行累加 |
| 全部库存字段 | 220 ms | 批量/并行，与比例同时执行 |
| 融合、parser、序列化 | 15 ms | 纯 CPU |
| IPC、Draft 更新与持久化排队 | 30 ms | 持久化不得阻塞识别结果显示 |
| 端到端关键路径 | **<300 ms** | p95，暖机、受支持 Profile |

表中子项并行，不能简单相加。附加门槛：

- p50 目标小于 180 ms；
- p99 目标小于 450 ms；
- 启动时完成预热，正常捕获不得付出模型冷启动；
- worker 冷启动单独记录，目标小于 1.5 s，失败时 UI 明确显示未就绪；
- 连续 30 分钟监控不得出现无界内存增长、worker 堆积或旧任务回写；
- CPU、private working set、安装包体积都要与当前 Paddle 版本做前后对照，不只汇报延迟。

## 6. 分阶段实施

### Phase 0：冻结现状与仪表

交付：

- 固定当前 POE1/POE2 可工作的版本和回滚 tag；
- 在现有 Paddle 路径记录 capture、crop、初始化、推理、解析、Draft 总耗时；
- 统一错误分类：错位、动画帧、字符错、行粘连、目录冲突、数字格式错、超时；
- 建立不包含隐私信息的 fixture manifest 和 hash。

通过条件：同一批截图可重复跑现有实现，报告字段级和端到端基线。

### Phase 1：真实语料与 Profile

分别建立 POE1/POE2 × English/繁中的语料，不混用分母：

- Development：用于调参；
- Validation：调参过程中只做阶段复核；
- Frozen：阈值锁定后才生成/读取；
- 必须包含正常、刷新中、鼠标遮挡、库存空值、极端长数字、相似通货、图标/名称冲突、界面错位和错误页面。

最低公开发布证据沿用现有严格门槛：至少 800 张真实截图、4,000 个关键字段、3,000 个 reviewed parser-accepted 关键字段，并有至少 150 个困难/负例/模糊样本。POE1 与 POE2 不能互相凑数。

### Phase 2：共用 worker 与协议骨架

- 创建 `trade-vision-core`、`trade-vision-worker` 和 schema；
- 实现启动预热、批量 ROI、取消、超时、崩溃恢复、版本/模型 hash；
- 先使用 mock/fixture 输出验证两款 Tracker 的接线；
- 保持产品默认仍走 Paddle，worker 只运行离线或影子模式。

通过条件：并发任务、取消、超时、重启、旧 revision 丢弃测试全部通过，且不改变现有保存语义。

### Phase 3：字段识别器离线完成

建议顺序：比较符号 → 通货图标 → 比例 → 库存 → 名称辅助 → 整行融合。每个字段先达到自己的准确率和拒绝门槛，再接入下一字段；不得用整行看似正确掩盖局部错误。

通过条件：Validation 上没有 silent incorrect accept，延迟满足字段预算，并输出可审计的失败原因。

### Phase 4：POE2 影子接入

POE2 当前完整 Paddle worker 边界最清晰，先让新 worker 与 Paddle 对同一截图并行运行：

- UI 和数据库仍采用 Paddle/人工确认结果；
- 记录两条路径的字段差异、延迟和 Review 原因；
- 不允许新路径影响行情和算法；
- 修完 POE2 独有布局/Profile 后再进入可选 Beta。

通过条件：真实日常使用的差异报告可解释，端到端 p95 <300 ms，连续运行无任务堆积。

### Phase 5：POE2 可回滚 Beta

- 设置隐藏 feature flag：`field_pipeline` / `paddle_legacy`；
- 新路径生成 Draft，Paddle 仅在 Review 或用户主动诊断时运行；
- 任何 profile 缺失、worker 故障或指标越线可在一次重启内回退；
- 保留人工确认，禁止自动提交。

通过条件：冻结测试通过，且一段规定时长的真实 Beta 没有 incorrect accept 或数据损坏。

### Phase 6：POE1 接入

- 复用共用 worker、字段协议和测试框架；
- 把 POE1 已验证的字段专用规则迁入共用内核，而不是退回通用 OCR；
- 新建 POE1 独立 Profile、目录和真实语料；
- 保持 POE1 现有 Rust Draft/provenance/capture contract 不变。

通过条件与 POE2 相同，但必须独立达标；POE2 的成绩不能作为 POE1 发布凭证。

### Phase 7：Paddle 降级或移除

按证据分三步：

1. 从默认热路径退出，作为 Review 后备；
2. 从正常用户流程退出，只保留开发者离线对照；
3. 确认公开版本的 frozen、跨环境和回滚门槛后，才从安装包移除 Python/Paddle runtime。

每一步都单独比较安装包大小、启动时间、内存、识别覆盖和回滚成本。不能因为新路径“感觉更快”就直接删除旧实现。

## 7. 测试与发布门槛

### 7.1 正确性

- 通货身份：图标与目录组合后的 Accepted 结果必须 exact；冲突必须 Rejected。
- 比例/库存：按字符串语义 exact，不允许近似数值正确。
- 整行：所有关键字段和所属行 exact 才算整行正确。
- reviewed Accepted 关键字段 incorrect accept 必须为 0，并报告 `0/N` 的单侧 95% 上界；不能只写“准确率 100%”。
- 刷新中、错位和错误画面必须 Retry/Rejected，不能形成 Draft Accepted 行。
- Development、Validation、Frozen 不得包含同图裁剪、近重复帧或泄漏调参数据。

### 7.2 性能

- 每个 Game/Language/Layout Profile 分别报告 p50/p95/p99/max；
- 暖机端到端 p95 <300 ms；
- 30 分钟与 2 小时 soak 均无请求泄漏、内存持续增长、僵尸 worker；
- 快速连续刷新时只保留最新 revision，队列长度有上限；
- 低配置 CPU 和无 zh-TW OCR 环境必须单独测试。

### 7.3 产品集成

- 未确认 OCR 永远只是 Draft；
- 取消、重启、窗口切换、Profile 变化不会保存旧结果；
- 诊断日志脱敏，不保存完整用户截图，除非用户明确选择测试采集；
- 安装、卸载、升级和回滚不破坏已有行情数据库；
- POE2 Electron 必须运行相关 OCR/capture smoke、`npm run build` 和 `npm run electron:build`；
- POE1 必须运行 workspace Rust tests、capture-to-draft 回放和现有 frozen gate。

## 8. 风险与应对

| 风险 | 表现 | 应对 |
|---|---|---|
| 固定 Profile 过拟合 | 换分辨率/UI 缩放即错位 | 未注册 Profile 直接拒绝；按真实矩阵逐项开放 |
| 繁中字体/组件差异 | Windows OCR 机器间表现不同 | 图标为主，随包 recognition-only fallback；分别报告环境 |
| 数字识别快速但误报 | 比例/库存出现合理的错误数 | 严格字符语法、独立证据、0 incorrect-accept gate |
| 邻行粘连 | 库存或比例串到上一/下一行 | 行锚点、窄 ROI、空槽负例、整行一致性校验 |
| 时序竞争 | 新截图被旧 OCR 结果覆盖 | generation + capture revision + bounded queue |
| 共用内核耦合两款游戏 | 修 POE2 导致 POE1 回归 | profile 隔离、game-scoped corpus、双套 frozen gate |
| worker/模型打包失败 | 开发机能跑，用户机缺文件 | 自检、资源 hash、干净 Windows 打包 smoke |
| Paddle 过早移除 | 新增通货或边界画面无后备 | 分阶段退出、feature flag、旧安装包与回滚 tag |
| 只优化模型忽略端到端 | 单字段 5 ms，用户仍感到 2 s | 全链路时间戳，预算按 wall-clock Draft 完成计算 |

## 9. 回滚方案

- 在改造前为两款 Tracker 建立可安装的基线 release/tag；
- provider 选择由本地 feature flag 控制，不迁移或改写用户原始截图/行情数据；
- 新旧 provider 输出使用相同 Draft contract，回滚不需要数据库降级；
- Profile、模型和目录均带版本/hash，发现回归可仅回滚资源包；
- worker 连续超时、崩溃、hash 不符或 Profile 不支持时自动停用新路径并提示用户，不应静默切换后继续自动接受结果；
- Paddle 从安装包移除前至少保留一个完整发布周期的 legacy 构建和恢复说明。

## 10. 交付物

1. `trade-vision-core` 源码、字段 parser 与测试。
2. `trade-vision-worker` 常驻可执行文件、版本化协议和 supervisor 示例。
3. POE1/POE2 独立的 Game/Layout/Language/Catalog/Recognition Profile。
4. 通货目录、图标模板和资源 hash 清单。
5. Development/Validation/Frozen fixture manifest、标注工具和去重审计。
6. 字段级、整行和端到端 benchmark 报告，包含准确率、拒绝率、incorrect accept、延迟、CPU、内存与包体。
7. POE2 Electron 接入、POE1 Rust 接入及各自的回归测试。
8. feature flag、故障自检、诊断和回滚说明。
9. Paddle 影子对照报告及“保留/移除 runtime”的最终 ADR。
10. 面向用户的支持矩阵：游戏、语言、分辨率、DPI、UI 缩放、是否建议安装 zh-TW OCR。

## 11. 开工时的第一批任务

正式启动时建议只授权 Phase 0–2，不直接改默认识别：

1. 冻结两个 Tracker 的当前可用版本；
2. 从现有真实截图建立严格分组、去重的字段语料；
3. 把当前完整 Paddle 的全链路耗时和资源占用测清；
4. 确定共用 worker 协议与 Profile schema；
5. 用 mock worker 打通 POE1/POE2 影子接线；
6. 从比较符号和图标识别开始做第一个离线字段 gate。

做到这一步后再决定后续实现顺序，可以避免再次陷入“不断换 OCR 模型，但没有改变识别问题定义”的循环。
