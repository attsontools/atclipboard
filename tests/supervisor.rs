use std::{
    io,
    sync::{
        atomic::{AtomicI32, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use atclipboard_service::{
    ServiceLaunchSpec, ServiceLauncher, ServiceProcess, ServiceServer, ServiceSupervisor,
    ServiceSupervisorError,
};
use serde_json::json;

const RUNNING: i32 = i32::MIN;

#[derive(Default)]
struct ServerLauncher {
    launches: AtomicUsize,
    tasks: Mutex<Vec<tokio::task::AbortHandle>>,
}

impl ServiceLauncher for ServerLauncher {
    fn launch(
        &self,
        spec: &ServiceLaunchSpec,
    ) -> Result<Box<dyn ServiceProcess>, ServiceSupervisorError> {
        self.launches.fetch_add(1, Ordering::SeqCst);
        let exit_code = Arc::new(AtomicI32::new(RUNNING));
        let task_exit_code = Arc::clone(&exit_code);
        let endpoint = spec.endpoint_path.clone();
        let data = spec.data_directory.clone();
        let version = spec.service_version.clone();
        let task = tokio::spawn(async move {
            let result = match ServiceServer::bind(endpoint, data, version).await {
                Ok(server) => server.run().await,
                Err(error) => Err(error),
            };
            task_exit_code.store(if result.is_ok() { 0 } else { 1 }, Ordering::SeqCst);
        });
        self.tasks.lock().unwrap().push(task.abort_handle());
        Ok(Box::new(TestProcess { exit_code }))
    }
}

impl Drop for ServerLauncher {
    fn drop(&mut self) {
        for task in self.tasks.lock().unwrap().drain(..) {
            task.abort();
        }
    }
}

struct TestProcess {
    exit_code: Arc<AtomicI32>,
}

impl ServiceProcess for TestProcess {
    fn try_exit(&mut self) -> io::Result<Option<i32>> {
        let code = self.exit_code.load(Ordering::SeqCst);
        Ok((code != RUNNING).then_some(code))
    }
}

#[tokio::test]
async fn concurrent_supervisors_share_the_single_service_lease_winner() {
    let temporary = tempfile::tempdir().unwrap();
    let data = temporary.path().join("clipboard");
    let endpoint = data.join("service.endpoint.json");
    let launcher = Arc::new(ServerLauncher::default());
    let mut first = supervisor(&endpoint, &data, launcher.clone());
    let mut second = supervisor(&endpoint, &data, launcher.clone());

    let (first, second) = tokio::join!(first.connect(), second.connect());
    let first = first.unwrap();
    let second = second.unwrap();

    assert_eq!(launcher.launches.load(Ordering::SeqCst), 2);
    assert_eq!(
        first
            .request("clipboard.history.list", json!({}))
            .await
            .unwrap(),
        json!({ "items": [], "nextCursor": null })
    );
    assert_eq!(
        second
            .request("clipboard.history.list", json!({}))
            .await
            .unwrap(),
        json!({ "items": [], "nextCursor": null })
    );
}

fn supervisor(
    endpoint: &std::path::Path,
    data: &std::path::Path,
    launcher: Arc<dyn ServiceLauncher>,
) -> ServiceSupervisor {
    ServiceSupervisor::with_launcher(
        endpoint.to_path_buf(),
        data.to_path_buf(),
        "supervisor-test",
        env!("CARGO_PKG_VERSION"),
        Duration::from_secs(2),
        launcher,
    )
}
