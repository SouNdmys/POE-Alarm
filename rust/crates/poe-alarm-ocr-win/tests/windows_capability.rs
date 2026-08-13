#![cfg(windows)]

use std::sync::{Arc, Barrier};

use poe_alarm_ocr_win::{OcrLane, OcrLanguagePreference, OcrWorker, OwnedBgraImage};

fn blank(width: usize, height: usize) -> OwnedBgraImage {
    OwnedBgraImage::new(width, height, width * 4, vec![255; width * height * 4]).unwrap()
}

#[test]
fn sequential_capability_and_bilingual_recognition_share_one_actor() {
    let worker = OcrWorker::start().expect("process OCR worker");
    let capabilities = worker.capabilities().expect("Windows OCR capabilities");
    assert!(capabilities.maximum_image_dimension >= 96);
    assert!(
        capabilities
            .available_language_tags
            .iter()
            .any(|tag| tag.eq_ignore_ascii_case("en-US"))
    );

    let english = worker
        .recognize(OcrLanguagePreference::English, blank(320, 96))
        .expect("English buffer OCR");
    assert!(english.language_tag.to_ascii_lowercase().starts_with("en"));

    let traditional = worker
        .recognize(OcrLanguagePreference::TraditionalChinese, blank(320, 96))
        .expect("Traditional Chinese buffer OCR");
    assert_eq!(traditional.language_tag, "zh-Hant-TW");
}

#[test]
fn concurrent_callers_are_serialized_without_moving_winrt_objects() {
    let worker = OcrWorker::start().expect("process OCR worker");
    let barrier = Arc::new(Barrier::new(9));
    let mut callers = Vec::new();
    for _ in 0..8 {
        let worker = worker.clone();
        let barrier = Arc::clone(&barrier);
        callers.push(std::thread::spawn(move || {
            barrier.wait();
            worker
                .recognize(OcrLanguagePreference::English, blank(160, 64))
                .expect("cross-caller OCR")
                .language_tag
        }));
    }
    barrier.wait();
    for caller in callers {
        assert!(
            caller
                .join()
                .expect("caller should not panic")
                .starts_with("en")
        );
    }
}

#[test]
fn dropping_all_client_handles_keeps_process_actor_valid() {
    {
        let first = OcrWorker::start().expect("first OCR handle");
        let pending = first
            .submit_recognition(OcrLanguagePreference::English, blank(160, 64))
            .expect("queued OCR");
        drop(first);
        pending.wait().expect("queued work survives client drop");
    }

    let replacement = OcrWorker::start().expect("replacement OCR handle");
    replacement
        .capabilities()
        .expect("process actor remains valid after clean client shutdown");
}

#[test]
fn primary_and_confirmation_lanes_survive_repeated_client_reconnects() {
    for iteration in 0..25 {
        let worker = OcrWorker::start().expect("process OCR worker");
        let primary = worker
            .submit_recognition_on(
                OcrLane::Default,
                OcrLanguagePreference::English,
                blank(160, 64),
            )
            .expect("primary OCR")
            .wait()
            .expect("primary OCR result");
        let confirmation = worker
            .submit_recognition_on(
                OcrLane::Poe2Confirmation,
                OcrLanguagePreference::English,
                blank(160, 64),
            )
            .expect("confirmation OCR")
            .wait()
            .expect("confirmation OCR result");
        assert!(
            primary.language_tag.starts_with("en"),
            "iteration {iteration}"
        );
        assert!(
            confirmation.language_tag.starts_with("en"),
            "iteration {iteration}"
        );
        drop(worker);
    }
}
