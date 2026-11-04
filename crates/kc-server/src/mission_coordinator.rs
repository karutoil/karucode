//! Mission planning tools. Drafting is not execution: only the explicit start
//! endpoint authorizes dispatch. Ordinary session tools retain their usual policy.
use std::sync::Arc;

use async_trait::async_trait;
use kc_core::approval::ApprovalKind;
use kc_core::config::ApprovalMode;
use kc_core::tool::{Tool, ToolCallRequest, ToolClass, ToolCtx, ToolOutcome, ToolSpec};
use serde::Deserialize;
use serde_json::json;

use crate::state::AppState;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Draft {
    mission_id: String,
    project_id: Option<String>,
    title: String,
    prompt: String,
    #[serde(default)]
    dependencies: Vec<String>,
    #[serde(default)]
    artifact_ids: Vec<String>,
}

pub struct DraftTask(pub AppState);

#[async_trait]
impl Tool for DraftTask {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "mission_draft_task".into(),
            description: "Add one draft task to a mission. This does not start work; the user must explicitly start the mission. Dependencies must be IDs of existing tasks in the same mission. Use only in the mission's coordinator chat.".into(),
            parameters: json!({"type":"object","properties":{
                "mission_id":{"type":"string"}, "project_id":{"type":"string","description":"Registered target project; omit for home project"},
                "title":{"type":"string"}, "prompt":{"type":"string"},
                "dependencies":{"type":"array","items":{"type":"string"}},
                "artifact_ids":{"type":"array","items":{"type":"string"},"description":"Explicitly selected same-mission artifacts as untrusted context"}
            },"required":["mission_id","title","prompt"],"additionalProperties":false}),
            origin: Default::default(),
        }
    }
    fn class(&self) -> ToolClass {
        ToolClass::Mutating
    }
    async fn invoke(&self, request: ToolCallRequest, ctx: ToolCtx) -> ToolOutcome {
        let args: Draft = match serde_json::from_value(request.arguments) {
            Ok(v) => v,
            Err(e) => return ToolOutcome::err(format!("invalid task: {e}")),
        };
        let store = self.0.store();
        let links = match store.list_mission_sessions(&args.mission_id).await {
            Ok(v) => v,
            Err(e) => return ToolOutcome::err(e.to_string()),
        };
        if ctx.approvals.agent_id.is_some()
            || !links.iter().any(|link| {
                link.request_id == crate::routes::missions::COORDINATOR_REQUEST
                    && link.session_id == ctx.approvals.session_id.as_str()
            })
        {
            return ToolOutcome::err("This tool is only available to the mission's coordinator session, not subagents or other sessions.");
        }
        // A draft is a database mutation, not a workspace path: path allowlists
        // cannot authorize it. Match the mutating-tool mode policy otherwise.
        match ctx.settings.approval_policy().await.mode {
            ApprovalMode::ReadOnly => return ToolOutcome::err(
                "mission_draft_task is refused: the approval policy is `read_only`, so only read-only tools are allowed.",
            ),
            ApprovalMode::Ask => {
                let decision = ctx.approvals.ask(
                    ApprovalKind::Other,
                    format!("Draft mission task: {}", args.title),
                    format!("Add a draft task to mission {}. This does not start work.", args.mission_id),
                    json!({"tool": "mission_draft_task", "mission_id": args.mission_id,
                        "project_id": args.project_id, "title": args.title,
                        "prompt": args.prompt, "dependencies": args.dependencies, "artifact_ids": args.artifact_ids}),
                ).await;
                if !decision.is_allowed() {
                    return ToolOutcome::err(format!("denied by user ({}) for mission_draft_task", decision.as_str()));
                }
            }
            ApprovalMode::Auto | ApprovalMode::AskCommands => {}
        }
        match crate::routes::missions::add_task(
            &self.0,
            &args.mission_id,
            args.project_id,
            args.title,
            args.prompt,
            args.dependencies,
            args.artifact_ids,
        )
        .await
        {
            Ok(task) => ToolOutcome::ok(task.to_string()),
            Err(e) => ToolOutcome::err(e.message),
        }
    }
}

pub fn tool(state: AppState) -> Arc<dyn Tool> {
    Arc::new(DraftTask(state))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Revision {
    mission_id: String,
    task_id: String,
    project_id: Option<String>,
    prompt: Option<String>,
    dependencies: Option<Vec<String>>,
}

pub struct ReviseTask(pub AppState);

#[async_trait]
impl Tool for ReviseTask {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "mission_revise_task".into(),
            description: "Revise the prompt, target project or dependencies of an existing draft task before its mission starts. Omitted fields stay unchanged; dependencies replace the entire list. This never starts work. Use only in the mission's coordinator chat.".into(),
            parameters: json!({"type":"object","properties":{
                "mission_id":{"type":"string"}, "task_id":{"type":"string"},
                "project_id":{"type":"string","description":"Registered target project"},
                "prompt":{"type":"string"},
                "dependencies":{"type":"array","items":{"type":"string"},"description":"Replacement list of same-mission task IDs"}
            },"required":["mission_id","task_id"],"additionalProperties":false}),
            origin: Default::default(),
        }
    }
    fn class(&self) -> ToolClass {
        ToolClass::Mutating
    }
    async fn invoke(&self, request: ToolCallRequest, ctx: ToolCtx) -> ToolOutcome {
        let args: Revision = match serde_json::from_value(request.arguments) {
            Ok(v) => v,
            Err(e) => return ToolOutcome::err(format!("invalid revision: {e}")),
        };
        let links = match self.0.store().list_mission_sessions(&args.mission_id).await {
            Ok(v) => v,
            Err(e) => return ToolOutcome::err(e.to_string()),
        };
        if ctx.approvals.agent_id.is_some()
            || !links.iter().any(|link| {
                link.request_id == crate::routes::missions::COORDINATOR_REQUEST
                    && link.session_id == ctx.approvals.session_id.as_str()
            })
        {
            return ToolOutcome::err("This tool is only available to the mission's coordinator session, not subagents or other sessions.");
        }
        match ctx.settings.approval_policy().await.mode {
            ApprovalMode::ReadOnly => return ToolOutcome::err("mission_revise_task is refused: the approval policy is `read_only`, so only read-only tools are allowed."),
            ApprovalMode::Ask => {
                let decision = ctx.approvals.ask(
                    ApprovalKind::Other,
                    format!("Revise mission task: {}", args.task_id),
                    format!("Revise a draft task in mission {}. This does not start work.", args.mission_id),
                    json!({"tool":"mission_revise_task", "mission_id":args.mission_id,
                        "task_id":args.task_id, "project_id":args.project_id,
                        "prompt":args.prompt, "dependencies":args.dependencies}),
                ).await;
                if !decision.is_allowed() {
                    return ToolOutcome::err(format!("denied by user ({}) for mission_revise_task", decision.as_str()));
                }
            }
            ApprovalMode::Auto | ApprovalMode::AskCommands => {}
        }
        match crate::routes::missions::edit_task(
            &self.0,
            &args.mission_id,
            &args.task_id,
            args.project_id,
            args.prompt,
            args.dependencies,
        )
        .await
        {
            Ok(task) => ToolOutcome::ok(task.to_string()),
            Err(e) => ToolOutcome::err(e.message),
        }
    }
}

pub fn revise_tool(state: AppState) -> Arc<dyn Tool> {
    Arc::new(ReviseTask(state))
}
