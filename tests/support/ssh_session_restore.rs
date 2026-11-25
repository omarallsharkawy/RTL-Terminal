use super::*;

#[test]
fn round_trips_remote_and_old_local_leaves_and_reattaches_with_saved_identity() {
    use crate::remote::RemotePane;

    #[derive(Default)]
    struct RecordingFactory {
        local: FakeShells,
        remote: Vec<Target>,
    }

    impl PaneFactory for RecordingFactory {
        fn spawn(
            &mut self,
            id: PaneId,
            cols: usize,
            rows: usize,
            cwd: Option<PathBuf>,
        ) -> Option<Pane> {
            self.local.spawn(id, cols, rows, cwd)
        }

        fn spawn_remote(
            &mut self,
            id: PaneId,
            cols: usize,
            rows: usize,
            target: Target,
        ) -> Option<Pane> {
            self.remote.push(target.clone());
            let mut pane = Pane::new(id, Terminal::new(cols, rows, |_| {}), None, (cols, rows));
            pane.remote = Some(RemotePane::new(target));
            Some(pane)
        }
    }

    let dir = TempDir::new();
    let store = dir.store();
    let path = store.path.as_deref().unwrap();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        path,
        br#"{
      "version":1,
      "active_tab":0,
      "sidebar_visible":true,
      "tabs":[{"focused":1,"nodes":[
        {"kind":"pane","title":"Old local pane"},
        {"kind":"ssh","destination":"user@example","name":"build-1"},
        {"kind":"split","direction":"horizontal","ratio":0.5,"a":0,"b":1}
      ]}]
    }"#,
    )
    .unwrap();

    let mut factory = RecordingFactory::default();
    let workspace = store.load().unwrap().rebuild(&Grid, &mut factory);
    let target = Target::new("user@example", "build-1").unwrap();
    assert_eq!(factory.remote, std::slice::from_ref(&target));
    assert_eq!(factory.local.calls.len(), 1);
    assert_eq!(
        workspace
            .pane(workspace.focused_id().unwrap())
            .unwrap()
            .remote
            .as_ref()
            .unwrap()
            .target,
        target,
    );
    assert_eq!(
        workspace
            .panes()
            .filter_map(|pane| pane.title.as_deref())
            .collect::<Vec<_>>(),
        ["Old local pane"]
    );

    store.finish(&workspace, true);
    let saved: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert_eq!(saved["version"], 1);
    assert_eq!(
        saved["tabs"][0]["nodes"][1],
        serde_json::json!({"kind":"ssh","destination":"user@example","name":"build-1"}),
    );
    assert_eq!(
        saved["tabs"][0]["nodes"][0],
        serde_json::json!({"kind":"pane","title":"Old local pane"})
    );

    let mut factory = RecordingFactory::default();
    store.load().unwrap().rebuild(&Grid, &mut factory);
    assert_eq!(factory.remote, [target]);
    assert_eq!(factory.local.calls.len(), 1);
}
