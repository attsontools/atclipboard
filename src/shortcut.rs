use std::{
    sync::{mpsc, Arc, Mutex},
    thread,
    time::Duration,
};

use global_hotkey::{hotkey::HotKey, GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use tao::{
    event::Event,
    event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy},
    platform::run_return::EventLoopExtRunReturn,
};

use crate::{
    platform::system_shortcut_command, ClipboardShortcutStatus, ServiceError, ServiceRunOptions,
    ServiceServer, SystemClipboardBackend,
};

const SHORTCUT_COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
const INITIAL_REGISTER_ATTEMPTS: usize = 3;
const INITIAL_REGISTER_RETRY_DELAY: Duration = Duration::from_millis(250);
const UNREGISTER_ATTEMPTS: usize = 3;
const UNREGISTER_RETRY_DELAY: Duration = Duration::from_millis(40);

type OpenHistoryHandler = Arc<dyn Fn() + Send + Sync>;

#[derive(Clone)]
pub(crate) struct ShortcutController {
    runtime: ShortcutRuntime,
    status: Arc<Mutex<ClipboardShortcutStatus>>,
    open_history: Arc<Mutex<Option<OpenHistoryHandler>>>,
}

#[derive(Clone)]
enum ShortcutRuntime {
    Active(EventLoopProxy<PlatformEvent>),
    Unavailable,
}

impl ShortcutController {
    pub(crate) fn active(proxy: EventLoopProxy<PlatformEvent>) -> Self {
        Self {
            runtime: ShortcutRuntime::Active(proxy),
            status: Arc::new(Mutex::new(ClipboardShortcutStatus {
                registered: false,
                register_error: None,
                unavailable_reason: None,
                system_shortcut_command: None,
            })),
            open_history: Arc::new(Mutex::new(None)),
        }
    }

    pub(crate) fn unavailable(reason: impl Into<String>, command: Option<String>) -> Self {
        Self {
            runtime: ShortcutRuntime::Unavailable,
            status: Arc::new(Mutex::new(ClipboardShortcutStatus {
                registered: false,
                register_error: None,
                unavailable_reason: Some(reason.into()),
                system_shortcut_command: command,
            })),
            open_history: Arc::new(Mutex::new(None)),
        }
    }

    pub(crate) fn status(&self) -> ClipboardShortcutStatus {
        self.status
            .lock()
            .map(|status| status.clone())
            .unwrap_or_else(|_| ClipboardShortcutStatus {
                registered: false,
                register_error: Some("shortcut state is unavailable".into()),
                unavailable_reason: None,
                system_shortcut_command: None,
            })
    }

    pub(crate) fn replace(&self, previous: &str, next: &str) -> Result<(), String> {
        let ShortcutRuntime::Active(proxy) = &self.runtime else {
            return Err("global shortcuts are unavailable on this platform session".into());
        };
        let (response, result) = mpsc::sync_channel(1);
        proxy
            .send_event(PlatformEvent::Replace {
                previous: previous.to_string(),
                next: next.to_string(),
                response,
            })
            .map_err(|_| "global shortcut runtime is unavailable".to_string())?;
        let result = result
            .recv_timeout(SHORTCUT_COMMAND_TIMEOUT)
            .map_err(|_| "global shortcut registration timed out".to_string())?;
        let was_registered = self.status().registered;
        match &result {
            Ok(()) => self.update_status(true, None),
            Err(_) => self.update_status(was_registered, Some("shortcut is unavailable")),
        }
        result
    }

    pub(crate) fn initialize(&self, shortcut: &str) {
        if matches!(self.runtime, ShortcutRuntime::Unavailable) {
            return;
        }
        for attempt in 0..INITIAL_REGISTER_ATTEMPTS {
            if self.replace("", shortcut).is_ok() {
                return;
            }
            if attempt + 1 < INITIAL_REGISTER_ATTEMPTS {
                thread::sleep(INITIAL_REGISTER_RETRY_DELAY);
            }
        }
    }

    pub(crate) fn set_open_history_handler(&self, handler: OpenHistoryHandler) {
        if let Ok(mut current) = self.open_history.lock() {
            *current = Some(handler);
        }
    }

    fn trigger_open_history(&self) {
        let handler = self
            .open_history
            .lock()
            .ok()
            .and_then(|handler| handler.clone());
        if let Some(handler) = handler {
            handler();
        }
    }

    fn update_status(&self, registered: bool, error: Option<&str>) {
        if let Ok(mut status) = self.status.lock() {
            status.registered = registered;
            status.register_error = error.map(str::to_string);
        }
    }
}

pub fn run_service_process(
    options: ServiceRunOptions,
    service_version: impl Into<String>,
) -> Result<(), ServiceError> {
    let service_version = service_version.into();
    if let Some(reason) = shortcut_unavailable_reason() {
        return run_headless(
            options,
            service_version,
            ShortcutController::unavailable(reason, system_shortcut_command()),
        );
    }

    let mut event_loop = EventLoopBuilder::<PlatformEvent>::with_user_event().build();
    let controller = ShortcutController::active(event_loop.create_proxy());
    let backend = Arc::new(SystemClipboardBackend::with_shortcut(controller.clone()));
    let hotkey_proxy = event_loop.create_proxy();
    GlobalHotKeyEvent::set_event_handler(Some(move |event| {
        let _ = hotkey_proxy.send_event(PlatformEvent::HotKey(event));
    }));
    let exit_proxy = event_loop.create_proxy();
    let server_thread = thread::spawn(move || {
        let result = run_server(options, service_version, backend);
        let _ = exit_proxy.send_event(PlatformEvent::Exit);
        result
    });
    let manager = GlobalHotKeyManager::new().ok();
    let mut registered = RegisteredShortcuts::default();
    event_loop.run_return(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            Event::UserEvent(PlatformEvent::Replace {
                previous,
                next,
                response,
            }) => {
                let result =
                    replace_registered(manager.as_ref(), &mut registered, &previous, &next);
                let _ = response.send(result);
            }
            Event::UserEvent(PlatformEvent::Exit) => {
                *control_flow = ControlFlow::Exit;
            }
            Event::UserEvent(PlatformEvent::HotKey(event)) => {
                if event.state() == HotKeyState::Pressed && registered.matches(event.id()) {
                    controller.trigger_open_history();
                }
            }
            _ => {}
        }
    });
    let thread_result = server_thread.join().map_err(|_| {
        ServiceError::new(
            "CLIPBOARD_SERVICE_RUNTIME_FAILED",
            "clipboard service runtime stopped unexpectedly",
        )
    })?;
    thread_result
}

fn run_headless(
    options: ServiceRunOptions,
    service_version: String,
    controller: ShortcutController,
) -> Result<(), ServiceError> {
    run_server(
        options,
        service_version,
        Arc::new(SystemClipboardBackend::with_shortcut(controller)),
    )
}

fn run_server(
    options: ServiceRunOptions,
    service_version: String,
    backend: Arc<SystemClipboardBackend>,
) -> Result<(), ServiceError> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| {
            ServiceError::new(
                "CLIPBOARD_SERVICE_RUNTIME_FAILED",
                "cannot start clipboard service runtime",
            )
        })?
        .block_on(async move {
            let server = ServiceServer::bind_with_isolated_backend(
                options.endpoint_path,
                options.data_directory,
                service_version,
                backend,
            )
            .await?;
            let stop = server.stop_handle();
            tokio::spawn(async move {
                if tokio::signal::ctrl_c().await.is_ok() {
                    stop.notify_one();
                }
            });
            server.run().await
        })
}

fn replace_registered(
    manager: Option<&impl ShortcutRegistrar>,
    registered: &mut RegisteredShortcuts,
    previous: &str,
    next: &str,
) -> Result<(), String> {
    let manager = manager.ok_or_else(|| "global shortcut manager is unavailable".to_string())?;
    registered.cleanup_pending(manager)?;
    let next = next
        .parse::<HotKey>()
        .map_err(|_| "global shortcut format is invalid".to_string())?;
    if registered
        .current
        .as_ref()
        .is_some_and(|current| current.id() == next.id())
    {
        return Ok(());
    }
    manager.register(next)?;
    if let Some(current) = registered.current.as_mut() {
        let expected = previous.parse::<HotKey>().ok();
        if expected.is_some_and(|expected| expected.id() != current.id())
            || unregister_with_retry(manager, *current).is_err()
        {
            if unregister_with_retry(manager, next).is_err() {
                registered.pending_cleanup.push(next);
                return Err(
                    "previous global shortcut could not be released and candidate cleanup is pending"
                        .into(),
                );
            }
            return Err("previous global shortcut could not be released".into());
        }
        *current = next;
    } else {
        registered.current = Some(next);
    }
    Ok(())
}

#[derive(Default)]
struct RegisteredShortcuts {
    current: Option<HotKey>,
    pending_cleanup: Vec<HotKey>,
}

impl RegisteredShortcuts {
    fn matches(&self, id: u32) -> bool {
        self.current.is_some_and(|hotkey| hotkey.id() == id)
            || self.pending_cleanup.iter().any(|hotkey| hotkey.id() == id)
    }

    fn cleanup_pending(&mut self, manager: &impl ShortcutRegistrar) -> Result<(), String> {
        let mut remaining = Vec::new();
        for shortcut in self.pending_cleanup.drain(..) {
            if unregister_with_retry(manager, shortcut).is_err() {
                remaining.push(shortcut);
            }
        }
        self.pending_cleanup = remaining;
        if self.pending_cleanup.is_empty() {
            Ok(())
        } else {
            Err("stale global shortcut cleanup is still pending".into())
        }
    }
}

fn unregister_with_retry(manager: &impl ShortcutRegistrar, shortcut: HotKey) -> Result<(), String> {
    let mut last_error = None;
    for attempt in 0..UNREGISTER_ATTEMPTS {
        match manager.unregister(shortcut) {
            Ok(()) => return Ok(()),
            Err(error) => {
                last_error = Some(error);
                if attempt + 1 < UNREGISTER_ATTEMPTS {
                    thread::sleep(UNREGISTER_RETRY_DELAY);
                }
            }
        }
    }
    Err(last_error.expect("at least one unregister attempt"))
}

trait ShortcutRegistrar {
    fn register(&self, shortcut: HotKey) -> Result<(), String>;
    fn unregister(&self, shortcut: HotKey) -> Result<(), String>;
}

impl ShortcutRegistrar for GlobalHotKeyManager {
    fn register(&self, shortcut: HotKey) -> Result<(), String> {
        GlobalHotKeyManager::register(self, shortcut)
            .map_err(|_| "global shortcut is already registered".to_string())
    }

    fn unregister(&self, shortcut: HotKey) -> Result<(), String> {
        GlobalHotKeyManager::unregister(self, shortcut)
            .map_err(|_| "global shortcut could not be released".to_string())
    }
}

#[cfg(target_os = "linux")]
fn shortcut_unavailable_reason() -> Option<&'static str> {
    let session_type = std::env::var("XDG_SESSION_TYPE").ok();
    let wayland = std::env::var_os("WAYLAND_DISPLAY");
    (session_type
        .as_deref()
        .is_some_and(|value| value.eq_ignore_ascii_case("wayland"))
        || wayland.is_some())
    .then_some("global shortcuts are unavailable in this Wayland session")
}

#[cfg(not(target_os = "linux"))]
fn shortcut_unavailable_reason() -> Option<&'static str> {
    None
}

#[derive(Debug)]
pub(crate) enum PlatformEvent {
    Replace {
        previous: String,
        next: String,
        response: mpsc::SyncSender<Result<(), String>>,
    },
    HotKey(GlobalHotKeyEvent),
    Exit,
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, sync::Mutex};

    use global_hotkey::hotkey::HotKey;

    use super::{replace_registered, RegisteredShortcuts, ShortcutRegistrar};

    #[test]
    fn replacement_registers_the_new_shortcut_before_releasing_the_old_one() {
        let registrar = FakeRegistrar::default();
        let old = parse("CommandOrControl+Alt+V");
        let mut registered = RegisteredShortcuts {
            current: Some(old),
            ..RegisteredShortcuts::default()
        };

        replace_registered(
            Some(&registrar),
            &mut registered,
            "CommandOrControl+Alt+V",
            "CommandOrControl+Shift+X",
        )
        .unwrap();

        let next = parse("CommandOrControl+Shift+X");
        assert_eq!(registered.current, Some(next));
        assert_eq!(
            *registrar.operations.lock().unwrap(),
            [
                format!("register:{}", next.id()),
                format!("unregister:{}", old.id())
            ]
        );
    }

    #[test]
    fn registration_conflict_keeps_the_previous_shortcut_active() {
        let registrar = FakeRegistrar {
            fail_register: true,
            ..FakeRegistrar::default()
        };
        let old = parse("CommandOrControl+Alt+V");
        let next = parse("CommandOrControl+Shift+X");
        let mut registered = RegisteredShortcuts {
            current: Some(old),
            ..RegisteredShortcuts::default()
        };

        assert!(replace_registered(
            Some(&registrar),
            &mut registered,
            "CommandOrControl+Alt+V",
            "CommandOrControl+Shift+X",
        )
        .is_err());

        assert_eq!(registered.current, Some(old));
        assert_eq!(
            *registrar.operations.lock().unwrap(),
            [format!("register:{}", next.id())]
        );
    }

    #[test]
    fn persistent_old_release_failure_unregisters_the_candidate_and_keeps_the_old_state() {
        let registrar = FakeRegistrar::default();
        let old = parse("CommandOrControl+Alt+V");
        let next = parse("CommandOrControl+Shift+X");
        registrar.fail_unregister(old, 3);
        let mut registered = RegisteredShortcuts {
            current: Some(old),
            ..RegisteredShortcuts::default()
        };

        assert!(replace_registered(
            Some(&registrar),
            &mut registered,
            "CommandOrControl+Alt+V",
            "CommandOrControl+Shift+X",
        )
        .is_err());

        assert_eq!(registered.current, Some(old));
        assert!(registered.pending_cleanup.is_empty());
        assert_eq!(
            *registrar.operations.lock().unwrap(),
            [
                format!("register:{}", next.id()),
                format!("unregister:{}", old.id()),
                format!("unregister:{}", old.id()),
                format!("unregister:{}", old.id()),
                format!("unregister:{}", next.id()),
            ]
        );
    }

    #[test]
    fn failed_candidate_cleanup_remains_dispatchable_and_is_cleaned_before_the_next_change() {
        let registrar = FakeRegistrar::default();
        let old = parse("CommandOrControl+Alt+V");
        let candidate = parse("CommandOrControl+Shift+X");
        let replacement = parse("CommandOrControl+Shift+Y");
        registrar.fail_unregister(old, 3);
        registrar.fail_unregister(candidate, 3);
        let mut registered = RegisteredShortcuts {
            current: Some(old),
            ..RegisteredShortcuts::default()
        };

        assert!(replace_registered(
            Some(&registrar),
            &mut registered,
            "CommandOrControl+Alt+V",
            "CommandOrControl+Shift+X",
        )
        .is_err());
        assert!(registered.matches(old.id()));
        assert!(registered.matches(candidate.id()));

        replace_registered(
            Some(&registrar),
            &mut registered,
            "CommandOrControl+Alt+V",
            "CommandOrControl+Shift+Y",
        )
        .unwrap();
        assert_eq!(registered.current, Some(replacement));
        assert!(registered.pending_cleanup.is_empty());
        assert!(!registered.matches(candidate.id()));
    }

    #[derive(Default)]
    struct FakeRegistrar {
        operations: Mutex<Vec<String>>,
        fail_register: bool,
        unregister_failures: Mutex<HashMap<u32, usize>>,
    }

    impl FakeRegistrar {
        fn fail_unregister(&self, shortcut: HotKey, attempts: usize) {
            self.unregister_failures
                .lock()
                .unwrap()
                .insert(shortcut.id(), attempts);
        }
    }

    impl ShortcutRegistrar for FakeRegistrar {
        fn register(&self, shortcut: HotKey) -> Result<(), String> {
            self.operations
                .lock()
                .unwrap()
                .push(format!("register:{}", shortcut.id()));
            if self.fail_register {
                Err("conflict".into())
            } else {
                Ok(())
            }
        }

        fn unregister(&self, shortcut: HotKey) -> Result<(), String> {
            self.operations
                .lock()
                .unwrap()
                .push(format!("unregister:{}", shortcut.id()));
            let mut failures = self.unregister_failures.lock().unwrap();
            if let Some(remaining) = failures.get_mut(&shortcut.id()) {
                if *remaining > 0 {
                    *remaining -= 1;
                    return Err("release failed".into());
                }
            }
            Ok(())
        }
    }

    fn parse(value: &str) -> HotKey {
        value.parse().unwrap()
    }
}
