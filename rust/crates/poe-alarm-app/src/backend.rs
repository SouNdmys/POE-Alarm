//! Phase 4:GPUI 前端 ↔ 生产运行时桥接。
//!
//! Windows 上使用真实 `poe-alarm-runtime`(截屏、OCR、规则判定、红色拦截窗
//! 与提示音都由 runtime 内部完成;UI 只发命令、收事件)。非 Windows 平台
//! 提供模拟器,保证云端 Linux 开发环境可编译、可迭代布局。
//!
//! 设置继续使用 Rust 预览独立目录(`SettingsStore::preview_default`),
//! 不接管 .NET 正式配置。

// 非 Windows 平台下部分事件变体只由 Windows 路径构造。
#![allow(dead_code)]

use std::sync::mpsc::{Receiver, Sender, channel};

use poe_alarm_settings::{AppSettings, ScreenRegion, SettingsStore};

/// 平台事件(全局热键 / 框选结果 / 浮窗拖动),由后台线程投递、UI 轮询消费。
#[derive(Clone, Copy, Debug)]
pub enum PlatformEvent {
    HotKeyStart,
    HotKeySelectRegion,
    HotKeyStopOrAcknowledge,
    RegionSelected(ScreenRegion),
    RegionSelectionCancelled,
    RegionSelectionFailed,
    /// HUD 拖动结束:工作区内的相对坐标(0..=1),供写回设置。
    HudMoved(f64, f64),
}

/// 运行时状态镜像(非 Windows 平台没有 runtime crate,因此桥接层自带一份)。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BridgeState {
    Idle,
    Starting,
    Monitoring,
    TestingScreenshot,
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
            R::TestingScreenshot => Self::TestingScreenshot,
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
    /// 监控快照:扫描轮数 / 截屏耗时 ms / OCR 耗时 ms / 是否缓存复用 / 最近行
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
    ScreenshotReport {
        lines: Vec<String>,
        matched: bool,
        detail: String,
    },
    AlertPresented,
    AlertAcknowledged,
    Fault(String),
    SoundFallback,
}

pub struct Backend {
    store: SettingsStore,
    pub settings: AppSettings,
    pub read_only: bool,
    platform_rx: Receiver<PlatformEvent>,
    platform_tx: Sender<PlatformEvent>,
    selecting_region: bool,
    #[cfg(windows)]
    hud: Option<crate::hud_service::HudService>,
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
            SettingsStore::preview_default().map_err(|e| format!("settings path: {e}"))?;
        let settings = store.load();
        let read_only = store.is_read_only();
        let (platform_tx, platform_rx) = channel();
        #[cfg(windows)]
        spawn_hotkey_thread(
            platform_tx.clone(),
            settings.start_monitoring_hot_key.clone(),
        );
        #[cfg(windows)]
        let hud = Some(crate::hud_service::HudService::start(
            settings.allow_overlay_capture,
            settings.keep_hud_visible,
            settings.hud_placement.clone(),
            platform_tx.clone(),
        ));
        Ok(Self {
            store,
            settings,
            read_only,
            platform_rx,
            platform_tx,
            selecting_region: false,
            #[cfg(windows)]
            hud,
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

    /// 排空平台事件(热键 / 框选结果)。
    pub fn poll_platform(&mut self) -> Vec<PlatformEvent> {
        let mut out = Vec::new();
        while let Ok(event) = self.platform_rx.try_recv() {
            if matches!(
                event,
                PlatformEvent::RegionSelected(_)
                    | PlatformEvent::RegionSelectionCancelled
                    | PlatformEvent::RegionSelectionFailed
            ) {
                self.selecting_region = false;
            }
            out.push(event);
        }
        out
    }

    /// 触发 F11 框选(独立线程运行全屏 overlay,结果经 channel 回投)。
    pub fn begin_region_selection(&mut self) -> Result<(), String> {
        if self.selecting_region {
            return Ok(());
        }
        self.selecting_region = true;
        #[cfg(windows)]
        {
            let tx = self.platform_tx.clone();
            std::thread::spawn(move || {
                use poe_alarm_platform_win::{RegionSelectionOverlay, SelectionOverlayConfig};
                let event = match RegionSelectionOverlay::select(SelectionOverlayConfig::default())
                {
                    Ok(Some(rect)) => PlatformEvent::RegionSelected(ScreenRegion::new(
                        rect.x,
                        rect.y,
                        rect.width,
                        rect.height,
                    )),
                    Ok(None) => PlatformEvent::RegionSelectionCancelled,
                    Err(error) => {
                        eprintln!("region selection failed: {error}");
                        PlatformEvent::RegionSelectionFailed
                    }
                };
                let _ = tx.send(event);
            });
        }
        #[cfg(not(windows))]
        {
            let _ = self
                .platform_tx
                .send(PlatformEvent::RegionSelected(ScreenRegion::new(
                    0, 58, 1134, 956,
                )));
        }
        Ok(())
    }

    pub fn set_game(&mut self, profile: poe_alarm_settings::GameProfile) {
        self.settings.selected_game_profile = profile;
    }

    pub fn set_ocr_language(&mut self, language: &str) {
        self.settings.selected_profile_mut().ocr_language =
            poe_alarm_settings::normalize_ocr_language(language).to_owned();
    }

    pub fn set_region(&mut self, region: ScreenRegion) {
        self.settings.selected_profile_mut().capture_region = Some(region);
    }

    pub fn region_label(&self) -> String {
        match self.settings.selected_profile().capture_region {
            Some(r) => format!("{}×{} @ {},{}", r.width, r.height, r.x, r.y),
            None => "未框选".to_owned(),
        }
    }

    pub fn ocr_language_label(&self) -> String {
        self.settings.selected_profile().ocr_language.clone()
    }

    pub fn sound_label(&self) -> String {
        match self.settings.custom_alert_sound_path.as_deref() {
            Some(path) => std::path::Path::new(path)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.to_owned()),
            None => "内置提示音".to_owned(),
        }
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
                poe_alarm_runtime::ProductionRuntimeConfig {
                    alert,
                    paddle: None,
                },
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

    /// 识别截图:用当前设置在 runtime 里回放存档截图。
    #[cfg(windows)]
    pub fn test_screenshot(&mut self, path: std::path::PathBuf) -> Result<(), String> {
        self.ensure_runtime()?;
        let settings = self.settings.clone().normalize();
        let request = poe_alarm_runtime::ScreenshotRequest::new(
            poe_alarm_runtime::RuntimeRequestId(1),
            settings,
            path,
        );
        self.runtime
            .as_ref()
            .expect("runtime just ensured")
            .test_screenshot(request)
            .map_err(|e| e.to_string())
    }

    #[cfg(not(windows))]
    pub fn test_screenshot(&mut self, _path: std::path::PathBuf) -> Result<(), String> {
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
                    out.push(BridgeEvent::Fault(format!("提示音播放失败:{detail}")));
                }
                E::Fault {
                    operation, detail, ..
                } => out.push(BridgeEvent::Fault(format!("{operation:?}: {detail}"))),
                E::ScreenshotCompleted(report) => {
                    let matched = report.evaluation.is_match;
                    let detail = report.evaluation.detail.clone().unwrap_or_default();
                    out.push(BridgeEvent::ScreenshotReport {
                        lines: report.lines.clone(),
                        matched,
                        detail,
                    });
                }
                E::Ready
                | E::SettingsCompiled { .. }
                | E::ScreenshotCancelled { .. }
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

/// 全局热键线程:注册 Ctrl⇧F10/F11/F12(以设置里的启动热键为准),
/// 独立消息循环把 WM_HOTKEY 翻译成 PlatformEvent。
#[cfg(windows)]
fn spawn_hotkey_thread(tx: Sender<PlatformEvent>, start_hot_key: String) {
    std::thread::spawn(move || {
        use poe_alarm_platform_win::{
            HotKeyAction, HotKeyConfig, HotKeyManager, StartMonitoringHotKey,
        };
        use windows::Win32::UI::WindowsAndMessaging::{GetMessageW, MSG, WM_HOTKEY};

        let config = HotKeyConfig {
            start: StartMonitoringHotKey::parse_or_default(Some(&start_hot_key)),
        };
        let manager = match HotKeyManager::register_for_current_thread(config) {
            Ok(manager) => manager,
            Err(error) => {
                eprintln!("global hotkey registration failed: {error}");
                return;
            }
        };
        let mut message = MSG::default();
        // SAFETY: standard thread message loop; the manager lives for the loop's duration.
        while unsafe { GetMessageW(&mut message, None, 0, 0) }.as_bool() {
            if message.message == WM_HOTKEY
                && let Some(action) = manager.action_for_message(message.message, message.wParam.0)
            {
                let event = match action {
                    HotKeyAction::StartMonitoring => PlatformEvent::HotKeyStart,
                    HotKeyAction::SelectRegion => PlatformEvent::HotKeySelectRegion,
                    HotKeyAction::StopOrAcknowledge => PlatformEvent::HotKeyStopOrAcknowledge,
                };
                if tx.send(event).is_err() {
                    break;
                }
            }
        }
    });
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
    poe_alarm_app_win::built_in_alert_wave()
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
