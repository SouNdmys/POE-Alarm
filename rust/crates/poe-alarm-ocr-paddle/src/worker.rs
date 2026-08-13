use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use poe_alarm_core::FullLineAffixMatcher;

use crate::{
    CtcBatchRecognition, CtcRecognition, CtcTargetRecognition, OwnedImage, PaddleAssetPaths,
    PaddleAssets, PaddleCtcSession, PaddleError, PaddleSessionConfig, initialize_onnx_runtime,
};

enum WorkerCommand {
    Recognize {
        image: OwnedImage,
        reply: Sender<Result<CtcRecognition, PaddleError>>,
    },
    RecognizeBatch {
        image: OwnedImage,
        targets: Vec<FullLineAffixMatcher>,
        reply: Sender<Result<CtcBatchRecognition, PaddleError>>,
    },
    RecognizeSegmented {
        image: OwnedImage,
        maximum_segment_width: usize,
        reply: Sender<Result<CtcRecognition, PaddleError>>,
    },
    RecognizeTarget {
        image: OwnedImage,
        target: FullLineAffixMatcher,
        maximum_segment_width: Option<usize>,
        reply: Sender<Result<CtcTargetRecognition, PaddleError>>,
    },
    Stop,
}

/// Pending batch recognition whose receive operation never runs on the OCR actor.
pub struct PaddleBatchPending {
    receiver: Receiver<Result<CtcBatchRecognition, PaddleError>>,
}

impl PaddleBatchPending {
    pub fn wait(self) -> Result<CtcBatchRecognition, PaddleError> {
        self.receiver
            .recv()
            .map_err(|_| PaddleError::WorkerUnavailable)?
    }

    /// Polls without blocking so a monitoring controller can abandon an in-flight result after
    /// cancellation. The actor still owns and finishes the native inference safely.
    pub fn try_take(
        &self,
    ) -> Result<Option<Result<CtcBatchRecognition, PaddleError>>, PaddleError> {
        try_take(&self.receiver)
    }

    pub fn take_timeout(
        &self,
        timeout: Duration,
    ) -> Result<Option<Result<CtcBatchRecognition, PaddleError>>, PaddleError> {
        take_timeout(&self.receiver, timeout)
    }
}

/// Pending single-target recognition, including the strict segmented route.
pub struct PaddleTargetPending {
    receiver: Receiver<Result<CtcTargetRecognition, PaddleError>>,
}

impl PaddleTargetPending {
    pub fn wait(self) -> Result<CtcTargetRecognition, PaddleError> {
        self.receiver
            .recv()
            .map_err(|_| PaddleError::WorkerUnavailable)?
    }

    pub fn try_take(
        &self,
    ) -> Result<Option<Result<CtcTargetRecognition, PaddleError>>, PaddleError> {
        try_take(&self.receiver)
    }

    pub fn take_timeout(
        &self,
        timeout: Duration,
    ) -> Result<Option<Result<CtcTargetRecognition, PaddleError>>, PaddleError> {
        take_timeout(&self.receiver, timeout)
    }
}

/// Pending recognition whose receive operation never executes on the OCR thread.
pub struct PaddlePending {
    receiver: Receiver<Result<CtcRecognition, PaddleError>>,
}

impl PaddlePending {
    pub fn wait(self) -> Result<CtcRecognition, PaddleError> {
        self.receiver
            .recv()
            .map_err(|_| PaddleError::WorkerUnavailable)?
    }

    pub fn try_take(&self) -> Result<Option<Result<CtcRecognition, PaddleError>>, PaddleError> {
        try_take(&self.receiver)
    }

    pub fn take_timeout(
        &self,
        timeout: Duration,
    ) -> Result<Option<Result<CtcRecognition, PaddleError>>, PaddleError> {
        take_timeout(&self.receiver, timeout)
    }
}

fn try_take<T>(
    receiver: &Receiver<Result<T, PaddleError>>,
) -> Result<Option<Result<T, PaddleError>>, PaddleError> {
    match receiver.try_recv() {
        Ok(result) => Ok(Some(result)),
        Err(mpsc::TryRecvError::Empty) => Ok(None),
        Err(mpsc::TryRecvError::Disconnected) => Err(PaddleError::WorkerUnavailable),
    }
}

fn take_timeout<T>(
    receiver: &Receiver<Result<T, PaddleError>>,
    timeout: Duration,
) -> Result<Option<Result<T, PaddleError>>, PaddleError> {
    match receiver.recv_timeout(timeout) {
        Ok(result) => Ok(Some(result)),
        Err(mpsc::RecvTimeoutError::Timeout) => Ok(None),
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(PaddleError::WorkerUnavailable),
    }
}

/// Process-lifetime owner for the serial native ONNX session and reusable tensor workspace.
pub struct PaddleOcrWorker {
    sender: Sender<WorkerCommand>,
    join: Option<JoinHandle<()>>,
}

impl PaddleOcrWorker {
    pub fn start(
        runtime_library: PathBuf,
        asset_paths: PaddleAssetPaths,
        config: PaddleSessionConfig,
    ) -> Result<Self, PaddleError> {
        let (sender, receiver) = mpsc::channel();
        let (startup_sender, startup_receiver) = mpsc::sync_channel(1);
        let join = thread::Builder::new()
            .name("poe-alarm-paddle-ocr".to_owned())
            .spawn(move || {
                let session = initialize_onnx_runtime(runtime_library)
                    .and_then(|()| PaddleAssets::load(&asset_paths))
                    .and_then(|assets| PaddleCtcSession::from_assets(&assets, config));
                match session {
                    Ok(mut session) => {
                        if startup_sender.send(Ok(())).is_err() {
                            return;
                        }
                        while let Ok(command) = receiver.recv() {
                            match command {
                                WorkerCommand::Recognize { image, reply } => {
                                    let _ = reply.send(session.recognize(image.as_view()));
                                }
                                WorkerCommand::RecognizeBatch {
                                    image,
                                    targets,
                                    reply,
                                } => {
                                    let _ = reply
                                        .send(session.recognize_batch(image.as_view(), &targets));
                                }
                                WorkerCommand::RecognizeSegmented {
                                    image,
                                    maximum_segment_width,
                                    reply,
                                } => {
                                    let _ = reply.send(session.recognize_with_segment_width(
                                        image.as_view(),
                                        Some(maximum_segment_width),
                                    ));
                                }
                                WorkerCommand::RecognizeTarget {
                                    image,
                                    target,
                                    maximum_segment_width,
                                    reply,
                                } => {
                                    let _ =
                                        reply.send(session.recognize_target_with_segment_width(
                                            image.as_view(),
                                            &target,
                                            maximum_segment_width,
                                        ));
                                }
                                WorkerCommand::Stop => break,
                            }
                        }
                    }
                    Err(error) => {
                        let _ = startup_sender.send(Err(error.to_string()));
                    }
                }
            })
            .map_err(|error| PaddleError::WorkerStart(error.to_string()))?;
        match startup_receiver.recv() {
            Ok(Ok(())) => Ok(Self {
                sender,
                join: Some(join),
            }),
            Ok(Err(message)) => {
                let _ = join.join();
                Err(PaddleError::WorkerStart(message))
            }
            Err(_) => {
                let _ = join.join();
                Err(PaddleError::WorkerStart(
                    "worker exited during initialization".to_owned(),
                ))
            }
        }
    }

    pub fn recognize(&self, image: OwnedImage) -> Result<PaddlePending, PaddleError> {
        let (reply, receiver) = mpsc::channel();
        self.sender
            .send(WorkerCommand::Recognize { image, reply })
            .map_err(|_| PaddleError::WorkerUnavailable)?;
        Ok(PaddlePending { receiver })
    }

    /// Queues one inference for the complete target set. Canonically duplicate
    /// targets are removed by the session before logits are analyzed.
    pub fn recognize_batch(
        &self,
        image: OwnedImage,
        targets: Vec<FullLineAffixMatcher>,
    ) -> Result<PaddleBatchPending, PaddleError> {
        let (reply, receiver) = mpsc::channel();
        self.sender
            .send(WorkerCommand::RecognizeBatch {
                image,
                targets,
                reply,
            })
            .map_err(|_| PaddleError::WorkerUnavailable)?;
        Ok(PaddleBatchPending { receiver })
    }

    /// Queues one target-independent segmented greedy decode. Callers can run the returned
    /// transcript through any number of strict matchers without repeating ONNX work per target.
    pub fn recognize_segmented(
        &self,
        image: OwnedImage,
        maximum_segment_width: usize,
    ) -> Result<PaddlePending, PaddleError> {
        let (reply, receiver) = mpsc::channel();
        self.sender
            .send(WorkerCommand::RecognizeSegmented {
                image,
                maximum_segment_width,
                reply,
            })
            .map_err(|_| PaddleError::WorkerUnavailable)?;
        Ok(PaddlePending { receiver })
    }

    pub fn recognize_target(
        &self,
        image: OwnedImage,
        target: FullLineAffixMatcher,
        maximum_segment_width: Option<usize>,
    ) -> Result<PaddleTargetPending, PaddleError> {
        let (reply, receiver) = mpsc::channel();
        self.sender
            .send(WorkerCommand::RecognizeTarget {
                image,
                target,
                maximum_segment_width,
                reply,
            })
            .map_err(|_| PaddleError::WorkerUnavailable)?;
        Ok(PaddleTargetPending { receiver })
    }
}

impl Drop for PaddleOcrWorker {
    fn drop(&mut self) {
        let _ = self.sender.send(WorkerCommand::Stop);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_poll_is_non_blocking_before_and_after_completion() {
        let (sender, receiver) = mpsc::channel::<Result<(), PaddleError>>();

        assert!(try_take(&receiver).unwrap().is_none());
        sender.send(Ok(())).unwrap();
        assert!(matches!(try_take(&receiver), Ok(Some(Ok(())))));
    }

    #[test]
    fn dropping_a_pending_result_does_not_require_waiting_for_native_completion() {
        let (sender, receiver) = mpsc::channel();
        let pending = PaddlePending { receiver };

        assert!(pending.try_take().unwrap().is_none());
        drop(pending);

        assert!(sender.send(Err(PaddleError::WorkerUnavailable)).is_err());
    }
}
