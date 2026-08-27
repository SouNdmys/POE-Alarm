//! Phase 4:GPUI 前端 ↔ 生产运行时桥接。
//!
//! Windows 上使用真实 `poe-alarm-runtime`(读取物品文本、规则判定、红色拦截窗
//! 与提示音都由 runtime 内部完成;UI 只发命令、收事件)。非 Windows 平台
//! 提供模拟器,保证云端 Linux 开发环境可编译、可迭代布局。
//!
//! 设置使用正式路径(`SettingsStore::release_default`,
//! `%LOCALAPPDATA%/PoeAlarm/settings.json`);首次启动会把 Rust 预览目录的
//! 设置一次性迁移过来,旧 .NET 设置自动留档。

// 非 Windows 平台下部分事件变体只由 Windows 路径构造。
#![allow(dead_code)]

use std::sync::mpsc::{Receiver, Sender, channel};

use poe_alarm_settings::{AppSettings, SettingsStore};

/// 平台事件(全局热键 / 浮窗拖动),由后台线程投递、UI 轮询消费。
#[derive(Clone, Debug)]
pub enum PlatformEvent {
    HotKeyStart,
    /// 光标下物品的 Ctrl+C 原文,已在热键线程上取好。
    ItemUnderCursor(String),
    /// 用户在提权弹窗里点了「是」。
    ElevationConfirmed,
    /// 提权副本已经起来了,这个实例可以退出。
    ElevationSucceeded,
    /// 提权失败。`detail` 是技术原因(含子进程退出码),给日志用。
    ElevationFailed {
        declined: bool,
        detail: String,
    },
    /// 全局热键一个都没注册上(被别的软件占用)。
    ///
    /// 注册是全有全无的,所以这条一旦出现就意味着 F10/F11 都不会响应。
    /// 以前它只往 stderr 打一行,而发布版是 GUI 子系统、没有控制台。
    HotKeysUnavailable,
    /// 游戏没在前台,热键按早了。
    GameNotFocused,
    /// 复制到了东西,但光标下不是一件物品。
    NoItemUnderCursor,
    /// 游戏在前台,但没有回应复制键。多半是权限不匹配。
    CopyRefused {
        outranked: bool,
    },
    /// HUD 拖动结束:工作区内的相对坐标(0..=1),供写回设置。
    HudMoved(f64, f64),
}

/// 运行时状态镜像(非 Windows 平台没有 runtime crate,因此桥接层自带一份)。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BridgeState {
    Idle,
    Starting,
    Monitoring,
    CheckingItem,
    MatchFound,
    Faulted,
    ShuttingDown,
    Stopped,
}

#[cfg(windows)]
impl From<poe_alarm_runtime::RuntimeState> for BridgeState {
    fn from(state: poe_alarm_runtime::RuntimeState) -> Self {
        use poe_alarm_runtime::RuntimeState as R;
        match state {
            R::Idle => Self::Idle,
            R::Starting => Self::Starting,
            R::Monitoring => Self::Monitoring,
            R::CheckingItem => Self::CheckingItem,
            R::MatchFound => Self::MatchFound,
            R::Faulted => Self::Faulted,
            R::ShuttingDown => Self::ShuttingDown,
            R::Stopped => Self::Stopped,
        }
    }
}

/// 桥接层向 UI 报告的事件(已翻译为 UI 语义)。
#[derive(Clone, Debug)]
pub enum BridgeEvent {
    State(BridgeState),
    /// 监控快照:轮询轮数 / 读取耗时 ms / 判定耗时 ms / 是否未变化 / 最近行
    Snapshot {
        scan_count: u64,
        capture_ms: f64,
        ocr_ms: f64,
        cached: bool,
        lines: Vec<String>,
        detail: Option<String>,
    },
    MatchFound {
        detail: String,
        lines: Vec<String>,
        matched_group: Option<String>,
    },
    ItemCheckReport {
        lines: Vec<String>,
        matched: bool,
        detail: String,
    },
    AlertPresented,
    AlertAcknowledged,
    Fault {
        detail: String,
        /// The user can fix this; the UI must interrupt rather than log.
        actionable: bool,
    },
    SoundFallback,
}

pub struct Backend {
    store: SettingsStore,
    pub settings: AppSettings,
    pub read_only: bool,
    platform_rx: Receiver<PlatformEvent>,
    platform_tx: Sender<PlatformEvent>,
    #[cfg(windows)]
    hud: Option<crate::hud_service::HudService>,
    #[cfg(windows)]
    hotkey_thread: Option<u32>,
    #[cfg(windows)]
    runtime: Option<poe_alarm_runtime::RuntimeHandle>,
    #[cfg(windows)]
    sound_fell_back: bool,
    #[cfg(not(windows))]
    sim: sim::Sim,
}

impl Backend {
    pub fn new() -> Result<Self, String> {
        let mut store =
            SettingsStore::release_default().map_err(|e| format!("settings path: {e}"))?;
        let settings = store.load();
        let read_only = store.is_read_only();
        let (platform_tx, platform_rx) = channel();
        #[cfg(windows)]
        let hotkey_thread = spawn_hotkey_thread(
            platform_tx.clone(),
            settings.start_monitoring_hot_key.clone(),
        );
        #[cfg(windows)]
        let hud = Some(crate::hud_service::HudService::start(
            settings.allow_overlay_capture,
            settings.keep_hud_visible,
            settings.hud_placement.clone(),
            if settings.ui_language.starts_with("en") {
                crate::i18n::EN.hud_idle
            } else {
                crate::i18n::ZH.hud_idle
            }
            .to_owned(),
            platform_tx.clone(),
        ));
        Ok(Self {
            store,
            settings,
            read_only,
            platform_rx,
            platform_tx,
            #[cfg(windows)]
            hud,
            #[cfg(windows)]
            hotkey_thread,
            #[cfg(windows)]
            runtime: None,
            #[cfg(windows)]
            sound_fell_back: false,
            #[cfg(not(windows))]
            sim: sim::Sim::default(),
        })
    }

    pub fn settings_path(&self) -> String {
        self.store.path().display().to_string()
    }

    /// 排空平台事件(热键)。
    /// Puts the elevation question in front of the user and reports the answer.
    ///
    /// On its own thread: MessageBoxW runs a nested modal message loop, and
    /// running one inside a GPUI frame would block the renderer for as long as
    /// the box is up. The answer comes back through the same channel the
    /// hotkey thread uses.
    #[cfg(windows)]
    pub fn ask_to_relaunch_elevated(&self, title: String, body: String) {
        let tx = self.platform_tx.clone();
        std::thread::spawn(move || {
            if poe_alarm_platform_win::confirm_relaunch_elevated(&title, &body) {
                let _ = tx.send(PlatformEvent::ElevationConfirmed);
            }
        });
    }

    #[cfg(not(windows))]
    pub fn ask_to_relaunch_elevated(&self, _title: String, _body: String) {}

    /// Hands the global hotkeys back before an elevated copy tries to take them.
    ///
    /// Registration is all-or-nothing, so a new instance starting while this one
    /// still holds Ctrl+Shift+F10/F11/F12 comes up with none of them. Ending the
    /// hotkey thread's message loop drops its HotKeyManager, whose Drop
    /// unregisters.
    #[cfg(windows)]
    pub fn release_hotkeys(&mut self) {
        use windows::Win32::Foundation::{LPARAM, WPARAM};
        use windows::Win32::UI::WindowsAndMessaging::{PostThreadMessageW, WM_QUIT};

        if let Some(thread) = self.hotkey_thread.take() {
            // SAFETY: posting to a thread this process owns.
            let _ = unsafe { PostThreadMessageW(thread, WM_QUIT, WPARAM(0), LPARAM(0)) };
        }
    }

    /// Relaunches elevated on a background thread and reports what happened.
    ///
    /// Off the UI thread because it waits on the new process, and reporting
    /// rather than assuming because "the process was created" is not "the
    /// process is running" — the caller must not exit until the replacement has
    /// survived its own startup.
    #[cfg(windows)]
    pub fn begin_relaunch_elevated(&mut self) {
        use poe_alarm_platform_win::ElevateError;

        self.release_hotkeys();
        let tx = self.platform_tx.clone();
        std::thread::spawn(move || {
            let settle = std::time::Duration::from_secs(4);
            let event = match poe_alarm_platform_win::relaunch_elevated(&[], settle) {
                Ok(()) => PlatformEvent::ElevationSucceeded,
                Err(error @ ElevateError::Declined) => PlatformEvent::ElevationFailed {
                    declined: true,
                    detail: error.to_string(),
                },
                // The exit code is the only thing that identifies why a child
                // died during startup, and stderr goes nowhere in a
                // GUI-subsystem binary. It has to reach the log.
                Err(error) => PlatformEvent::ElevationFailed {
                    declined: false,
                    detail: error.to_string(),
                },
            };
            let _ = tx.send(event);
        });
    }

    #[cfg(not(windows))]
    pub fn begin_relaunch_elevated(&mut self) {}

    pub fn poll_platform(&mut self) -> Vec<PlatformEvent> {
        let mut out = Vec::new();
        while let Ok(event) = self.platform_rx.try_recv() {
            out.push(event);
        }
        out
    }

    pub fn set_game(&mut self, profile: poe_alarm_settings::GameProfile) {
        self.settings.selected_game_profile = profile;
    }

    pub fn set_ocr_language(&mut self, language: &str) {
        self.settings.selected_profile_mut().ocr_language =
            poe_alarm_settings::normalize_ocr_language(language).to_owned();
    }

    pub fn ocr_language_label(&self) -> String {
        self.settings.selected_profile().ocr_language.clone()
    }

    /// 自定义提示音文件名;使用内置音效时为 None(占位文案由界面按语言提供)。
    pub fn sound_label(&self) -> Option<String> {
        self.settings
            .custom_alert_sound_path
            .as_deref()
            .map(|path| {
                std::path::Path::new(path)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.to_owned())
            })
    }

    /// 更新状态浮窗内容(非 Windows 为空操作)。
    #[cfg(windows)]
    pub fn hud_update(&self, monitoring: bool, status_text: &str, elapsed: &str, target: &str) {
        if let Some(hud) = &self.hud {
            hud.update(crate::hud_service::HudContent {
                monitoring,
                status_text: status_text.to_owned(),
                elapsed: elapsed.to_owned(),
                target: target.to_owned(),
            });
        }
    }

    #[cfg(not(windows))]
    pub fn hud_update(&self, _monitoring: bool, _status_text: &str, _elapsed: &str, _target: &str) {
    }

    /// 状态浮窗显隐:跟随"持续显示"设置,命中弹窗期间让位隐藏。
    #[cfg(windows)]
    pub fn hud_set_visible(&self, visible: bool) {
        if let Some(hud) = &self.hud {
            hud.set_visible(visible);
        }
    }

    #[cfg(not(windows))]
    pub fn hud_set_visible(&self, _visible: bool) {}

    pub fn hotkey_label(&self) -> String {
        self.settings.start_monitoring_hot_key.clone()
    }

    /// 当前启动热键在三个可选项中的下标。
    #[cfg(windows)]
    pub fn start_hotkey_index(&self) -> usize {
        use poe_alarm_platform_win::StartMonitoringHotKey;
        let current =
            StartMonitoringHotKey::parse_or_default(Some(&self.settings.start_monitoring_hot_key));
        StartMonitoringHotKey::OPTIONS
            .iter()
            .position(|option| *option == current)
            .unwrap_or(0)
    }

    #[cfg(not(windows))]
    pub fn start_hotkey_index(&self) -> usize {
        0
    }

    /// 切换启动热键(三选一):写设置并即时通知热键线程重注册。
    #[cfg(windows)]
    pub fn set_start_hotkey_index(&mut self, index: usize) {
        use poe_alarm_platform_win::StartMonitoringHotKey;
        let Some(option) = StartMonitoringHotKey::OPTIONS.get(index).copied() else {
            return;
        };
        self.settings.start_monitoring_hot_key = option.setting_value().to_owned();
        if let Some(thread) = self.hotkey_thread {
            use windows::Win32::Foundation::{LPARAM, WPARAM};
            use windows::Win32::UI::WindowsAndMessaging::PostThreadMessageW;
            // SAFETY: 向自有热键线程投递重配消息;线程随进程存活。
            let _ = unsafe {
                PostThreadMessageW(thread, WM_APP_SET_START_HOTKEY, WPARAM(index), LPARAM(0))
            };
        }
    }

    #[cfg(not(windows))]
    pub fn set_start_hotkey_index(&mut self, _index: usize) {}

    /// 状态浮窗交互:未监控时可拖动(Placement),监控中点击穿透(Passive)。
    #[cfg(windows)]
    pub fn hud_set_interactive(&self, interactive: bool) {
        if let Some(hud) = &self.hud {
            hud.set_interactive(interactive);
        }
    }

    #[cfg(not(windows))]
    pub fn hud_set_interactive(&self, _interactive: bool) {}

    /// 状态浮窗的录屏可见性(即时生效)。
    #[cfg(windows)]
    pub fn hud_set_capture(&self, allow: bool) {
        if let Some(hud) = &self.hud {
            hud.set_capture(allow);
        }
    }

    #[cfg(not(windows))]
    pub fn hud_set_capture(&self, _allow: bool) {}

    pub fn save(&mut self) -> Result<(), String> {
        let normalized = self.settings.clone().normalize();
        self.settings = normalized.clone();
        self.store.save(&normalized).map_err(|e| e.to_string())
    }

    /// 故障后重建:丢弃运行时句柄(触发其内部 shutdown),下次启动时全新创建。
    /// 用于清理 BitBlt 超时等一次性系统故障留下的状态。
    #[cfg(windows)]
    pub fn reset_runtime(&mut self) {
        self.runtime = None;
    }

    #[cfg(not(windows))]
    pub fn reset_runtime(&mut self) {}

    // ------------------------------------------------------------------
    // Windows:真实运行时
    // ------------------------------------------------------------------

    #[cfg(windows)]
    fn ensure_runtime(&mut self) -> Result<(), String> {
        if self.runtime.is_some() {
            return Ok(());
        }
        // 旧运行时刚被丢弃时,其警报服务线程可能尚未释放进程单例
        // (AlreadyInUse);短暂等待重试而不是直接报错。
        let mut attempt = 0u8;
        let (handle, fell_back) = loop {
            let (wave, fell_back) = resolve_alert_wave(&self.settings)?;
            let mut alert = poe_alarm_alert_win::AlertServiceConfig::new(wave);
            alert.allow_overlay_capture = self.settings.allow_overlay_capture;
            match poe_alarm_runtime::RuntimeHandle::start_production(
                poe_alarm_runtime::ProductionRuntimeConfig { alert },
            ) {
                Ok(handle) => break (handle, fell_back),
                Err(e) => {
                    let text = e.to_string();
                    if text.contains("AlreadyInUse") && attempt < 6 {
                        attempt += 1;
                        std::thread::sleep(std::time::Duration::from_millis(300));
                        continue;
                    }
                    return Err(format!("runtime startup failed: {text}"));
                }
            }
        };
        self.runtime = Some(handle);
        self.sound_fell_back = fell_back;
        Ok(())
    }

    #[cfg(windows)]
    pub fn start_monitoring(&mut self) -> Result<(), String> {
        self.ensure_runtime()?;
        let settings = self.settings.clone().normalize();
        self.runtime
            .as_ref()
            .expect("runtime just ensured")
            .start(settings)
            .map_err(|e| e.to_string())
    }

    #[cfg(windows)]
    pub fn stop_monitoring(&mut self) -> Result<(), String> {
        match &self.runtime {
            Some(r) => r.stop().map_err(|e| e.to_string()),
            None => Ok(()),
        }
    }

    #[cfg(windows)]
    pub fn acknowledge_alert(&mut self) -> Result<(), String> {
        match &self.runtime {
            Some(r) => r.acknowledge_alert().map_err(|e| e.to_string()),
            None => Ok(()),
        }
    }

    /// 检查物品文本:用当前规则跑一次判定。
    ///
    /// 入参是玩家在游戏里 Ctrl+C 复制出来的原文。
    #[cfg(windows)]
    pub fn check_item(&mut self, text: String) -> Result<(), String> {
        self.ensure_runtime()?;
        let settings = self.settings.clone().normalize();
        let request = poe_alarm_runtime::ItemCheckRequest::new(
            poe_alarm_runtime::RuntimeRequestId(1),
            settings,
            text,
        );
        self.runtime
            .as_ref()
            .expect("runtime just ensured")
            .check_item(request)
            .map_err(|e| e.to_string())
    }

    #[cfg(not(windows))]
    pub fn check_item(&mut self, _text: String) -> Result<(), String> {
        Ok(())
    }

    #[cfg(windows)]
    pub fn poll(&mut self) -> Vec<BridgeEvent> {
        use poe_alarm_runtime::RuntimeEvent as E;
        let mut out = Vec::new();
        if self.sound_fell_back {
            self.sound_fell_back = false;
            out.push(BridgeEvent::SoundFallback);
        }
        let Some(runtime) = &self.runtime else {
            return out;
        };
        while let Some(event) = runtime.try_next_event() {
            match event {
                E::StateChanged { state, .. } => out.push(BridgeEvent::State(state.into())),
                E::MonitorSnapshot { snapshot, .. } => out.push(BridgeEvent::Snapshot {
                    scan_count: snapshot.scan_count,
                    capture_ms: snapshot.capture_elapsed.as_secs_f64() * 1000.0,
                    ocr_ms: snapshot.ocr_elapsed.as_secs_f64() * 1000.0,
                    cached: snapshot.ocr_was_cached,
                    lines: snapshot.last_lines.clone(),
                    detail: snapshot.detail.clone(),
                }),
                E::MatchFound { detection, .. } => out.push(BridgeEvent::MatchFound {
                    detail: detection.detail.clone(),
                    lines: detection.lines.clone(),
                    matched_group: detection.matched_group.clone(),
                }),
                E::AlertPresented { .. } => out.push(BridgeEvent::AlertPresented),
                E::AlertAcknowledged { .. } => out.push(BridgeEvent::AlertAcknowledged),
                E::AlertSoundFailed { detail, .. } => {
                    out.push(BridgeEvent::Fault {
                        detail: format!("提示音播放失败:{detail}"),
                        actionable: false,
                    });
                }
                E::Fault {
                    operation,
                    detail,
                    actionable,
                    ..
                } => out.push(BridgeEvent::Fault {
                    detail: format!("{operation:?}: {detail}"),
                    actionable,
                }),
                E::ItemCheckCompleted(report) => {
                    let matched = report.evaluation.is_match;
                    let detail = report.evaluation.detail.clone().unwrap_or_default();
                    out.push(BridgeEvent::ItemCheckReport {
                        lines: report.lines.clone(),
                        matched,
                        detail,
                    });
                }
                E::Ready
                | E::SettingsCompiled { .. }
                | E::ItemCheckCancelled { .. }
                | E::ShutdownComplete { .. } => {}
            }
        }
        out
    }

    // ------------------------------------------------------------------
    // 非 Windows:模拟器(仅供云端布局开发)
    // ------------------------------------------------------------------

    #[cfg(not(windows))]
    pub fn start_monitoring(&mut self) -> Result<(), String> {
        self.sim.start();
        Ok(())
    }

    #[cfg(not(windows))]
    pub fn stop_monitoring(&mut self) -> Result<(), String> {
        self.sim.stop();
        Ok(())
    }

    #[cfg(not(windows))]
    pub fn acknowledge_alert(&mut self) -> Result<(), String> {
        self.sim.stop();
        Ok(())
    }

    #[cfg(not(windows))]
    pub fn poll(&mut self) -> Vec<BridgeEvent> {
        self.sim.poll()
    }
}

/// 热键线程的自定义消息:wParam = StartMonitoringHotKey::OPTIONS 下标。
#[cfg(windows)]
const WM_APP_SET_START_HOTKEY: u32 = 0x8000 + 0x41; // WM_APP + 0x41

/// 全局热键线程:注册 Ctrl⇧F10(可配)与 Ctrl⇧F12,独立消息循环把
/// WM_HOTKEY 翻译成 PlatformEvent。F12 全局停止热键已按用户裁定移除;
/// 命中解除由红窗按钮与其内部按键检测负责。返回线程 id 供运行时重配热键。
#[cfg(windows)]
fn spawn_hotkey_thread(tx: Sender<PlatformEvent>, start_hot_key: String) -> Option<u32> {
    let (id_tx, id_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        use poe_alarm_platform_win::{
            HotKeyAction, HotKeyConfig, HotKeyManager, StartMonitoringHotKey,
        };
        use windows::Win32::System::Threading::GetCurrentThreadId;
        use windows::Win32::UI::WindowsAndMessaging::{GetMessageW, MSG, WM_HOTKEY};

        let config = HotKeyConfig {
            start: StartMonitoringHotKey::parse_or_default(Some(&start_hot_key)),
        };
        let mut manager = match HotKeyManager::register_for_current_thread(config) {
            Ok(manager) => manager,
            Err(error) => {
                eprintln!("global hotkey registration failed: {error}");
                let _ = tx.send(PlatformEvent::HotKeysUnavailable);
                return;
            }
        };
        manager.unregister(HotKeyAction::StopOrAcknowledge);
        // 建立消息队列后再公布线程 id(GetMessageW 首次调用前队列已由注册创建)。
        let _ = id_tx.send(unsafe { GetCurrentThreadId() });
        let mut message = MSG::default();
        // SAFETY: standard thread message loop; the manager lives for the loop's duration.
        while unsafe { GetMessageW(&mut message, None, 0, 0) }.as_bool() {
            if message.message == WM_APP_SET_START_HOTKEY {
                if let Some(option) = StartMonitoringHotKey::OPTIONS
                    .get(message.wParam.0)
                    .copied()
                    && let Err(error) = manager.reconfigure_start(option)
                {
                    eprintln!("start hotkey reconfiguration failed: {error}");
                }
                continue;
            }
            if message.message == WM_HOTKEY
                && let Some(action) = manager.action_for_message(message.message, message.wParam.0)
            {
                let event = match action {
                    HotKeyAction::StartMonitoring => PlatformEvent::HotKeyStart,
                    // Copied here rather than on the UI thread: the hotkey
                    // fires while the game holds focus and the cursor is still
                    // over the item, and both stop being true the moment the
                    // user looks away.
                    HotKeyAction::CheckItemUnderCursor => copy_item_under_cursor(),
                    HotKeyAction::StopOrAcknowledge => continue,
                };
                if tx.send(event).is_err() {
                    break;
                }
            }
        }
    });
    id_rx.recv().ok()
}

/// Reads the item the cursor is resting on, or `None` if there is not one.
///
/// A far longer deadline than the monitor uses. That one is short because a
/// slow answer means a craft is in flight and waiting would blind it; here the
/// user has asked a direct question and is waiting for the reply.
#[cfg(windows)]
fn copy_item_under_cursor() -> PlatformEvent {
    use poe_alarm_platform_win::{
        KeyMethod, copy_hovered_item, game_is_foreground, game_process_outranks_us,
    };

    if !game_is_foreground() {
        return PlatformEvent::GameNotFocused;
    }
    // Three different things used to come back as "no item found", which read
    // as the hotkey not working at all. They need different answers: press it
    // again over an item, or fix the privilege mismatch that is silently
    // eating the keystroke.
    match copy_hovered_item(std::time::Duration::from_millis(250), KeyMethod::default()) {
        Ok(outcome) if poe_alarm_clipboard::looks_like_item(&outcome.text) => {
            PlatformEvent::ItemUnderCursor(outcome.text)
        }
        Ok(_) => PlatformEvent::NoItemUnderCursor,
        Err(_) => PlatformEvent::CopyRefused {
            outranked: game_process_outranks_us(),
        },
    }
}

#[cfg(windows)]
fn resolve_alert_wave(
    settings: &AppSettings,
) -> Result<(poe_alarm_platform_win::ValidatedWave, bool), String> {
    if let Some(path) = settings.custom_alert_sound_path.as_deref() {
        match poe_alarm_platform_win::ValidatedWave::open(path) {
            Ok(wave) => return Ok((wave, false)),
            Err(error) => eprintln!("custom alert sound was rejected: {error}"),
        }
    }
    poe_alarm_platform_win::built_in_alert_wave()
        .map(|wave| (wave, settings.custom_alert_sound_path.is_some()))
        .map_err(|error| format!("could not build the bundled alert sound: {error}"))
}

#[cfg(not(windows))]
mod sim {
    use super::{BridgeEvent, BridgeState};

    #[derive(Default)]
    pub struct Sim {
        running: bool,
        ticks: u64,
        pending: Vec<BridgeEvent>,
    }

    impl Sim {
        pub fn start(&mut self) {
            self.running = true;
            self.ticks = 0;
            self.pending
                .push(BridgeEvent::State(BridgeState::Monitoring));
        }

        pub fn stop(&mut self) {
            if self.running {
                self.running = false;
                self.pending.push(BridgeEvent::State(BridgeState::Idle));
            }
        }

        pub fn poll(&mut self) -> Vec<BridgeEvent> {
            let mut out = std::mem::take(&mut self.pending);
            if self.running {
                self.ticks += 1;
                if self.ticks.is_multiple_of(10) {
                    out.push(BridgeEvent::Snapshot {
                        scan_count: self.ticks,
                        capture_ms: 3.1,
                        ocr_ms: 29.0,
                        cached: self.ticks.is_multiple_of(20),
                        lines: vec!["增加 7% 攻擊速度".to_owned()],
                        detail: None,
                    });
                }
            }
            out
        }
    }
}
