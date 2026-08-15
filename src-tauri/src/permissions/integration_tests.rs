//! Permission pipeline integration tests — the exact backend chain the UI
//! buttons drive, against a REAL AppState + real SQLite database.
//!
//! Covered (each maps to a UI path):
//! - "始终允许" → respond → durable grant → next matching call auto-resolves
//!   without a dialog → 撤销后立即失效;
//! - "仍要允许一次" (Auto-Review override) → session grant → same-call retry
//!   passes; dangerous classes stay un-grantable;
//! - 模式切换 → per-session override drives the rule layer (plan/accept/
//!   bypass matrix).

use crate::bootstrap::AppState;
use crate::permissions::checker::PermissionResult;
use crate::permissions::grant_store::PendingPermission;
use crate::permissions::mode::PermissionMode;
use serde_json::json;
use std::time::Duration;

/// Build a real AppState against an isolated temp data dir (the app reads
/// `DEEPDEPCAT_DATA_DIR` before anything else). The env var is process-global,
/// so the data-dir tests serialize on `DATA_DIR_LOCK` to avoid two concurrent
/// `AppState::initialize` calls landing on the same DB.
async fn temp_app_state() -> AppState {
    let _guard = crate::permissions::DATA_DIR_LOCK.lock().unwrap();
    let dir = std::env::temp_dir().join(format!(
        "ddc-perm-it-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    std::env::set_var("DEEPDEPCAT_DATA_DIR", &dir);
    AppState::initialize(None).await.expect("AppState init")
}

fn bash_args(command: &str) -> serde_json::Value {
    json!({ "command": command })
}

#[tokio::test]
async fn always_allow_flow_auto_resolves_then_revoke_takes_effect_immediately() {
    let state = temp_app_state().await;
    let sid = {
        let mut sessions = state.sessions.lock().await;
        sessions
            .create_session("m", "p", None, None, None, None, None)
            .unwrap()
            .id
            .clone()
    };
    let tool = "bash";
    let args = bash_args("git status");

    // 1. First ask → user clicks 始终允许 (respond + record durable grant).
    let (tx1, mut rx1) = tokio::sync::oneshot::channel();
    let rid1 = crate::core::ids::tool_call_id();
    state
        .pending_permissions
        .lock()
        .await
        .insert(
            rid1.clone(),
            PendingPermission {
                sender: tx1,
                tool_name: tool.into(),
                args: args.clone(),
                session_id: sid.clone(),
            },
        );
    let meta = state
        .respond_permission(&rid1, true, None)
        .await
        .expect("respond must return tool metadata");
    assert_eq!(meta.0, tool);
    state.grant_store.record(&meta.0, &meta.1);

    // The dialog reply reached the waiting call.
    let reply = tokio::time::timeout(Duration::from_secs(1), &mut rx1)
        .await
        .expect("first ask must be answered")
        .expect("channel open");
    assert!(reply.allow);
    assert!(state.grant_store.allows(tool, &args));

    // 2. Same call again → auto-resolved without a dialog (the queued
    //    request is granted by the remembered grant).
    let (tx2, rx2) = tokio::sync::oneshot::channel();
    let rid2 = crate::core::ids::tool_call_id();
    state
        .pending_permissions
        .lock()
        .await
        .insert(
            rid2.clone(),
            PendingPermission {
                sender: tx2,
                tool_name: tool.into(),
                args: args.clone(),
                session_id: sid.clone(),
            },
        );
    state.auto_resolve_pending_permissions(&sid).await;
    let auto = tokio::time::timeout(Duration::from_secs(1), rx2)
        .await
        .expect("auto-resolve must answer")
        .expect("channel open");
    assert!(auto.allow, "remembered grant must auto-approve the same call");

    // 3. 撤销 → 立即失效（下一个请求必须重新询问，不再自动放行）。
    let pattern = crate::permissions::grant_store::extract_pattern(tool, &args);
    assert!(state.grant_store.remove(tool, &pattern));
    assert!(!state.grant_store.allows(tool, &args));
}

#[tokio::test]
async fn session_grant_override_covers_retry_but_never_dangerous_classes() {
    let state = temp_app_state().await;
    let sid = {
        let mut sessions = state.sessions.lock().await;
        sessions
            .create_session("m", "p", None, None, None, None, None)
            .unwrap()
            .id
            .clone()
    };

    // 「仍要允许一次」= exact-action session grant; the same retry passes.
    let args = bash_args("git status");
    state.record_session_grant(&sid, "bash", &args).await;
    assert!(
        state.session_grant_allows(&sid, "bash", &args).await,
        "override must cover the exact retry"
    );

    // Dangerous classes are never covered by the override.
    assert!(
        !state
            .session_grant_allows(&sid, "bash", &bash_args("git push origin main"))
            .await,
        "git push (irreversible remote action) must stay un-grantable"
    );
    assert!(
        !state
            .session_grant_allows(&sid, "bash", &bash_args("rm -rf x"))
            .await,
        "dangerous bash must stay un-grantable"
    );
    // A different (unrelated) command is not covered either.
    assert!(
        !state
            .session_grant_allows(&sid, "bash", &bash_args("echo nope"))
            .await,
        "override must stay scoped to the exact action"
    );
}

#[tokio::test]
async fn session_mode_switch_drives_the_rule_layer() {
    let state = temp_app_state().await;
    let sid = {
        let mut sessions = state.sessions.lock().await;
        sessions
            .create_session("m", "p", None, None, None, None, None)
            .unwrap()
            .id
            .clone()
    };
    let edit = json!({ "path": "x.rs" });

    // 只读：写编辑硬拒。
    state.set_session_mode(&sid, PermissionMode::ReadOnly).await;
    assert_eq!(state.session_mode(&sid).await, PermissionMode::ReadOnly);
    assert!(matches!(
        state.permissions.check_with_mode(
            "edit_file",
            &edit,
            false,
            &sid,
            state.session_mode(&sid).await,
        ),
        PermissionResult::Deny(_)
    ));

    // 接受编辑：编辑自动放行。
    state.set_session_mode(&sid, PermissionMode::AcceptEdits).await;
    assert!(matches!(
        state.permissions.check_with_mode(
            "edit_file",
            &edit,
            false,
            &sid,
            state.session_mode(&sid).await,
        ),
        PermissionResult::Allow
    ));

    // 完全访问：写与 bash 都放行。
    state.set_session_mode(&sid, PermissionMode::FullAccess).await;
    assert!(matches!(
        state.permissions.check_with_mode(
            "bash",
            &bash_args("echo hi"),
            false,
            &sid,
            state.session_mode(&sid).await,
        ),
        PermissionResult::Allow
    ));

    // 清除覆盖 → 回到全局默认（accept-edits → 编辑自动放行）。
    state.clear_session_mode(&sid).await;
    assert!(matches!(
        state.permissions.check_with_mode(
            "edit_file",
            &edit,
            false,
            &sid,
            state.session_mode(&sid).await,
        ),
        PermissionResult::Allow
    ));
}

#[tokio::test]
async fn deny_decision_is_returned_and_records_nothing() {
    let state = temp_app_state().await;
    let sid = {
        let mut sessions = state.sessions.lock().await;
        sessions
            .create_session("m", "p", None, None, None, None, None)
            .unwrap()
            .id
            .clone()
    };
    let tool = "bash";
    let args = bash_args("curl example.com");
    let (tx, mut rx) = tokio::sync::oneshot::channel();
    let rid = crate::core::ids::tool_call_id();
    state
        .pending_permissions
        .lock()
        .await
        .insert(
            rid.clone(),
            PendingPermission {
                sender: tx,
                tool_name: tool.into(),
                args: args.clone(),
                session_id: sid.clone(),
            },
        );
    let meta = state
        .respond_permission(&rid, false, Some("用户拒绝".into()))
        .await
        .expect("deny must return metadata");
    let reply = tokio::time::timeout(Duration::from_secs(1), &mut rx)
        .await
        .expect("denied ask must be answered")
        .expect("channel open");
    assert!(!reply.allow);
    assert_eq!(reply.reason.as_deref(), Some("用户拒绝"));
    assert!(
        !state.grant_store.allows(tool, &args),
        "a deny must never record a grant"
    );
    assert_eq!(meta.0, tool);
}
