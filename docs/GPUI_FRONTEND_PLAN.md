# GPUI 前端施工计划(Ledger v1)

依据:`docs/design/project/POE Alarm Ledger.dc.html`(设计规范 v1,最终稿)与
`POE Alarm 界面方向.dc.html`(方向探索,仅作背景)。本计划取代
`RUST_NEXT_SESSION_PLAN.md` 中"以 Direct2D + DirectWrite 重做视觉层、不引入 GPUI"
的旧决定 —— 按用户 2026-08-14 的明确指示,本次改用 GPUI,并作为
"Rust 原生后端 + GPUI 前端"技术栈的一次正式验证。

## 0. 技术判断(为什么这样做)

### GPUI 现状(2026-08 核实)

- `gpui` 未在 crates.io 正式发布,标准引入方式是 git 依赖锁定
  `zed-industries/zed` 仓库的固定 commit;`Cargo.lock` 必须入库,保证可重现构建。
- Windows 后端(DirectX 11 + DirectWrite)自 Zed Windows 正式版后已是生产状态,
  CJK 文本渲染与 IME 有支持;但 GPUI API 本身不稳定,升级 rev 需按迁移对待。
- 组件基座(用户 2026-08-14 拍板):采用 `longbridge/gpui-component`
  (60+ 组件、明确支持 Windows 与 CJK,含成熟的文本输入/IME 处理)。
  它的 Input/Dropdown/Checkbox/树/虚拟化列表直接复用,主题系统用 Ledger
  token 全量覆盖(无阴影、0–2px 圆角、发丝线);设计中朴素的结构件
  (面板、树行视觉、日志、状态栏、警告条)用裸 GPUI div 自绘。
  gpui 的 rev 以 gpui-component 锁定的版本为准,二者一起 pin。

### 风险排序与对策

1. **中文 IME 文本输入**(最高风险)——词缀模板、条件名称都要输入/粘贴中文。
   对策:使用 gpui-component 的 Input(已含 IME 处理);Phase 1 仍做最小
   试验窗,用户实机验证 IME/DPI 后再全面施工(GO/NO-GO 门)。
   NO-GO 时回退旧 Direct2D 方案,损失只有一个试验 crate。
2. **GPUI 事件循环与现有 Win32 层共存**——全局热键、鼠标钩子、HUD、红色锁定卡
   目前运行在独立 Win32 线程/消息循环上。对策:保持它们原样(已实机验证的
   安全关键路径,尤其红色锁定卡),GPUI 只接管主窗口;跨线程用现有 channel 协议。
3. **DPI 与像素取整**——设计要求 scale_px 最近像素取整、发丝线 ≥1 物理像素。
   GPUI 内部用逻辑像素,自带 DPI 缩放;发丝线用 `px(1.)` 并在 96/120/144 实测。
4. **构建链**——用户 Windows 侧需 MSVC 工具链(已有);gpui 首次编译较慢。
   云端 Linux 可原生 cargo check/build GPUI 代码做快速迭代,Windows 行为
   由用户实机验证。

### 施工与验证分工

- 云端(Claude):写代码、Linux 侧 cargo check/fmt/clippy/test、逐文件写回本机磁盘。
- 本机(用户):git 提交推送、Windows 构建、实机验证(IME/DPI/热键/红窗)。
- git 操作全部在用户本机执行:云桥接对仓库 git 有锁文件权限限制,且 Linux 视图
  下有 99 个纯 CRLF 假差异,不允许从云端提交。

## 1. 分阶段计划

### Phase 0 — 本机快照与 .NET 清理(已完成;当时的一次性脚本已随迁移完成删除)

1. `git add -A` + commit:快照当前 rust 工作区与设计稿,push 到
   `origin codex/rust-native-migration`(首次 `-u`)。
2. `git rm` 移除 .NET 实现并二次提交推送:
   `src/`、`tools/`、C# 的 `tests/PoeAlarm.*` 六个工程、`PoeAlarm.slnx`、
   `global.json`、`Directory.Build.props`、`README-运行说明.txt`、
   `licenses/DotNet-*.txt`。
3. **保留**:`tests/fixtures/`、`tests/corpus/`(rust 测试依赖这些语料;`tests/screenshots/` 随 OCR 链路一同删除)、
   `RELEASE_NOTES_*.md`(历史)、ONNX/Paddle license(rust 仍在用)。
4. 删除本地未跟踪的 .NET 工具目录:`.dotnet-cli/.dotnet-home/.packages/.tools`。

### Phase 1 — GPUI 基线与 IME 试验(GO/NO-GO 门)

- 云端 clone 推送后的分支;新建 crate `rust/crates/poe-alarm-app`(GPUI 前端),
  旧 `poe-alarm-app-win` 暂保留作移植参照,收尾阶段退役。
- 锁定 zed rev(git 依赖)+ 提交 Cargo.lock;Linux 编译通过。
- 试验窗内容:一个 Ledger 配色面板 + 一个文本输入框 + 一个按钮。
- 用户实机验证清单:窗口渲染、微软雅黑/等宽字族、繁中/简中 IME 输入与候选框
  跟随、粘贴繁中词缀、120/144 DPI 缩放、窗口关闭干净退出。
- 通过 → 继续;不通过且无法在合理成本内修复 → 回退 Direct2D 旧方案。

### Phase 2 — Ledger 设计系统层

新模块 `theme`(token 全部来自规范,组件内禁止新色值):

- 表面:canvas `#F5F2EC` / panel `#FBF8F2` / rail `#F1EDE4` / well `#FFFDF9`
  / hover `#F1EDE4` / selected `#EFE9DE`(+2px 左边框)/ pressed `#E7E0D1`。
- 发丝线:soft `#E9E3D8` / normal `#D8D0C2` / strong `#CBC2B2`。
- 文字四级:`#1D1A15` / `#524C41` / `#6F6759` / `#A79E8E`。
- 状态三色:墨青 `#0E6A64`(文字 `#0B534E`,底 `#E3EEEB`,线 `#A6C9C4`)、
  琥珀 `#8A5A0B`(底 `#F6EEDD`)、砖红 `#A6382B`(文字 `#8C2F24`,
  底 `#F7E7E2`,线 `#E0B4A9`)。三色义务分离:青=运行、琥珀=注意、红=命中/错误。
- 字体:中文 Microsoft YaHei UI、英文 Segoe UI Variable Text;数据一律等宽
  (Cascadia Mono 优先,JetBrains Mono 兜底);字号阶
  10/10.5/11.5/12/12.5/13/15/20;字重仅 400/600;微标题 10px + .14em 字距。
- 间距阶 4/6/8/10/12/16/18/24;label 列固定 96;控件高八档
  22/24/26/28/30/32/40/46,不得新增。
- 圆角:面板/表格/输入 0,按钮/chip 2,状态点圆;焦点 = 内描 2px accent,
  不改变尺寸;hover 只改底色,禁止位移缩放。

基础组件(全部带键盘可达与焦点环;凡 gpui-component 有现成实现的
——Input/数值输入/Dropdown/Checkbox/树/滚动——一律复用并重皮,其余自绘):
按钮四类(Primary/Secondary/Quiet/Destructive)× 五态;文本输入(well 底,
唯一近白面)、数值输入(等宽,空值显示 `—`)、下拉、勾选、分段控件、热键 chip、
树行六态(active/selected/hover/default/warning/disabled,缩进 11/24/40,
行高 26,右侧等宽右对齐计数)、日志面板(10.5px 等宽,新行 120ms 淡入)、
状态栏 24px、琥珀警告条(2px 左边框,就地显示不弹窗)、错误行(占位常驻
一行高,出现不推挤)。

### Phase 3 — 三档窗口骨架与共享视图状态

- Workbench 1180×620:标题栏 30(面包屑 + 停止/开始 + 窗控)|
  左规则树栏 218 | 中编辑区(tab 条 30:词缀条件/识别区域/提醒与显示;
  条件名称行、完整词缀模板(左 2px accent 竖条)、归一化预览、数值条件表
  104/1fr/110/110/60、校验错误行、琥珀注意条)| 右运行栏 318
  (状态点+文字+计时、指标行 24、截图识别结果日志、验证并保存/撤销)|
  状态栏 24。
- Instrument 720×560:单条/多词缀分段、目标模板、词缀文字/识别区域/画面提示/
  提醒声音四行摘要、主操作 28 + 识别截图;右 264 运行栏。
- Dock 400×620:Ctrl+K 命令行/粘贴词缀、当前目标卡、六行 32 摘要、
  状态块、40px 主操作 + 两个 28 次操作。
- `Ctrl⇧1/2/3` 切档:同一窗口改尺寸与布局,共用一份视图状态实体,
  不重建控件、不丢输入(规范硬约束 3)。
- 动效白名单仅三处:状态点 1.6s 呼吸(1→0.45)、日志新行 120ms 淡入、
  模式切换 100ms 线性显隐;禁止 toast/骨架/进度条/数字滚动。
- 空间稳定:状态点+状态文字+计时坐标恒定;识别中不重排任何控件。
- OCR 状态机八态文案(idle/unchanged/scanning/recheck/no-blue/fallback/hit/
  cancelled)右栏与状态栏共用同一套。

### Phase 4 — 后端接线

- 设置:沿用 `poe-alarm-settings`(继续用 Rust 预览独立设置目录,
  不接管 .NET 正式配置,直至发布门禁达成)。
- 运行时:沿用 `poe-alarm-runtime` actor 协议 —— 开始/停止监控、逐带进度、
  局部复核计数、p50/p95 指标、命中事件、截图测试;GPUI 侧用
  `cx.spawn` + channel 桥接,UI 永不阻塞识别热路径。
- 保持 Win32(仅按 Ledger token 重配色,不改行为):
  状态浮窗(296 宽两态卡:冷灰未监控/墨青监控中,点击穿透)、
  红色命中锁定卡(2px `#A6382B` 边框、46px 确认键、300ms 尾击吸收)、
  F11 框选 overlay、全局热键、命中后鼠标闸门。
- 界面语言:沿用 `localization.rs` 的简中/English 双语文案。

### Phase 5 — 实机联调验证(用户)

96/120/144 DPI;繁中 IME 输入与粘贴;树键盘操作(↑↓←→ Space);
Tab 焦点顺序(目标→识别→提醒→主操作→右栏);三档切换不丢输入;
监控主链零回归:快速点击直通、命中红窗接管、Ctrl⇧F10/F11/F12。

### Phase 6 — 文档与收尾

README 移除 .NET 段落、记录 GPUI 架构与构建方式;退役 `poe-alarm-app-win`;
`cargo fmt/clippy/test` 全绿;对照规范"13 · 交付给工程的硬约束"逐条核对。

## 2. 明确不做

- 不动识别、匹配、监控引擎的任何行为;不以 UI 改造为由放宽任何识别严格度。
- 不在第一次 GPUI 尝试里重写红色锁定卡与鼠标钩子(安全关键路径)。
- 不引入 Tauri/WebView;不新增设计 token 之外的颜色、字号、控件高度。
