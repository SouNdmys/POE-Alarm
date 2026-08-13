# Native runtime boundaries

`poe-alarm-runtime` is the background orchestration boundary between the
native Win32 UI and the capture/recognition/monitoring services.

## UI-thread contract

The UI may only enqueue a typed `RuntimeCommand` and poll `RuntimeEvent` with
`try_next_event`. Runtime methods never join a worker, capture the screen, run
OCR, decode a screenshot, or wait for an alert window. Dropping the handle
requests shutdown and detaches.

Hot-key registration and HUD window ownership remain on the native UI message
thread because their platform types are deliberately thread-bound. A compiled
`CompiledUiBindings` value is returned with each accepted settings snapshot so
the UI can apply those settings without compiling rules itself. Region
selection likewise remains a modal UI operation; its result is persisted as a
`ScreenRegion` and validated by this runtime before monitoring starts.

## Live session ownership

Each Start command compiles a fresh immutable `MonitorPlan`, validates the
region and recognition profile, creates a new recognizer, prepares the native
mouse hook, and starts one `poe_alarm_monitoring::Monitor`. A game/language
switch therefore cannot retain recognition cache state from the previous
profile.

Replacement commands are coalesced before any new native recognizer is
allocated. At most one retiring live monitor or screenshot worker is tracked
at a time, and no replacement starts until that worker has actually dropped
its recognizer. Commands received during that interval overwrite one
`pending_intent`, so only the newest Start or Screenshot request survives. A
cleanup-thread creation failure moves the runtime into a terminal
`CleanupBlocked` state; it does not repeatedly leak another native session.
This bounds per-runtime Paddle sessions to the active/retiring ownership
budget even when a native call ignores cancellation.

`GdiScreenCapture` contains thread-bound native handles and is not `Send`. The
production capture adapter is a zero-sized movable token; the actual reusable
GDI capture object is created in thread-local storage on the monitor worker's
first capture and is destroyed with that worker. No unsafe `Send`
implementation and no per-frame capture actor hop are used.

High-frequency monitor snapshots are coalesced into one latest-value slot.
Detections, faults, screenshot results and lifecycle transitions use the
reliable event channel. This prevents an unresponsive UI from creating an
unbounded snapshot backlog.

## Input protection and alert handoff

For compatibility with the verified .NET 1.0 behavior, the pending mouse hook
is used only for Traditional Chinese recognition and POE2. POE1 English still
uses the semantic fingerprint cache but never installs or arms this short
guard. Where enabled, the hook is installed before `Monitor::start`, while it
is still pass-through, so an already-held physical button is known before the
first changed frame arms protection. The monitor callback performs this order:

1. arm the prepared pending-input guard;
2. run recognition and strict rule evaluation;
3. synchronously pass the owned guard to `BlockingAlertService::trigger`;
4. return only after the red alert has accepted ownership;
5. let the monitor release its now-empty short guard request.

The alert service verifies that its red overlay is visible and hit-testable
before it transfers the guard. Any preparation, arm, trigger or presentation
failure fails open and is surfaced as a typed `RuntimeEvent::Fault`. There is
no yellow/careful mode.

An audio playback failure is different: the verified red overlay continues to
block input, so the runtime emits `AlertSoundFailed` without releasing the
alert or changing the session to Faulted. Alert events whose native alert ID is
no longer known are discarded instead of being attached to a synthetic
generation.

## Screenshot replay

A Screenshot command first stops live monitoring and invalidates its
generation. WIC decodes the full PNG/JPEG on a dedicated screenshot thread.
The configured crop is applied when it fits; otherwise the full image is used
and `used_full_image_fallback` is reported.

Screenshot replay uses the same recognition profile, `MonitorPlan`, strict
Quick assisted-target equality, and Structured
`evaluate_with_identity` path as live monitoring. Structured targets are
submitted as one batch per recognition pass. Cancellation and replacement
invalidate the generation and suppress late results; screenshot replay never
opens an alert.

A cancelled screenshot worker always reports its terminal exit internally,
including backend errors or a caught panic. That internal event is distinct
from the public screenshot result: it releases the one retiring-work slot but
cannot publish a stale completion. Repeated screenshot requests therefore
retain only the newest request without starting concurrent ONNX sessions.

## Shutdown

Stop, profile replacement and Shutdown invalidate the active generation and
the protection service's generation lease before moving the monitor to its
single tracked drain worker. The event bridge has its own shared validity
flag, and the native guard slot is generation-bound, so an old worker cannot
publish a detection, latch a red alert, or release a newer session's input
guard after the stop command has linearized. The actor does not join an
in-flight native OCR call, but it also does not start another recognizer until
the tracked drain reports that the old recognizer was dropped.

An accepted red alert owns a separate native alert-ID lease. Replacement work
waits until both the live generation and this alert lease are gone, so it
cannot race the 300 ms acknowledgement/button-release drain. Empty alert-event
polls are observational only and never clear the live generation. Queued
commands are coalesced before pending work is launched, and Shutdown takes
priority over a queued replacement.

Shutdown has a stricter one-second application-close target. The production
WinRT adapter can abandon a pending response, but the current public Paddle
adapter cannot interrupt ONNX after inference has entered the native runtime;
it checks cancellation immediately before and after that call. Shutdown
still invalidates the tracked worker without joining it. The red alert, sound
services are stopped and verified synchronously on the runtime actor before
`ShutdownComplete`.

Terminal pending-input cleanup first atomically force-releases the lock-free
guard state, making every subsequent mouse message pass through. Waking the
native hook thread is best-effort and the actor never joins that thread. The
hook thread retains the process-wide singleton token until it really unhooks,
so a stalled cleanup rejects another hook with `AlreadyInUse` instead of
installing two conflicting callbacks. A drain worker or hook thread normally
exits a few milliseconds later; if native inference or native cleanup is
stuck, process termination is the final reclamation boundary rather than a UI
or input-safety blocker.

Screenshot workers have the same production limitation because the public
offline recognition entry points use a non-cancellable probe. They are
cancelled logically and retained as the sole retiring work item: cancellation
is checked between every pass and before result publication, so a decoder or
OCR backend that is slow to return cannot block close or publish a late result.
The process-lifetime WinRT actor is intentionally shared by the OCR crate; a
per-screenshot Paddle worker drops and joins after its in-flight inference
returns, before another pending screenshot or live recognizer may start.
