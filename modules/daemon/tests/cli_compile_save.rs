use metteur_cli::commands::{self, Outcome, SessionState};
use metteur_daemon::{
    grpc::{AppState, DaemonService},
    registry::Registry,
    storage::{blueprint_files, persistence::cf},
    workspace::WorkspaceManager,
};
use metteur_proto::proto::{self, daemon_client::DaemonClient};
use std::{path::PathBuf, sync::Arc};
use tonic::transport::{Channel, Server};
use uuid::Uuid;

const SOURCE: &str = "blueprint \"CLI saved\"\nentry s: Start\ne: End\ns -> e\n";

struct Fixture {
    client: DaemonClient<Channel>,
    state: SessionState,
    app: Arc<AppState>,
    root: PathBuf,
    server: tokio::task::JoinHandle<()>,
}
impl Fixture {
    async fn new() -> Self {
        let root = std::env::temp_dir().join(format!("cli-save-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let app = Arc::new(AppState::new(
            WorkspaceManager::new().with_global_config_path(root.join("global.toml")),
            Arc::new(Registry::with_builtins()),
            Default::default(),
        ));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let service = DaemonService::new(app.clone());
        let server = tokio::spawn(async move {
            Server::builder()
                .add_service(proto::daemon_server::DaemonServer::new(service))
                .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
                .await
                .unwrap();
        });
        let mut this = Self {
            client: DaemonClient::connect(address).await.unwrap(),
            state: SessionState::default(),
            app,
            root,
            server,
        };
        this.command(&format!("open {}", this.root.display())).await.unwrap();
        this
    }
    async fn command(&mut self, text: &str) -> Result<String, String> {
        let command = commands::parse(text)?;
        match commands::dispatch(&mut self.client, &mut self.state, command).await {
            Ok(Outcome::Printed(text)) => Ok(text),
            Ok(_) => Err("Expected a printed result".into()),
            Err(error) => Err(format!("{error:#}")),
        }
    }
    async fn compile(&mut self, name: &str, suffix: &str) -> Result<String, String> {
        self.command(&format!("bp compile {}{suffix}", self.root.join(name).display())).await
    }
    async fn close(mut self) {
        self.command(&format!("close {}", self.root.display())).await.unwrap();
        self.server.abort();
        let _ = self.server.await;
        drop(self.app);
        std::fs::remove_dir_all(self.root).unwrap();
    }
}

#[tokio::test]
async fn cli_print_save_and_save_as_use_authoritative_files_mirrors_and_versions() {
    let mut f = Fixture::new().await;
    std::fs::write(f.root.join("plan.mbp"), SOURCE).unwrap();
    let printed = f.compile("plan.mbp", "").await.unwrap();
    let bare = blueprint_files::decode(printed.as_bytes()).unwrap();
    assert!(!f.root.join("plan.mbp.blueprint").exists());
    let ws = f.app.workspaces.get(&f.root).await.unwrap();
    assert!(ws.db.get(cf::BLUEPRINTS, bare.id.as_bytes()).unwrap().is_none());
    let saved = f.compile("plan.mbp", " save").await.unwrap();
    let bytes = std::fs::read(f.root.join("plan.mbp.blueprint")).unwrap();
    let graph = blueprint_files::decode(&bytes).unwrap();
    assert!(saved.contains(&graph.id.to_string()));
    assert_ne!(bare.id, graph.id);
    let mirrored =
        blueprint_files::decode(&ws.db.get(cf::BLUEPRINTS, graph.id.as_bytes()).unwrap().unwrap())
            .unwrap();
    assert_eq!(mirrored, graph);
    let version = blueprint_files::binding(&ws.db, graph.id).unwrap().unwrap();
    ws.version_manager.verify_blueprint(&version).unwrap();
    let history = f
        .client
        .get_file_history(proto::GetFileHistoryRequest {
            workspace_path: f.root.to_string_lossy().into(),
            path: "plan.mbp.blueprint".into(),
        })
        .await
        .unwrap()
        .into_inner();
    assert!(
        history.entries.iter().any(|v| v.snapshot_id == version.snapshot_id.to_string()
            && v.content_hash == version.blob_hash)
    );
    let loaded = f.command(&format!("load-bp {}", graph.id)).await.unwrap();
    assert_eq!(blueprint_files::decode(loaded.as_bytes()).unwrap(), graph);
    std::fs::write(f.root.join("copy.mbp"), SOURCE).unwrap();
    let id = Uuid::new_v4();
    f.compile("copy.mbp", &format!(" save as {id}")).await.unwrap();
    let copy = blueprint_files::decode(&std::fs::read(f.root.join("copy.mbp.blueprint")).unwrap())
        .unwrap();
    assert_eq!(copy.id, id);
    assert!(ws.db.get(cf::BLUEPRINTS, id.as_bytes()).unwrap().is_some());
    assert!(blueprint_files::binding(&ws.db, id).unwrap().is_some());
    drop(ws);
    f.close().await;
}

#[tokio::test]
async fn cli_save_failures_preserve_files_and_do_not_create_mirrors() {
    let mut f = Fixture::new().await;
    std::fs::write(f.root.join("invalid.mbp"), "not a blueprint").unwrap();
    assert!(f.compile("invalid.mbp", " save").await.is_err());
    assert!(!f.root.join("invalid.mbp.blueprint").exists());
    for name in ["foreign.mbp", "broken.mbp", "readonly.mbp", "directory.mbp"] {
        std::fs::write(f.root.join(name), SOURCE).unwrap();
    }
    let sample = f.compile("foreign.mbp", "").await.unwrap();
    let original_id = blueprint_files::decode(sample.as_bytes()).unwrap().id;
    std::fs::write(f.root.join("foreign.mbp.blueprint"), &sample).unwrap();
    std::fs::write(f.root.join("broken.mbp.blueprint"), "user content is invalid JSON").unwrap();
    std::fs::create_dir(f.root.join("directory.mbp.blueprint")).unwrap();
    let readonly = f.root.join("readonly.mbp.blueprint");
    std::fs::write(&readonly, &sample).unwrap();
    let original_permissions = std::fs::metadata(&readonly).unwrap().permissions();
    let mut locked = original_permissions.clone();
    locked.set_readonly(true);
    std::fs::set_permissions(&readonly, locked).unwrap();
    for (name, expected) in [
        ("foreign.mbp", "different blueprint id"),
        ("broken.mbp", "invalid"),
        ("directory.mbp", "directory"),
    ] {
        let id = Uuid::new_v4();
        let error = f.compile(name, &format!(" save as {id}")).await.unwrap_err();
        assert!(error.to_ascii_lowercase().contains(expected), "{error}");
        let ws = f.app.workspaces.get(&f.root).await.unwrap();
        assert!(ws.db.get(cf::BLUEPRINTS, id.as_bytes()).unwrap().is_none());
        assert!(blueprint_files::binding(&ws.db, id).unwrap().is_none());
    }
    let error = f.compile("readonly.mbp", &format!(" save as {original_id}")).await.unwrap_err();
    assert!(error.contains("read-only"), "{error}");
    assert_eq!(std::fs::read_to_string(&readonly).unwrap(), sample);
    assert_eq!(std::fs::read_to_string(f.root.join("foreign.mbp.blueprint")).unwrap(), sample);
    assert_eq!(
        std::fs::read_to_string(f.root.join("broken.mbp.blueprint")).unwrap(),
        "user content is invalid JSON"
    );
    assert!(f.root.join("directory.mbp.blueprint").is_dir());
    std::fs::set_permissions(readonly, original_permissions).unwrap();
    f.close().await;
}

#[tokio::test]
async fn cli_save_cannot_replace_an_active_blueprint() {
    let mut f = Fixture::new().await;
    std::fs::write(
        f.root.join("active.mbp"),
        "blueprint \"Active\"\nentry s: Start\nd: Delay(Ms = 10000)\ne: End\ns -> d\nd -> e\n",
    )
    .unwrap();
    let id = Uuid::new_v4();
    f.compile("active.mbp", &format!(" save as {id}")).await.unwrap();
    let file = f.root.join("active.mbp.blueprint");
    let before = std::fs::read(&file).unwrap();
    let graph = blueprint_files::decode(&before).unwrap();
    let delay = graph.nodes.iter().find(|n| n.kind == "Delay").unwrap().id.to_string();
    let mut stream = f
        .client
        .execute_blueprint(proto::ExecuteBlueprintRequest {
            workspace_path: f.root.to_string_lossy().into(),
            blueprint_id: id.to_string(),
            blueprint_json: String::new(),
        })
        .await
        .unwrap()
        .into_inner();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let event = stream.message().await.unwrap().unwrap();
            if event.kind == "started" && event.node_id == delay {
                break;
            }
        }
    })
    .await
    .unwrap();
    let error = f.compile("active.mbp", &format!(" save as {id}")).await.unwrap_err();
    assert!(error.contains("blueprint is running"), "{error}");
    assert_eq!(std::fs::read(&file).unwrap(), before);
    f.command("cancel").await.unwrap();
    // Delay completes at its existing node boundary before cancellation is observed.
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            match stream.message().await {
                Ok(Some(_)) => {}
                Err(status) => {
                    assert_eq!(status.code(), tonic::Code::Aborted);
                    break;
                }
                Ok(None) => panic!("cancelled execution must report interruption"),
            }
        }
    })
    .await
    .unwrap();
    f.close().await;
}
