# Mirror Tier 复合规则真实语料验证

验证日期：2026-08-13

## 结论

现有 `RuleSet` 确实能够表达并正确计算本项目当前讨论的核心做装场景：

- 多个“值得停手结果”之间为 OR；
- 本 Mirror fixture 的结果组覆盖 `All` 与 `AtLeast N`；规则引擎的 `Any` 语义由 `tests/PoeAlarm.Rules.Tests` 单独覆盖；
- 可表达“4 个候选词缀至少命中 2 个”；
- 每个数值槽能独立做闭区间、下限或精确值约束；
- 一条实际 modifier 不能因文本重叠或 Hybrid 的两行显示而重复计数；
- 近邻词缀（全域 / 法术、攻击技能 / 近战技能等）保持严格区分。

这项验证主要证明：给定正确 OCR 文本时，规则引擎能否得出正确结果。Hybrid 专门案例还会构造 `OcrRecognitionResult`，经过与生产监控相同的 Assisted/Physical identity 投影验证跨层去重契约；它并未运行 OCR 模型，因此仍不等价于“所有游戏分辨率、语言和词缀颜色下 OCR 都已验证”。多目标局部复核与 Guarded 策略应继续使用截图回放和实机测试证明识别层能力。

## 数据与复现

可复现 fixture：

- `tests/fixtures/mirror-tier-composite-rules.json`

无第三方测试框架的探针：

- `tests/PoeAlarm.MirrorCorpusProbe`

运行：

```powershell
dotnet run --project tests\PoeAlarm.MirrorCorpusProbe\PoeAlarm.MirrorCorpusProbe.csproj -c Release --no-build
```

当前语料包含 14 条来源记录、7 个规则场景、36 个断言案例；其中 POE1 三个、POE2 三个场景来自官网交易论坛实际 Mirror Service 成品，另一个 POE1 珠宝场景直接来自当前 PoEDB 词缀池，用于覆盖用户提出的“四选二”玩法。每个来源在 fixture 中保存 URL、访问日期、游戏、装备槽及其证明内容。

案例覆盖：

- 15 个正例；
- 21 个负例；
- 7 个近邻词缀负例；
- 8 个少一条负例；
- 13 个数值边界案例；
- 2 个直接使用普通非 Alt tooltip 显示值的真实镜装正例；
- 1 个 Hybrid 同一 modifier 不重复计数的 `OcrRecognitionResult` 跨层身份契约案例。

## 来源方法

词缀本体和当前区间优先以数据库页面为准：

- POE1 [One Hand Swords](https://poedb.tw/us/One_Hand_Swords)、[Wands](https://poedb.tw/us/Wands)、[Sceptres](https://poedb.tw/us/Sceptres)、[Cobalt Jewel](https://poedb.tw/us/Cobalt_Jewel)；
- POE2 [Quarterstaves](https://poe2db.tw/us/Quarterstaves)、[Two Hand Maces](https://poe2db.tw/us/Two_Hand_Maces)、[Greater Essence of Haste](https://poe2db.tw/Greater_Essence_of_Haste)、[Wands](https://poe2db.tw/us/Wands)。

实际高端组合以 GGG 官方论坛的 Mirror Service 物品文本为准：

- POE1 [Phoenix Needle 物理剑](https://www.pathofexile.com/forum/view-thread/2196247)：178% 物理、24–40 点伤、27% 攻速、27% 暴击等组合；
- POE1 [Wrath Edge 法术法杖](https://www.pathofexile.com/forum/view-thread/3924610)：全域暴击加成、全法术/物理法术等级、物理转额外冰冷等组合；
- POE1 [Golem Bane / Blight Braid](https://www.pathofexile.com/forum/view-thread/3527883)：用于交叉确认法术伤害、法术暴击、宝石等级、施法速度和穿透确实属于实际镜装组合；
- POE2 [Razor Quarterstaff](https://www.pathofexile.com/forum/view-thread/3868726)：纯物理、物理/命中 Hybrid、点伤、攻速、近战技能等级、猛攻概率；
- POE2 [Cataclysm Thresher](https://www.pathofexile.com/forum/view-thread/3865567)：点伤（破裂）、纯物理、命中、攻击技能等级、猛攻与亵渎攻速；
- POE2 [Havoc Call Lightning Wand](https://www.pathofexile.com/forum/view-thread/3843434)：闪电伤害、额外闪电、法术伤害、闪电法术等级、法术暴伤和施法速度。

论坛物品用于确认实际组合，数据库用于确认词缀模板/范围；不能把论坛中一个历史镜装的具体数值当作永远不变的当前 T1 定义。

## 对之前场景的适用性判断

### 可以直接应用

“只要命中任一可接受结果便报警”与“4 条里至少中 2 条”都能直接应用。比如同一套珠宝规则可以把：

1. 全域暴击率；
2. 法术暴击率；
3. 法术暴击加成；
4. 攻击速度；

设成 `AtLeast 2`，并再加一个 `All` 分支表达必须同时出现的法术暴击组合。引擎会保留各分支证据；若多个分支同时成立，按配置顺序选择第一个作为主命中结果。

武器的阶段性做装同样能表达：一个 OR 分支监视前缀包（纯物理 + Hybrid + 点伤），另一个分支监视后缀/完工条件（攻速 + 技能等级）。数值边界用当前屏幕显示值判断。

### 必须明确限制

1. **破裂来源不可判。** 非 Alt tooltip 只改变文字颜色，不包含 `fractured` 字样；程序可以判断“这条可见词缀和值还在”，不能承诺它就是破裂词缀。fixture 因此只在 `unobservableFacts` 记录破裂/亵渎来源，绝不把它做成条件。
2. **Hybrid 必须按完整 modifier 建模。** `78% increased Physical Damage` 与 `+192 Accuracy Rating` 来自同一个 Hybrid modifier 时，应建立一个两行条件。不能拆成两个独立条件去凑 `AtLeast 2`。专门负例同时提供同一物理 band 的主识别与局部复核候选：保留 identity 时只能贡献一次；探针还验证去掉主识别 identity 的反事实会错误命中。这里证明的是识别结果到规则引擎的身份契约，不是实机 OCR 精度。
3. **品质、催化剂、符文、附魔等只影响显示值。** 当前规则可比较最终显示值，却无法反推出未经这些效果修正的底层 tier。界面应继续提醒用户按最终 tooltip 值配置。
4. **前缀/后缀、tier 名称与已有词缀身份不可由普通 tooltip 可靠恢复。** Guarded 策略若要求“已有破裂词缀仍存在”，只能保护其可见完整文本与数值，不能证明词缀来源/槽位身份。

## 对 Guarded 的测试建议

真实语料支持把安全策略建立在“规则证据变化”上，而不是重新发明词缀匹配：

- `TargetReached`：任一可接受结果稳定命中；
- `ProtectedVisibleModifierLost`：用户显式选定的可见完整 modifier 在确认帧消失；
- `AmbiguousRecognition`：局部复核仍不能确定目标/保护项；
- `ChangedButNoTarget`：物品确实变化但未命中规则，可继续点击；
- 所有停止原因都要经过变化帧确认和冷却，不能因单帧 OCR 噪声触发。

“保护项”必须由用户用完整可见词缀模板明确选择，不能自动把颜色识别成破裂，也不能把所有起始词缀都隐式锁定。
