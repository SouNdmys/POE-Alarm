use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock, mpsc};
use std::time::Duration;

use poe_alarm_vision::{CapturedFrame, CroppedMask};

use crate::{
    OcrCapabilities, OcrError, OcrLane, OcrLanguagePreference, OcrRecognition, OwnedBgraImage,
};

const MAXIMUM_QUEUED_COMMANDS: usize = 16;

#[cfg(not(windows))]
use crate::non_windows::OcrBackend;
#[cfg(windows)]
use crate::windows_ocr::OcrBackend;

/// Cloneable client for the one process-lifetime OCR actor.
///
/// `submit_*` only queues work and is suitable for a UI thread. `wait` and the
/// convenience blocking methods must run on a controller/background thread.
#[derive(Clone)]
pub struct OcrWorker {
    runtime: Arc<WorkerRuntime>,
}

impl OcrWorker {
    /// Starts the process-lifetime MTA worker once, or attaches to it.
    pub fn start() -> Result<Self, OcrError> {
        static RUNTIME: OnceLock<Result<Arc<WorkerRuntime>, OcrError>> = OnceLock::new();
        match RUNTIME.get_or_init(WorkerRuntime::start) {
            Ok(runtime) => Ok(Self {
                runtime: Arc::clone(runtime),
            }),
            Err(error) => Err(error.clone()),
        }
    }

    pub fn submit_capabilities(&self) -> Result<OcrPending<OcrCapabilities>, OcrError> {
        let (response, pending) = pending_pair();
        let cancelled = Arc::clone(&pending.cancelled);
        self.runtime
            .sender
            .try_send(Command::Capabilities {
                response,
                cancelled,
            })
            .map_err(map_try_send_error)?;
        Ok(pending)
    }

    /// Blocking convenience wrapper; do not call from the UI thread.
    pub fn capabilities(&self) -> Result<OcrCapabilities, OcrError> {
        self.submit_capabilities()?.wait()
    }

    pub fn submit_recognition(
        &self,
        preference: OcrLanguagePreference,
        image: OwnedBgraImage,
    ) -> Result<OcrPending<OcrRecognition>, OcrError> {
        self.submit_recognition_on(OcrLane::Default, preference, image)
    }

    pub fn submit_recognition_on(
        &self,
        lane: OcrLane,
        preference: OcrLanguagePreference,
        image: OwnedBgraImage,
    ) -> Result<OcrPending<OcrRecognition>, OcrError> {
        let (response, pending) = pending_pair();
        let cancelled = Arc::clone(&pending.cancelled);
        self.runtime
            .sender
            .try_send(Command::Recognize {
                lane,
                preference,
                image,
                response,
                cancelled,
            })
            .map_err(map_try_send_error)?;
        Ok(pending)
    }

    /// Blocking convenience wrapper; do not call from the UI thread.
    pub fn recognize(
        &self,
        preference: OcrLanguagePreference,
        image: OwnedBgraImage,
    ) -> Result<OcrRecognition, OcrError> {
        self.submit_recognition(preference, image)?.wait()
    }

    pub fn recognize_frame(
        &self,
        preference: OcrLanguagePreference,
        frame: &CapturedFrame,
    ) -> Result<OcrRecognition, OcrError> {
        self.recognize(preference, OwnedBgraImage::from_frame(frame))
    }

    pub fn recognize_crop(
        &self,
        preference: OcrLanguagePreference,
        crop: &CroppedMask,
    ) -> Result<OcrRecognition, OcrError> {
        self.recognize(preference, OwnedBgraImage::from_crop(crop))
    }
}

/// One typed response from the OCR actor.
pub struct OcrPending<T> {
    receiver: mpsc::Receiver<Result<T, OcrError>>,
    cancelled: Arc<AtomicBool>,
}

impl<T> Drop for OcrPending<T> {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
    }
}

impl<T> OcrPending<T> {
    /// Blocks the current thread until the worker finishes. Never use on the UI thread.
    pub fn wait(self) -> Result<T, OcrError> {
        self.receiver
            .recv()
            .map_err(|_| OcrError::WorkerUnavailable)?
    }

    /// Polls without blocking, for event-loop integration.
    pub fn try_take(&self) -> Result<Option<Result<T, OcrError>>, OcrError> {
        match self.receiver.try_recv() {
            Ok(result) => Ok(Some(result)),
            Err(mpsc::TryRecvError::Empty) => Ok(None),
            Err(mpsc::TryRecvError::Disconnected) => Err(OcrError::WorkerUnavailable),
        }
    }

    /// Waits for at most `timeout` for the actor to finish.
    ///
    /// Unlike a `try_take`/`yield_now` spin loop this parks the controller thread while WinRT is
    /// doing CPU work, but still lets monitoring re-check cancellation at a bounded interval.
    pub fn take_timeout(&self, timeout: Duration) -> Result<Option<Result<T, OcrError>>, OcrError> {
        match self.receiver.recv_timeout(timeout) {
            Ok(result) => Ok(Some(result)),
            Err(mpsc::RecvTimeoutError::Timeout) => Ok(None),
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(OcrError::WorkerUnavailable),
        }
    }
}

struct WorkerRuntime {
    sender: mpsc::SyncSender<Command>,
}

impl WorkerRuntime {
    fn start() -> Result<Arc<Self>, OcrError> {
        let (sender, receiver) = mpsc::sync_channel(MAXIMUM_QUEUED_COMMANDS);
        let (ready_sender, ready_receiver) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("poe-alarm-winrt-ocr".to_owned())
            .spawn(move || {
                let backend = OcrBackend::initialize();
                let ready = backend.as_ref().map(|_| ()).map_err(Clone::clone);
                let _ = ready_sender.send(ready);
                if let Ok(backend) = backend {
                    run_actor(backend, receiver);
                }
            })
            .map_err(|error| OcrError::WorkerStart {
                message: error.to_string(),
            })?;
        ready_receiver
            .recv()
            .map_err(|_| OcrError::WorkerUnavailable)??;
        Ok(Arc::new(Self { sender }))
    }
}

enum Command {
    Capabilities {
        response: mpsc::Sender<Result<OcrCapabilities, OcrError>>,
        cancelled: Arc<AtomicBool>,
    },
    Recognize {
        lane: OcrLane,
        preference: OcrLanguagePreference,
        image: OwnedBgraImage,
        response: mpsc::Sender<Result<OcrRecognition, OcrError>>,
        cancelled: Arc<AtomicBool>,
    },
    #[cfg(test)]
    Shutdown { acknowledged: mpsc::Sender<()> },
}

trait Backend {
    fn capabilities(&mut self) -> Result<OcrCapabilities, OcrError>;
    fn recognize(
        &mut self,
        lane: OcrLane,
        preference: OcrLanguagePreference,
        image: OwnedBgraImage,
    ) -> Result<OcrRecognition, OcrError>;
}

impl Backend for OcrBackend {
    fn capabilities(&mut self) -> Result<OcrCapabilities, OcrError> {
        self.capabilities()
    }

    fn recognize(
        &mut self,
        lane: OcrLane,
        preference: OcrLanguagePreference,
        image: OwnedBgraImage,
    ) -> Result<OcrRecognition, OcrError> {
        self.recognize_on(lane, preference, image)
    }
}

fn run_actor(mut backend: impl Backend, receiver: mpsc::Receiver<Command>) {
    while let Ok(command) = receiver.recv() {
        match command {
            Command::Capabilities {
                response,
                cancelled,
            } => {
                let result = if cancelled.load(Ordering::Acquire) {
                    Err(OcrError::Cancelled)
                } else {
                    backend.capabilities()
                };
                let _ = response.send(result);
            }
            Command::Recognize {
                lane,
                preference,
                image,
                response,
                cancelled,
            } => {
                let result = if cancelled.load(Ordering::Acquire) {
                    Err(OcrError::Cancelled)
                } else {
                    backend.recognize(lane, preference, image)
                };
                let _ = response.send(result);
            }
            #[cfg(test)]
            Command::Shutdown { acknowledged } => {
                let _ = acknowledged.send(());
                break;
            }
        }
    }
}

fn pending_pair<T>() -> (mpsc::Sender<Result<T, OcrError>>, OcrPending<T>) {
    let (sender, receiver) = mpsc::channel();
    (
        sender,
        OcrPending {
            receiver,
            cancelled: Arc::new(AtomicBool::new(false)),
        },
    )
}

fn map_try_send_error(error: mpsc::TrySendError<Command>) -> OcrError {
    match error {
        mpsc::TrySendError::Full(_) => OcrError::WorkerQueueFull,
        mpsc::TrySendError::Disconnected(_) => OcrError::WorkerUnavailable,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;
    use std::time::Duration;

    use super::*;

    struct MockBackend {
        calls: Arc<AtomicUsize>,
    }

    impl Backend for MockBackend {
        fn capabilities(&mut self) -> Result<OcrCapabilities, OcrError> {
            let calls = self.calls.fetch_add(1, Ordering::Relaxed) + 1;
            Ok(OcrCapabilities {
                available_language_tags: vec![format!("mock-{calls}")],
                maximum_image_dimension: 10_000,
            })
        }

        fn recognize(
            &mut self,
            lane: OcrLane,
            _preference: OcrLanguagePreference,
            _image: OwnedBgraImage,
        ) -> Result<OcrRecognition, OcrError> {
            let calls = self.calls.fetch_add(1, Ordering::Relaxed) + 1;
            Ok(OcrRecognition {
                language_tag: format!("mock-{lane:?}-{calls}"),
                elapsed: Duration::ZERO,
                lines: Vec::new(),
            })
        }
    }

    #[test]
    fn actor_serializes_requests_and_acknowledges_clean_shutdown() {
        let (sender, receiver) = mpsc::sync_channel(MAXIMUM_QUEUED_COMMANDS);
        let calls = Arc::new(AtomicUsize::new(0));
        let thread = std::thread::spawn({
            let calls = Arc::clone(&calls);
            move || run_actor(MockBackend { calls }, receiver)
        });

        let (capabilities_sender, capabilities) = pending_pair();
        let capabilities_cancelled = Arc::clone(&capabilities.cancelled);
        sender
            .send(Command::Capabilities {
                response: capabilities_sender,
                cancelled: capabilities_cancelled,
            })
            .unwrap();
        let (recognition_sender, recognition) = pending_pair();
        let recognition_cancelled = Arc::clone(&recognition.cancelled);
        sender
            .send(Command::Recognize {
                lane: OcrLane::Poe2Confirmation,
                preference: OcrLanguagePreference::English,
                image: OwnedBgraImage::new(1, 1, 4, vec![255; 4]).unwrap(),
                response: recognition_sender,
                cancelled: recognition_cancelled,
            })
            .unwrap();
        assert_eq!(
            capabilities.wait().unwrap().available_language_tags,
            ["mock-1"]
        );
        assert_eq!(
            recognition.wait().unwrap().language_tag,
            "mock-Poe2Confirmation-2"
        );

        let (acknowledged, ack) = mpsc::channel();
        sender.send(Command::Shutdown { acknowledged }).unwrap();
        ack.recv_timeout(Duration::from_secs(1)).unwrap();
        thread.join().unwrap();
        assert_eq!(calls.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn actor_skips_a_queued_request_after_its_pending_handle_is_dropped() {
        let (sender, receiver) = mpsc::sync_channel(MAXIMUM_QUEUED_COMMANDS);
        let calls = Arc::new(AtomicUsize::new(0));
        let (response, pending) = pending_pair();
        let cancelled = Arc::clone(&pending.cancelled);
        sender
            .send(Command::Recognize {
                lane: OcrLane::Default,
                preference: OcrLanguagePreference::English,
                image: OwnedBgraImage::new(1, 1, 4, vec![255; 4]).unwrap(),
                response,
                cancelled,
            })
            .unwrap();
        drop(pending);
        let (acknowledged, ack) = mpsc::channel();
        sender.send(Command::Shutdown { acknowledged }).unwrap();
        let thread = std::thread::spawn({
            let calls = Arc::clone(&calls);
            move || run_actor(MockBackend { calls }, receiver)
        });

        ack.recv_timeout(Duration::from_secs(1)).unwrap();
        thread.join().unwrap();
        assert_eq!(calls.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn bounded_queue_reports_full_without_blocking_the_submitter() {
        let (sender, _receiver) = mpsc::sync_channel(1);
        let (first_response, first_pending) = pending_pair();
        sender
            .try_send(Command::Capabilities {
                response: first_response,
                cancelled: Arc::clone(&first_pending.cancelled),
            })
            .unwrap();
        let (second_response, second_pending) = pending_pair();
        let error = sender
            .try_send(Command::Capabilities {
                response: second_response,
                cancelled: Arc::clone(&second_pending.cancelled),
            })
            .unwrap_err();
        assert!(matches!(
            map_try_send_error(error),
            OcrError::WorkerQueueFull
        ));
    }
}
