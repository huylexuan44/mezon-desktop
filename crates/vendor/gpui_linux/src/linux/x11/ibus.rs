//! mezon vendor addition: native IBus D-Bus client for X11.
//!
//! The classic XIM path to ibus goes through the ibus-x11 bridge, which encodes
//! preedit/commit strings as COMPOUND_TEXT via Xutf8TextListToTextProperty.
//! Characters outside the legacy ISO-8859 charsets (Vietnamese ư/ơ/ợ/…, and
//! many others) are silently dropped by that conversion unless the string
//! happens to contain exactly one unconvertible character (ibus-x11 only
//! applies its "\x1b%G" UTF-8 fallback when the conversion reports
//! EXIT_FAILURE == 1). Typing "được" over XIM+ibus therefore commits "đc".
//!
//! This module bypasses the bridge entirely by speaking the IBus D-Bus
//! protocol (the same protocol the GTK/Qt IM modules use), so preedit and
//! commit strings arrive as UTF-8. When no ibus daemon is reachable the X11
//! client falls back to the existing XIM path.
//!
//! Ordering matters: engines emit CommitText/UpdatePreeditText signals BEFORE
//! the ProcessKeyEvent reply for the triggering key (D-Bus preserves order on
//! the connection). The worker drains every already-queued signal into the
//! event queue before answering a key request, and the X11 client applies the
//! queue before dispatching an unhandled key — otherwise Enter would send the
//! message first and the engine's final-word commit would land in the already
//! cleared composer.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use ashpd::zbus;
use futures::{FutureExt as _, StreamExt as _};
use zbus::zvariant::{self, OwnedObjectPath};

pub(crate) const IBUS_RELEASE_MASK: u32 = 1 << 30;
const IBUS_CAP_PREEDIT_TEXT: u32 = 1;
const IBUS_CAP_FOCUS: u32 = 1 << 3;
const CALL_TIMEOUT: Duration = Duration::from_millis(500);
// The worker must always answer a key before the UI thread stops waiting for it:
// if the UI gave up first it would type the raw key while the engine was still
// composing it, mixing raw keystrokes into the text. The budget is generous
// because engines load dictionaries lazily and the first keystroke of a session
// can be far slower than the rest.
const KEY_CALL_TIMEOUT: Duration = Duration::from_millis(800);
pub(crate) const KEY_REPLY_TIMEOUT: Duration = Duration::from_millis(1000);

pub(crate) enum IbusRequest {
    Key {
        keyval: u32,
        keycode: u32,
        state: u32,
        reply: mpsc::Sender<bool>,
    },
    FocusIn,
    FocusOut,
    Reset,
    CursorLocation {
        x: i32,
        y: i32,
        w: i32,
        h: i32,
    },
}

pub(crate) enum IbusEvent {
    Commit(String),
    Preedit { text: String, visible: bool },
    ForwardKey { keyval: u32, keycode: u32, state: u32 },
    Dead,
}

#[derive(Clone)]
pub(crate) struct IbusEventSink {
    queue: Arc<Mutex<VecDeque<IbusEvent>>>,
    ping: calloop::ping::Ping,
}

impl IbusEventSink {
    pub(crate) fn new(ping: calloop::ping::Ping) -> Self {
        Self {
            queue: Arc::new(Mutex::new(VecDeque::new())),
            ping,
        }
    }

    fn push(&self, event: IbusEvent) {
        match self.queue.lock() {
            Ok(mut queue) => queue.push_back(event),
            Err(poisoned) => poisoned.into_inner().push_back(event),
        }
        self.ping.ping();
    }

    pub(crate) fn pop(&self) -> Option<IbusEvent> {
        match self.queue.lock() {
            Ok(mut queue) => queue.pop_front(),
            Err(poisoned) => poisoned.into_inner().pop_front(),
        }
    }
}

#[derive(Clone)]
pub(crate) struct IbusHandle {
    request_tx: smol::channel::Sender<IbusRequest>,
    dead: Arc<AtomicBool>,
}

impl IbusHandle {
    pub(crate) fn is_alive(&self) -> bool {
        !self.dead.load(Ordering::Relaxed)
    }

    pub(crate) fn mark_dead(&self) {
        self.dead.store(true, Ordering::Relaxed);
    }

    pub(crate) fn send(&self, request: IbusRequest) {
        if self.request_tx.try_send(request).is_err() {
            self.mark_dead();
        }
    }

    pub(crate) fn process_key(&self, keyval: u32, keycode: u32, state: u32) -> Option<bool> {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.send(IbusRequest::Key {
            keyval,
            keycode,
            state,
            reply: reply_tx,
        });
        reply_rx.recv_timeout(KEY_REPLY_TIMEOUT).ok()
    }
}

pub(crate) fn ibus_address() -> Option<String> {
    if let Ok(address) = std::env::var("IBUS_ADDRESS")
        && !address.trim().is_empty()
    {
        return Some(address.trim().to_string());
    }
    let display = std::env::var("DISPLAY").ok()?;
    let display_num = display
        .rsplit(':')
        .next()?
        .split('.')
        .next()
        .filter(|num| !num.is_empty() && num.chars().all(|c| c.is_ascii_digit()))?
        .to_string();
    let config_dir = std::env::var("XDG_CONFIG_HOME")
        .ok()
        .filter(|dir| !dir.is_empty())
        .unwrap_or(format!("{}/.config", std::env::var("HOME").ok()?));
    let bus_dir = std::path::PathBuf::from(config_dir).join("ibus/bus");
    let machine_id = std::fs::read_to_string("/etc/machine-id")
        .or_else(|_| std::fs::read_to_string("/var/lib/dbus/machine-id"))
        .map(|id| id.trim().to_string())
        .unwrap_or_default();
    let preferred = bus_dir.join(format!("{machine_id}-unix-{display_num}"));
    let mut candidates = vec![preferred];
    // The file is named after the session the daemon serves, which is not always
    // the X display we were started with: a GNOME/KDE Wayland session writes
    // `<machine>-unix-wayland-0` while the app runs on XWayland as `:0` or `:1`.
    // Prefer an exact display match, then anything the daemon left behind.
    if let Ok(entries) = std::fs::read_dir(&bus_dir) {
        let suffix = format!("-{display_num}");
        let mut matching = Vec::new();
        let mut others = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if name.ends_with(&suffix) {
                matching.push(path);
            } else if name.contains("-unix-") {
                others.push(path);
            }
        }
        matching.sort();
        others.sort();
        candidates.append(&mut matching);
        candidates.append(&mut others);
    }
    for candidate in candidates {
        if let Ok(content) = std::fs::read_to_string(&candidate)
            && let Some(address) = content
                .lines()
                .find_map(|line| line.strip_prefix("IBUS_ADDRESS="))
        {
            let address = address.trim();
            if !address.is_empty() {
                return Some(address.to_string());
            }
        }
    }
    None
}

/// Letters and digits an input method would normally consume while composing.
fn is_composable_key(keyval: u32) -> bool {
    matches!(keyval, 0x30..=0x39 | 0x41..=0x5a | 0x61..=0x7a)
}

fn ibus_text_string(value: &zvariant::Value) -> Option<String> {
    let mut value = value;
    while let zvariant::Value::Value(inner) = value {
        value = inner;
    }
    let zvariant::Value::Structure(structure) = value else {
        return None;
    };
    match structure.fields().get(2)? {
        zvariant::Value::Str(text) => Some(text.to_string()),
        _ => None,
    }
}

fn event_from_message(
    message: &zbus::Message,
    context_path: &OwnedObjectPath,
) -> Option<IbusEvent> {
    let header = message.header();
    if header.message_type() != zbus::message::Type::Signal {
        return None;
    }
    if header
        .path()
        .is_none_or(|path| path.as_str() != context_path.as_str())
    {
        return None;
    }
    let member = header.member()?;
    let body = message.body();
    match member.as_str() {
        "CommitText" => body
            .deserialize::<zvariant::Value>()
            .ok()
            .and_then(|text| ibus_text_string(&text))
            .map(IbusEvent::Commit),
        "UpdatePreeditText" => body
            .deserialize::<(zvariant::Value, u32, bool)>()
            .ok()
            .and_then(|(text, _caret, visible)| {
                Some(IbusEvent::Preedit {
                    text: ibus_text_string(&text)?,
                    visible,
                })
            }),
        "UpdatePreeditTextWithMode" => body
            .deserialize::<(zvariant::Value, u32, bool, u32)>()
            .ok()
            .and_then(|(text, _caret, visible, _mode)| {
                Some(IbusEvent::Preedit {
                    text: ibus_text_string(&text)?,
                    visible,
                })
            }),
        "HidePreeditText" => Some(IbusEvent::Preedit {
            text: String::new(),
            visible: false,
        }),
        "ForwardKeyEvent" => {
            body.deserialize::<(u32, u32, u32)>()
                .ok()
                .map(|(keyval, keycode, state)| IbusEvent::ForwardKey {
                    keyval,
                    keycode,
                    state,
                })
        }
        _ => None,
    }
}

async fn with_timeout<T>(
    future: impl Future<Output = zbus::Result<T>>,
    timeout: Duration,
) -> anyhow::Result<T> {
    let future = std::pin::pin!(future);
    let timer = std::pin::pin!(smol::Timer::after(timeout));
    match futures::future::select(future, timer).await {
        futures::future::Either::Left((result, _)) => Ok(result?),
        futures::future::Either::Right(_) => anyhow::bail!("ibus call timed out"),
    }
}

/// A call that timed out is recoverable — the engine is just slow, and killing
/// the connection over it would send every following keystroke through as raw
/// text. Only a dead connection is fatal.
enum CallError {
    TimedOut,
    Fatal(anyhow::Error),
}

/// Await a method reply while still draining signals, so preedit/commit events
/// never back up behind an in-flight call and stall the connection.
async fn pump_call(
    call: impl Future<Output = zbus::Result<zbus::Message>>,
    stream: &mut futures::stream::Fuse<zbus::MessageStream>,
    context_path: &OwnedObjectPath,
    sink: &IbusEventSink,
    timeout: Duration,
) -> Result<zbus::Message, CallError> {
    let mut call = std::pin::pin!(futures::FutureExt::fuse(call));
    let mut timer = std::pin::pin!(futures::FutureExt::fuse(smol::Timer::after(timeout)));
    loop {
        futures::select! {
            reply = call => return reply.map_err(|error| CallError::Fatal(error.into())),
            message = stream.next() => match message {
                Some(Ok(message)) => {
                    if let Some(event) = event_from_message(&message, context_path) {
                        sink.push(event);
                    }
                }
                Some(Err(_)) => {}
                None => {
                    return Err(CallError::Fatal(anyhow::anyhow!("ibus connection closed")));
                }
            },
            _ = timer => return Err(CallError::TimedOut),
        }
    }
}

async fn context_call(
    connection: &zbus::Connection,
    path: &OwnedObjectPath,
    method: &str,
    body: &(impl zbus::export::serde::ser::Serialize + zvariant::DynamicType),
    stream: &mut futures::stream::Fuse<zbus::MessageStream>,
    sink: &IbusEventSink,
) -> Result<(), CallError> {
    pump_call(
        connection.call_method(
            Some("org.freedesktop.IBus"),
            path.as_ref(),
            Some("org.freedesktop.IBus.InputContext"),
            method,
            body,
        ),
        stream,
        path,
        sink,
        CALL_TIMEOUT,
    )
    .await?;
    Ok(())
}

fn drain_ready_signals(
    stream: &mut futures::stream::Fuse<zbus::MessageStream>,
    context_path: &OwnedObjectPath,
    sink: &IbusEventSink,
) {
    loop {
        match stream.next().now_or_never() {
            Some(Some(Ok(message))) => {
                if let Some(event) = event_from_message(&message, context_path) {
                    sink.push(event);
                }
            }
            Some(Some(Err(_))) => {}
            Some(None) | None => break,
        }
    }
}

async fn run_ibus(
    address: String,
    request_rx: smol::channel::Receiver<IbusRequest>,
    sink: IbusEventSink,
) -> anyhow::Result<()> {
    let connection = with_timeout(
        async {
            zbus::connection::Builder::address(address.as_str())?
                .build()
                .await
        },
        CALL_TIMEOUT,
    )
    .await?;

    let mut stream = zbus::MessageStream::from(&connection).fuse();

    let reply = with_timeout(
        connection.call_method(
            Some("org.freedesktop.IBus"),
            "/org/freedesktop/IBus",
            Some("org.freedesktop.IBus"),
            "CreateInputContext",
            &("gpui"),
        ),
        CALL_TIMEOUT,
    )
    .await?;
    let context_path: OwnedObjectPath = reply.body().deserialize()?;

    if let Err(CallError::Fatal(error)) = context_call(
        &connection,
        &context_path,
        "SetCapabilities",
        &(IBUS_CAP_PREEDIT_TEXT | IBUS_CAP_FOCUS),
        &mut stream,
        &sink,
    )
    .await
    {
        return Err(error);
    }

    eprintln!("[ibus] connected; input context {}", context_path.as_str());

    enum WorkerEvent {
        Request(IbusRequest),
        Signal(zbus::Message),
        Closed,
    }

    let mut unhandled_keys = 0u8;

    loop {
        // The stream borrow must end before a request is served, so the request
        // handlers can keep draining signals while they await their own reply.
        let next = futures::select! {
            request = request_rx.recv().fuse() => match request {
                Ok(request) => WorkerEvent::Request(request),
                Err(_) => WorkerEvent::Closed,
            },
            message = stream.next() => match message {
                Some(Ok(message)) => WorkerEvent::Signal(message),
                Some(Err(_)) => continue,
                None => anyhow::bail!("ibus connection closed"),
            },
        };

        let request = match next {
            WorkerEvent::Closed => return Ok(()),
            WorkerEvent::Signal(message) => {
                if let Some(event) = event_from_message(&message, &context_path) {
                    sink.push(event);
                }
                continue;
            }
            WorkerEvent::Request(request) => request,
        };

        let result = match request {
            IbusRequest::Key {
                keyval,
                keycode,
                state,
                reply,
            } => {
                let call = pump_call(
                    connection.call_method(
                        Some("org.freedesktop.IBus"),
                        context_path.as_ref(),
                        Some("org.freedesktop.IBus.InputContext"),
                        "ProcessKeyEvent",
                        &(keyval, keycode, state),
                    ),
                    &mut stream,
                    &context_path,
                    &sink,
                    KEY_CALL_TIMEOUT,
                )
                .await;
                drain_ready_signals(&mut stream, &context_path, &sink);
                match call {
                    Ok(message) => {
                        let handled = message.body().deserialize::<bool>().unwrap_or(false);
                        if !handled && is_composable_key(keyval) && unhandled_keys < 5 {
                            unhandled_keys += 1;
                            eprintln!(
                                "[ibus] engine did not take keyval {keyval:#x}; typing it as plain text"
                            );
                        }
                        let _ = reply.send(handled);
                        Ok(())
                    }
                    Err(CallError::TimedOut) => {
                        // Let the key through as plain text this once; the engine
                        // is alive, so the next keystroke can still compose.
                        eprintln!("[ibus] engine did not answer keyval {keyval:#x} in time");
                        let _ = reply.send(false);
                        Ok(())
                    }
                    Err(error) => {
                        let _ = reply.send(false);
                        Err(error)
                    }
                }
            }
            IbusRequest::FocusIn => {
                context_call(
                    &connection,
                    &context_path,
                    "FocusIn",
                    &(),
                    &mut stream,
                    &sink,
                )
                .await
            }
            IbusRequest::FocusOut => {
                context_call(
                    &connection,
                    &context_path,
                    "FocusOut",
                    &(),
                    &mut stream,
                    &sink,
                )
                .await
            }
            IbusRequest::Reset => {
                context_call(&connection, &context_path, "Reset", &(), &mut stream, &sink).await
            }
            IbusRequest::CursorLocation { x, y, w, h } => {
                context_call(
                    &connection,
                    &context_path,
                    "SetCursorLocation",
                    &(x, y, w, h),
                    &mut stream,
                    &sink,
                )
                .await
            }
        };

        match result {
            Ok(()) => {}
            Err(CallError::TimedOut) => {
                eprintln!("[ibus] a request timed out; keeping the connection");
            }
            Err(CallError::Fatal(error)) => anyhow::bail!("ibus request failed: {error}"),
        }
    }
}

pub(crate) fn spawn_ibus_client(sink: IbusEventSink) -> Option<IbusHandle> {
    let address = ibus_address()?;
    let (request_tx, request_rx) = smol::channel::unbounded();
    let dead = Arc::new(AtomicBool::new(false));
    let thread_dead = dead.clone();
    let thread_sink = sink.clone();
    let spawned = std::thread::Builder::new()
        .name("ibus-client".to_string())
        .spawn(move || {
            let result = smol::block_on(run_ibus(address, request_rx, thread_sink.clone()));
            if let Err(error) = result {
                eprintln!("[ibus] client stopped: {error}");
            }
            thread_dead.store(true, Ordering::Relaxed);
            thread_sink.push(IbusEvent::Dead);
        });
    match spawned {
        Ok(_) => Some(IbusHandle { request_tx, dead }),
        Err(error) => {
            eprintln!("[ibus] failed to spawn client thread: {error}");
            None
        }
    }
}
