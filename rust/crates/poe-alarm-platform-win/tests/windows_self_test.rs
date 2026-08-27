#[cfg(windows)]
#[test]
fn native_resources_create_and_clean_up() {
    let report = poe_alarm_platform_win::run_windows_self_test().unwrap();
    assert!(report.hud_window_create_cleanup);
    assert!(report.hot_key_register_cleanup);
    assert!(report.mouse_hook_create_cleanup);
}
