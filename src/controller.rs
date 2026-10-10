//! Client state shared by the tray, the windows and the status page: the
//! connection to the local instance, pushed snapshots and running tasks.
use crate::{
    i18n::Language,
    local::{self, Detected, Log},
    model::{self, Action, Live, MenuModel, Service, Snapshot, Task, Traffic},
    notify, settings, tray, window,
};
use anyhow::{Context as _, Result};
use management_client::{Api, Endpoint, events::Feed};
use serde_json::{Value, json};
use std::{
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};
use tauri::{AppHandle, Manager as _};
use tokio::sync::{Notify, watch};

const READ_TIMEOUT: Duration = Duration::from_secs(10);
/// Delay tests, downloads and restarts are bounded by the service itself.
const ACTION_TIMEOUT: Duration = Duration::from_secs(180);
/// Core samples normally arrive each second. Silence is unknown, not zero.
const TRAFFIC_FRESHNESS: Duration = Duration::from_secs(3);

/// Never implements Debug: it holds the management token.
pub struct Connection {
    pub api: Api,
    pub endpoint: Endpoint,
    pub token: String,
    pub service_version: Option<String>,
}

struct Inner {
    /// The instance's shared interface language, else the saved one, else the
    /// system locale.
    language: Language,
    /// The saved copy of the shared language (see `settings`).
    saved: Option<Language>,
    /// The status page's temporary proxy for installation, never saved.
    proxy: Option<String>,
    service: Service,
    /// Whether the detected-but-stopped unit is enabled at boot.
    enabled: bool,
    traffic: Option<Traffic>,
    task: Task,
    last_error: Option<String>,
    /// The install or lifecycle task that failed last, until the next task.
    failed: Option<Task>,
}

pub struct Controller {
    system_language: Language,
    settings: settings::Store,
    inner: Mutex<Inner>,
    wake: Notify,
    connections: watch::Sender<Option<Arc<Connection>>>,
    pub log: Log,
}

impl Controller {
    pub fn new(system_language: Language, settings: settings::Store) -> Self {
        let saved = settings.language();
        Self {
            system_language,
            settings,
            inner: Mutex::new(Inner {
                language: saved.unwrap_or(system_language),
                saved,
                proxy: None,
                service: Service::Detecting,
                enabled: false,
                traffic: None,
                task: Task::Idle,
                last_error: None,
                failed: None,
            }),
            wake: Notify::new(),
            connections: watch::channel(None).0,
            log: Log::default(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn snapshot(&self) -> Snapshot {
        let inner = self.lock();
        Snapshot {
            service: inner.service.clone(),
            task: inner.task,
            last_error: inner.last_error.clone(),
            traffic: inner.traffic,
        }
    }

    pub fn connection(&self) -> Option<Arc<Connection>> {
        self.connections.borrow().clone()
    }

    pub fn task(&self) -> Task {
        self.lock().task
    }

    pub fn language(&self) -> Language {
        self.lock().language
    }

    /// Follow the instance's preference (`None`: the system locale) and keep
    /// a copy of it; whether the language changed.
    fn set_language(&self, preference: Option<Language>) -> bool {
        let language = preference.unwrap_or(self.system_language);
        let (changed, save) = {
            let mut inner = self.lock();
            let save = std::mem::replace(&mut inner.saved, preference) != preference;
            (std::mem::replace(&mut inner.language, language) != language, save)
        };
        if save && let Err(error) = self.settings.save_language(preference) {
            eprintln!("cannot save the interface language: {error:#}");
        }
        changed
    }

    fn saved_language(&self) -> Option<Language> {
        self.lock().saved
    }

    pub fn proxy(&self) -> Option<String> {
        self.lock().proxy.clone()
    }

    pub fn set_proxy(&self, proxy: Option<String>) {
        self.lock().proxy = proxy;
    }

    pub fn failed(&self) -> Option<Task> {
        self.lock().failed
    }

    /// Retry instance discovery now instead of after the reconnect delay.
    pub fn wake(&self) {
        self.wake.notify_one();
    }

    fn set_service(&self, service: Service) {
        let mut inner = self.lock();
        if !matches!(&service, Service::Running(live) if live.status["phase"] == "running") {
            inner.traffic = None;
        }
        inner.service = service;
    }

    fn apply_state(&self, connection: &Arc<Connection>, data: &Value) -> Result<bool> {
        anyhow::ensure!(
            data["status"]["phase"].is_string() && data["profiles"].is_object(),
            "invalid service state snapshot"
        );
        let mut inner = self.lock();
        if !self
            .connection()
            .is_some_and(|current| Arc::ptr_eq(&current, connection))
        {
            return Ok(false);
        }
        let same_core = matches!(&inner.service, Service::Running(live)
            if live.status["generation"] == data["status"]["generation"]
                && live.status["phase"] == "running" && data["status"]["phase"] == "running");
        if !same_core {
            inner.traffic = None;
        }
        inner.service = Service::Running(Box::new(Live {
            service_version: connection.service_version.clone(),
            status: data["status"].clone(),
            access: data["access"].clone(),
            user: data["user"].clone(),
            proxies: data["proxies"].clone(),
            profiles: data["profiles"].clone(),
        }));
        Ok(true)
    }

    fn drop_connection(&self) {
        let mut inner = self.lock();
        self.connections.send_replace(None);
        inner.traffic = None;
    }

    /// Ignore frames from an obsolete endpoint/token after reconnecting.
    fn set_traffic(&self, connection: &Arc<Connection>, generation: Option<u64>, traffic: Option<Traffic>) -> bool {
        let mut inner = self.lock();
        if !self
            .connection()
            .is_some_and(|current| Arc::ptr_eq(&current, connection))
        {
            return false;
        }
        let traffic = traffic.filter(|_| {
            matches!(&inner.service,
            Service::Running(live) if live.status["phase"] == "running"
                && generation.is_some() && live.status["generation"].as_u64() == generation)
        });
        std::mem::replace(&mut inner.traffic, traffic) != traffic
    }

    pub fn model(&self) -> MenuModel {
        model::derive(&self.snapshot(), self.language().strings())
    }
}

fn controller(app: &AppHandle) -> Arc<Controller> {
    app.state::<Arc<Controller>>().inner().clone()
}

/// Recompute the menu; the tray applies only what changed.
pub fn publish(app: &AppHandle) {
    tray::apply(app, controller(app).model());
}

fn short(error: &anyhow::Error) -> String {
    format!("{error:#}")
}

async fn timed<T>(future: impl Future<Output = Result<T>>) -> Result<T> {
    tokio::time::timeout(READ_TIMEOUT, future)
        .await
        .context("the management API did not answer in time")?
}

/// Discovery is retried only while disconnected; live facts arrive via feeds.
#[derive(Default)]
struct Monitor {
    failures: u32,
}

impl Monitor {
    async fn connect(&self, controller: &Controller) -> Result<Option<Arc<Connection>>> {
        let detected = tokio::task::spawn_blocking(local::detect).await??;
        let (endpoint, service_version) = match detected {
            Detected::Running { endpoint, version } => (endpoint, version),
            Detected::Inactive { enabled } => {
                let mut inner = controller.lock();
                inner.service = Service::Inactive;
                inner.enabled = enabled;
                return Ok(None);
            }
            Detected::NotInstalled => {
                controller.set_service(Service::NotInstalled);
                return Ok(None);
            }
        };
        let token = local::read_token(&endpoint)?;
        let api = Api::with_token(endpoint.clone(), token.clone())?.with_timeout(ACTION_TIMEOUT);
        // Adopt the instance's language before the first menu is shown; older
        // services have no preferences and keep the saved or system language.
        // A language chosen while no instance ran becomes the instance's.
        if let Ok(preferences) = timed(api.command("preferences", json!({}))).await {
            let mut shared = preference(&preferences);
            if shared.is_none()
                && let Some(saved) = controller.saved_language()
                && timed(api.command("set_language", json!({"language": saved.code()})))
                    .await
                    .is_ok()
            {
                shared = Some(saved);
            }
            controller.set_language(shared);
        }
        let api = api.with_language(controller.language().code());
        let connection = Arc::new(Connection {
            api,
            endpoint,
            token,
            service_version,
        });
        controller.connections.send_replace(Some(connection.clone()));
        Ok(Some(connection))
    }

    fn backoff(&mut self) -> Duration {
        self.failures = (self.failures + 1).min(5);
        Duration::from_secs(2u64.pow(self.failures).min(30))
    }
}

/// Discover/reconnect the local instance; live readings are entirely pushed.
pub async fn run(app: AppHandle, open_window: bool) {
    let controller = controller(&app);
    let mut monitor = Monitor::default();
    let mut first = open_window;
    loop {
        let result = monitor.connect(&controller).await;
        publish(&app);
        if std::mem::take(&mut first) {
            window::open_preferred(&app);
        }
        let delay = match result {
            Ok(Some(connection)) => match follow_state(&app, &controller, &connection, &mut monitor).await {
                Ok(()) => continue,
                Err(error) => {
                    controller.drop_connection();
                    let reason = short(&error);
                    let repeated = matches!(&controller.lock().service,
                            Service::Unreachable(previous) if *previous == reason);
                    if !repeated {
                        controller.log.push(format!("error: {reason}"));
                    }
                    controller.set_service(Service::Unreachable(reason));
                    monitor.backoff()
                }
            },
            Ok(None) => {
                monitor.failures = 0;
                Duration::from_secs(if controller.task() == Task::Idle { 5 } else { 2 })
            }
            Err(error) => {
                controller.drop_connection();
                controller.set_service(Service::Unreachable(short(&error)));
                monitor.backoff()
            }
        };
        publish(&app);
        tokio::select! {
            () = tokio::time::sleep(delay) => {}
            () = controller.wake.notified() => {}
        }
    }
}

async fn follow_state(
    app: &AppHandle,
    controller: &Controller,
    connection: &Arc<Connection>,
    monitor: &mut Monitor,
) -> Result<()> {
    let mut connections = controller.connections.subscribe();
    let current = connections.borrow_and_update().clone();
    if !current.is_some_and(|current| Arc::ptr_eq(&current, connection)) {
        return Ok(());
    }
    let mut feed = tokio::select! {
        _ = connections.changed() => return Ok(()),
        feed = Feed::connect(&connection.endpoint, &connection.token, Some("state")) =>
            feed.context("cannot subscribe to service state (requires mihomo-server with /api/streams/state support)")?,
    };
    let mut initial = true;
    loop {
        let event = tokio::select! {
            biased;
            _ = connections.changed() => return Ok(()),
            event = async {
                if initial {
                    timed(feed.next()).await
                } else {
                    feed.next().await
                }
            } => event?.context("service state feed closed")?,
        };
        if event["type"] == "state" {
            if controller.apply_state(connection, &event["data"])? {
                initial = false;
                monitor.failures = 0;
                adopt_language(app, preference(&event["data"]["preferences"]));
                publish(app);
            }
        } else if event["type"] == "error" {
            anyhow::bail!("service state feed failed: {}", event["message"]);
        }
    }
}

/// Independent traffic subscription: a stream failure clears rates without
/// discarding the service's state. Connection changes cancel all old reads.
pub async fn follow_traffic(app: AppHandle) {
    let controller = controller(&app);
    let mut connections = controller.connections.subscribe();
    let mut failures = 0u32;
    loop {
        let connection = connections.borrow_and_update().clone();
        let Some(connection) = connection else {
            if connections.changed().await.is_err() {
                return;
            }
            failures = 0;
            continue;
        };
        tokio::select! {
            biased;
            _ = connections.changed() => { failures = 0; }
            result = traffic_session(&app, &controller, &connection, &mut failures) => {
                if controller.set_traffic(&connection, None, None) { publish(&app); }
                failures = if result.is_err() { (failures + 1).min(5) } else { 0 };
                tokio::select! {
                    _ = connections.changed() => { failures = 0; }
                    _ = tokio::time::sleep(Duration::from_secs(2u64.pow(failures).min(30))) => {}
                }
            }
        }
    }
}

async fn traffic_session(
    app: &AppHandle,
    controller: &Controller,
    connection: &Arc<Connection>,
    failures: &mut u32,
) -> Result<()> {
    let mut feed = Feed::connect(&connection.endpoint, &connection.token, Some("traffic")).await?;
    let mut expires = None;
    let mut generation = None;
    loop {
        tokio::select! {
            event = feed.next() => {
                let event = event?.context("traffic feed closed")?;
                let traffic = match event["type"].as_str() {
                    Some("data") => {
                        let traffic = Traffic::read(&event["data"]);
                        if traffic.is_some() { *failures = 0; }
                        traffic
                    },
                    Some("core_state") => {
                        generation = event["data"]["generation"].as_u64();
                        None
                    },
                    Some("stream_error") => None,
                    Some("error") => anyhow::bail!("traffic feed failed"),
                    _ => continue,
                };
                expires = traffic.map(|_| tokio::time::Instant::now() + TRAFFIC_FRESHNESS);
                if controller.set_traffic(connection, generation, traffic) { publish(app); }
            }
            _ = async { tokio::time::sleep_until(expires.expect("expiry enabled")).await }, if expires.is_some() => {
                expires = None;
                if controller.set_traffic(connection, None, None) { publish(app); }
            }
        }
    }
}

/// The language in a `preferences` value; unset or unknown is `None`.
fn preference(preferences: &Value) -> Option<Language> {
    preferences
        .get("language")
        .and_then(Value::as_str)
        .and_then(Language::from_code)
}

fn adopt_language(app: &AppHandle, preference: Option<Language>) {
    let controller = controller(app);
    if controller.set_language(preference) {
        // Service messages follow too: reconnect with the new Accept-Language.
        controller.drop_connection();
        publish(app);
        controller.wake();
    }
}

/// The status page's language choice: the instance's shared preference when
/// one runs (as the Web UI's settings do), else saved until one does.
pub async fn choose_language(app: &AppHandle, language: Language) -> Result<()> {
    if let Some(connection) = controller(app).connection() {
        timed(
            connection
                .api
                .command("set_language", json!({"language": language.code()})),
        )
        .await?;
    }
    adopt_language(app, Some(language));
    Ok(())
}

fn command(action: &Action) -> Option<(&'static str, Value)> {
    Some(match action {
        Action::Mode(mode) => ("set_proxy_mode", json!({"mode": mode})),
        Action::Tun(enabled) => ("set_tun_enabled", json!({"enabled": enabled})),
        Action::Select { group, node } => ("select_node", json!({"group": group, "node": node})),
        Action::Unfix { group } => ("unfix_node", json!({"group": group})),
        Action::TestGroup { group } => ("delay_group", json!({"group": group})),
        Action::Profile { uid } => ("select_profile", json!({"uid": uid})),
        _ => return None,
    })
}

/// Start one background task; refuses (returns false) while another runs.
pub fn start_task(app: &AppHandle, task: Task, action: Option<Action>) -> bool {
    let controller = controller(app);
    {
        let mut inner = controller.lock();
        if inner.task != Task::Idle {
            return false;
        }
        inner.task = task;
        inner.last_error = None;
        inner.failed = None;
    }
    publish(app);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let controller = self::controller(&app);
        let result = match task {
            Task::Installing => local::install(&controller.log, controller.proxy().as_deref()).await,
            // A never-enabled instance is enabled: started now and at boot.
            Task::Starting => {
                let verb = if controller.lock().enabled { "start" } else { "enable" };
                local::run_helper(verb, &controller.log).await
            }
            Task::Stopping => local::run_helper("stop", &controller.log).await,
            Task::Restarting => local::run_helper("restart", &controller.log).await,
            Task::Command => run_command(&controller, action).await,
            Task::Idle => Ok(()),
        };
        {
            let mut inner = controller.lock();
            inner.task = Task::Idle;
            if task != Task::Command {
                // Installation and service lifecycle change the endpoint and
                // token; detect again instead of showing the state from before.
                controller.connections.send_replace(None);
                inner.traffic = None;
                inner.service = Service::Detecting;
            }
            if let Err(error) = &result {
                inner.last_error = Some(short(error));
                inner.failed = (task != Task::Command).then_some(task);
            }
        }
        if let Err(error) = &result {
            controller.log.push(format!("error: {}", short(error)));
        }
        publish(&app);
        controller.wake();
        if let Err(error) = &result {
            notify::failure(controller.language().strings().failed, &short(error)).await;
        }
    });
    true
}

async fn run_command(controller: &Controller, action: Option<Action>) -> Result<()> {
    let (name, fields) = action.as_ref().and_then(command).context("not a service command")?;
    let connection = controller.connection().context("mihomo-server is not connected")?;
    connection.api.command(name, fields).await.map(drop)
}

/// Run a tray action.
pub fn dispatch(app: &AppHandle, action: Action) {
    match action {
        Action::Quit => app.exit(0),
        // Finish cleanup before replacing the process through its launcher.
        Action::RestartClient => app.exit(crate::relaunch::EXIT_CODE),
        Action::OpenDashboard => window::open_dashboard(app),
        Action::OpenServicePage => window::open_service_page(app),
        Action::StartService => {
            start_task(app, Task::Starting, None);
        }
        Action::StopService => {
            start_task(app, Task::Stopping, None);
        }
        Action::RestartService => {
            start_task(app, Task::Restarting, None);
        }
        action => {
            start_task(app, Task::Command, Some(action));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connection(token: &str) -> Arc<Connection> {
        let endpoint = Endpoint::new("127.0.0.1:9090".parse().unwrap(), None, "/tmp/unused-token".into()).unwrap();
        Arc::new(Connection {
            api: Api::with_token(endpoint.clone(), token.into()).unwrap(),
            endpoint,
            token: token.into(),
            service_version: Some("0.3.2".into()),
        })
    }

    fn state(phase: &str, generation: u64) -> Value {
        json!({
            "status": {"phase": phase, "generation": generation},
            "access": {"running": phase == "running"},
            "profiles": {"current": "R1", "items": [{"uid": "R1", "type": "remote", "name": "Live"}]},
            "proxies": {"proxies": {"Main": {"type": "Selector", "now": "DIRECT", "all": ["DIRECT"]}}},
            "user": null,
        })
    }

    #[test]
    fn pushed_state_replaces_readings_and_clears_rates_on_core_generation_changes() {
        let controller = Controller::new(Language::En, settings::Store::memory());
        let connection = connection("token");
        controller.connections.send_replace(Some(connection.clone()));
        assert!(controller.apply_state(&connection, &state("running", 1)).unwrap());
        let traffic = Some(Traffic { up: 1024, down: 2048 });
        assert!(controller.set_traffic(&connection, Some(1), traffic));
        assert!(controller.apply_state(&connection, &state("running", 1)).unwrap());
        assert_eq!(
            controller.snapshot().traffic,
            traffic,
            "state updates retain fresh rates"
        );
        assert!(controller.apply_state(&connection, &state("running", 2)).unwrap());
        assert_eq!(
            controller.snapshot().traffic,
            None,
            "a new core cannot retain old rates"
        );
        assert!(
            !controller.set_traffic(&connection, Some(1), traffic),
            "late samples from the previous core are rejected"
        );
        assert!(controller.set_traffic(&connection, Some(2), traffic));
        controller.apply_state(&connection, &state("stopped", 2)).unwrap();
        assert_eq!(controller.snapshot().traffic, None);
        assert!(
            !controller.set_traffic(&connection, Some(1), traffic),
            "stopped cores reject late samples"
        );
    }

    #[test]
    fn disconnect_and_token_rotation_discard_stale_frames() {
        let controller = Controller::new(Language::En, settings::Store::memory());
        let old = connection("old");
        controller.connections.send_replace(Some(old.clone()));
        controller.apply_state(&old, &state("running", 1)).unwrap();
        let traffic = Some(Traffic { up: 10, down: 20 });
        controller.set_traffic(&old, Some(1), traffic);
        controller.drop_connection();
        assert_eq!(controller.snapshot().traffic, None);
        let new = connection("rotated");
        controller.connections.send_replace(Some(new.clone()));
        assert!(!controller.apply_state(&old, &state("failed", 1)).unwrap());
        assert!(!controller.set_traffic(&old, Some(1), traffic));
        assert!(controller.apply_state(&new, &state("running", 2)).unwrap());
        assert!(controller.set_traffic(&new, Some(2), traffic));
        assert!(controller.apply_state(&new, &Value::Null).is_err());
        assert_eq!(controller.snapshot().traffic, traffic);
    }

    #[test]
    fn tray_actions_map_to_management_commands() {
        assert_eq!(
            command(&Action::Mode("global")),
            Some(("set_proxy_mode", json!({"mode": "global"})))
        );
        assert_eq!(
            command(&Action::Select {
                group: "Proxies".into(),
                node: "A".into()
            }),
            Some(("select_node", json!({"group": "Proxies", "node": "A"})))
        );
        assert_eq!(
            command(&Action::Tun(true)),
            Some(("set_tun_enabled", json!({"enabled": true})))
        );
        assert_eq!(
            command(&Action::RestartService),
            None,
            "service lifecycle is not an API command"
        );
        assert_eq!(command(&Action::Quit), None);
        assert_eq!(command(&Action::RestartClient), None);
    }

    #[test]
    fn the_instance_language_wins_and_unset_falls_back_to_the_system() {
        let controller = Controller::new(Language::En, settings::Store::memory());
        assert!(controller.set_language(preference(&json!({"language": "zhtw"}))));
        assert_eq!(controller.language(), Language::Zhtw);
        assert!(
            !controller.set_language(preference(&json!({"language": "zhtw"}))),
            "unchanged"
        );
        assert!(controller.set_language(preference(&json!({"language": null}))));
        assert_eq!(controller.language(), Language::En);
        assert!(!controller.set_language(preference(&json!({"language": "klingon"}))));
    }

    #[test]
    fn the_shared_language_is_saved_for_the_next_start() {
        let directory = std::env::temp_dir().join(format!("mihomo-desktop-controller-{}", std::process::id()));
        let path = directory.join("settings.json");
        let controller = Controller::new(Language::En, settings::Store::at(path.clone()));
        assert_eq!(
            (controller.language(), controller.saved_language()),
            (Language::En, None)
        );
        controller.set_language(Some(Language::Zh));
        let restarted = Controller::new(Language::En, settings::Store::at(path.clone()));
        assert_eq!(
            (restarted.language(), restarted.saved_language()),
            (Language::Zh, Some(Language::Zh))
        );
        // A cleared preference falls back to the system locale, also after a restart.
        restarted.set_language(None);
        let cleared = Controller::new(Language::En, settings::Store::at(path));
        assert_eq!((cleared.language(), cleared.saved_language()), (Language::En, None));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn failures_back_off_to_thirty_seconds() {
        let mut poll = Monitor::default();
        let delays: Vec<u64> = (0..7).map(|_| poll.backoff().as_secs()).collect();
        assert_eq!(delays, [2, 4, 8, 16, 30, 30, 30]);
    }
}
