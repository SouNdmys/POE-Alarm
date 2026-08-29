# Native runtime boundaries

`poe-alarm-runtime` is the background orchestration boundary between the GPUI
front end and the clipboard/monitoring services. It has no dependency on a
capture or recognition crate, because there is nothing to capture or recognise:
affixes arrive as the item text the game client itself writes to the clipboard.

## UI-thread contract

The UI may only enqueue a typed `RuntimeCommand` — `Start`, `Stop`, `CheckItem`,
`CancelItemCheck`, `AlertAck`, `Shutdown` — and poll `RuntimeEvent` with
`try_next_event`. Runtime methods never join a worker, read the clipboard, or
wait for an alert window. Dropping the handle requests shutdown and detaches.

Hot-key registration and HUD window ownership remain on the native UI message
thread because their platform types are deliberately thread-bound. A compiled
`CompiledUiBindings` value is returned with each accepted settings snapshot so
the UI can apply those settings without compiling rules itself.

## Where affixes come from

A monitor is generic over `AffixSource`. Production supplies `ClipboardSource`
through `AffixSourceFactory`; `DynamicSource` is the newtype that lets a boxed
source satisfy the trait, since a blanket impl over a foreign `Box` and a
foreign trait cannot be written here.

`ClipboardSource` injects nothing. Each poll reads the clipboard sequence
number — a global counter, one syscall — and only when the user's own `Ctrl+C`
has moved it does the source read the payload, parse it, and hand it to the
rules. While the game is not the foreground window the source reads no content
at all: the clipboard belongs to whatever else the user is doing, and a copy
made while tabbed out must not be misread as a roll later.

No failure here is actionable in the elevation sense. The injector's one
actionable fault was Windows discarding keystrokes aimed at an elevated game;
with nothing sent, elevation is irrelevant to monitoring. The manual check
hotkey in the app crate is the only remaining caller of `copy_hovered_item`,
and it keeps its own elevation flow.

Polling pace lives in `poe-alarm-monitoring::clock`, not here. The passive poll
is a counter check, so it runs at the scheduler tick rather than at the
contention-shaped interval the injector needed — see that module.

## Live session ownership

Each `Start` compiles a fresh immutable `MonitorPlan`, creates a new source,
and starts one `poe_alarm_monitoring::Monitor`. A game or language switch
therefore cannot retain state from the previous profile.

Replacement commands are coalesced before any new worker is allocated. At most
one retiring worker is tracked at a time, and no replacement starts until that
worker has actually dropped its source. Commands received during that interval
overwrite one `pending_intent`, so only the newest `Start` or `CheckItem`
survives. A cleanup-thread creation failure moves the runtime into a terminal
`CleanupBlocked` state rather than leaking another session.

High-frequency monitor snapshots are coalesced into one latest-value slot.
Detections, faults, item-check results and lifecycle transitions use the
reliable event channel, so an unresponsive UI cannot build an unbounded
snapshot backlog.

## Input interception and alert handoff

The mouse hook is not armed for the duration of a session. It is armed at the
instant a rule matches, by `armed_guard_for_latch`, because a `WH_MOUSE_LL`
hook installs in microseconds while a shield window takes tens of milliseconds
— and the window that matters is the gap between the user's button release and
their next press. If either `prepare` or `arm` fails the guard is dropped and
the match still proceeds: a match with a slower lock is still a match.

After a match, `BlockingAlertService::trigger` creates and validates the red
overlay. The overlay must be visible, hit-testable and cover the intended
desktop before the runtime reports that the alert owns the interaction. Any
trigger or presentation failure surfaces as a typed `RuntimeEvent::Fault`.
There is no yellow/careful mode.

An audio failure is different: the verified overlay still blocks input, so the
runtime emits `AlertSoundFailed` without releasing the alert or faulting the
session. Alert events whose native alert ID is no longer known are discarded
rather than attached to a synthetic generation.

## Checking one item on demand

`CheckItem` carries `ItemCheckRequest`, which holds the raw `Ctrl+C` text
verbatim plus the settings to judge it against. This is what restores the
workflow the OCR build had for free — hover an item you already own, and get a
verdict — which the monitor alone cannot provide, since it only evaluates when
the text *changes* and its first read establishes a baseline.

Because the text arrives with the request, this path also serves offline
testing: any item text pasted into the UI runs the same compile and evaluation
as live monitoring. It never opens an alert. Cancellation and replacement
invalidate the request and suppress a late result.

## Shutdown

`Stop`, profile replacement and `Shutdown` invalidate the active generation and
the protection service's generation lease before moving the monitor to its
single tracked drain worker. The event bridge has its own shared validity flag
and the native guard slot is generation-bound, so an old worker cannot publish a
detection, latch an alert, or release a newer session's guard after the stop
command has linearized.

An accepted alert owns a separate native alert-ID lease. Replacement work waits
until both the live generation and that lease are gone, so it cannot race the
300 ms acknowledgement/button-release drain. Empty alert-event polls are
observational only and never clear the live generation. Queued commands are
coalesced before pending work is launched, and `Shutdown` takes priority over a
queued replacement.

Shutdown targets one second to application close. A passive read is a counter
check plus at most a bounded clipboard open, so unlike the OCR build there is no
long native call to abandon; the runtime still invalidates the tracked worker
without joining it.
The alert and sound services are stopped and verified synchronously on the
runtime actor before `ShutdownComplete`.

Terminal cleanup force-releases the guard's lock-free state atomically. Waking
the native hook thread is best-effort and the actor never joins it. The hook
thread retains the process-wide singleton token until it really unhooks, so a
stalled cleanup rejects another hook with `AlreadyInUse` instead of installing
two conflicting callbacks.
