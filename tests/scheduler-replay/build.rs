use std::{env, fs, path::PathBuf, process::Command};

fn main() {
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap())
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let path = "rust/crates/poe-alarm-runtime/src/clipboard_source.rs";
    let baseline = Command::new("git")
        .current_dir(&root)
        .args([
            "show",
            &format!("1e161bf0c27cc19182f816ce0dd99f46bb3bc946:{path}"),
        ])
        .output()
        .expect("git is required for the 1.1.4 baseline");
    assert!(
        baseline.status.success(),
        "fetch repository history containing 1.1.4 first"
    );
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    for (name, source) in [
        ("baseline", String::from_utf8(baseline.stdout).unwrap()),
        ("candidate", fs::read_to_string(root.join(path)).unwrap()),
    ] {
        // Only the OS boundary and clock are substituted. The production
        // source, parser and matcher execute without sending real input.
        let source = source
            .replace(
                "use poe_alarm_platform_win::{",
                "use crate::fake_platform::{",
            )
            .replace("Instant::now()", "crate::fake_platform::now()");
        fs::write(out.join(format!("{name}.rs")), source).unwrap();
    }
    fs::write(
        out.join("sources.rs"),
        format!(
            "#[path = {:?}] mod baseline;\n#[path = {:?}] mod candidate;\n",
            out.join("baseline.rs"),
            out.join("candidate.rs"),
        ),
    )
    .unwrap();
    println!("cargo:rerun-if-changed={}", root.join(path).display());
}
