use std::sync::{Arc, mpsc};
use std::thread::JoinHandle;

use crate::service::{RuntimeShared, ServiceOwnership, WorkerCommand};
use crate::{AlertEvent, AlertFailure, AlertFailureKind, AlertServiceConfig};

pub(crate) fn spawn_worker(
    _config: AlertServiceConfig,
    _commands: mpsc::Receiver<WorkerCommand>,
    _events: mpsc::Sender<AlertEvent>,
    _warnings: mpsc::Sender<AlertFailure>,
    _runtime: Arc<RuntimeShared>,
    _ready: mpsc::SyncSender<Result<(), AlertFailure>>,
    _ownership: ServiceOwnership,
) -> Result<JoinHandle<()>, AlertFailure> {
    Err(unsupported())
}

pub(crate) fn wake_commands(_thread_id: u32) -> Result<(), AlertFailure> {
    Err(unsupported())
}

pub(crate) fn wake_acknowledge(_thread_id: u32) -> Result<(), AlertFailure> {
    Err(unsupported())
}

pub(crate) fn wake_stop(_thread_id: u32) -> Result<(), AlertFailure> {
    Err(unsupported())
}

fn unsupported() -> AlertFailure {
    AlertFailure::new(
        AlertFailureKind::UnsupportedPlatform,
        "the blocking red alert is only available on Windows",
    )
}
