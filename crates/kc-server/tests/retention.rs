//! The server retention caller sees auxiliary deletions and reclaims their pages.

mod support;

use kc_core::checkpoint::FileCheckpoint;
use kc_core::store::RunStatus;
use support::HarnessBuilder;

#[tokio::test]
async fn retention_reclaims_an_auxiliary_only_prune() {
    let h = HarnessBuilder::new(Vec::new())
        .retention_days(1)
        .build()
        .await;
    let session = h.create_session(Some("Retention")).await;
    let store = h.state.store();
    store
        .create_run("r_stale", &session, "test-model", "old run")
        .await
        .unwrap();
    store
        .finish_run("r_stale", RunStatus::Interrupted, None)
        .await
        .unwrap();
    let checkpoint = FileCheckpoint {
        id: 0,
        session_id: session.clone(),
        run_id: "r_stale".into(),

        agent_id: None,
        call_id: "call_stale".into(),
        tool: "edit_file".into(),
        path: "orphan.txt".into(),
        existed: true,
        content: "private before-image".into(),
        capture_failed: false,
        // Unbound journal rows survive only while their writer is running.
        message_id: 0,
        created_at: 0,
    };
    let id = store.record_checkpoint(&checkpoint).await.unwrap();

    let report = kc_server::retention::run_once(&h.state).await;
    assert_eq!(report.messages, 0);
    assert_eq!(report.runs, 0);
    assert_eq!(report.agents, 0);
    assert_eq!(report.approvals, 0);
    assert_eq!(report.claims, 0);
    assert_eq!(report.checkpoints, 1, "{report:?}");
    assert_eq!(report.checkpoint_blobs, 1, "{report:?}");
    assert!(
        !report.is_empty(),
        "auxiliary deletion must request reclaim"
    );
    assert!(store.checkpoint_content(id).await.unwrap().is_none());
    assert!(store
        .list_checkpoints(&session, 0)
        .await
        .unwrap()
        .is_empty());
}
