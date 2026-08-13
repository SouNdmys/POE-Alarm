#![cfg(windows)]

use std::path::{Path, PathBuf};

use poe_alarm_ocr_paddle::{
    EXPECTED_CLASS_COUNT, EXPECTED_DICTIONARY_ENTRIES, PaddleAssetPaths, run_self_test,
};

#[test]
#[ignore = "requires the locally packaged ONNX Runtime 1.28 DLL"]
fn official_model_contract_and_real_inference() {
    let runtime = find_runtime().expect("locally packaged ONNX Runtime 1.28 DLL");
    let report = run_self_test(runtime, &PaddleAssetPaths::source_tree()).unwrap();
    assert_eq!(report.dictionary_entries, EXPECTED_DICTIONARY_ENTRIES);
    assert_eq!(report.contract.input_shape, [-1, 3, 48, -1]);
    assert_eq!(report.contract.output_shape, [-1, -1, 18_385]);
    assert_eq!(report.inference.tensor_width, 320);
    assert!(report.inference.time_steps > 0);
    assert_eq!(report.inference.class_count, EXPECTED_CLASS_COUNT);
    assert!(report.inference.mean_confidence.is_finite());
    eprintln!(
        "session={:.3}ms inference={:.3}ms total={:.3}ms text={:?}",
        report.session_creation_elapsed.as_secs_f64() * 1_000.0,
        report.inference.total_elapsed().as_secs_f64() * 1_000.0,
        report.total_elapsed.as_secs_f64() * 1_000.0,
        report.inference.text,
    );
}

fn find_runtime() -> Option<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    [
        root.join(".packages/microsoft.ml.onnxruntime/1.28.0/runtimes/win-x64/native/onnxruntime.dll"),
        root.join(".dotnet-home/.nuget/packages/microsoft.ml.onnxruntime/1.28.0/runtimes/win-x64/native/onnxruntime.dll"),
    ]
    .into_iter()
    .find(|candidate| candidate.is_file())
}
