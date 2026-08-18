use super::*;
use crate::protocol::{
    AgentApprovalStatus, AgentAttachmentLibraryContext, AgentAttachmentReference,
    AgentCommandRiskLevel, AgentCommandRuntimeBinding, AgentCommandRuntimeKind,
    AgentCommandRuntimeResolvedPackage, AgentInputAttachmentKind, AgentPermissions,
    AgentReadPermission, AgentRunContext, AgentSkillMaterializationResult,
    AgentSkillMaterializationResultStatus, AgentToolCall, AgentToolResult, AgentWorkspaceContext,
    AgentWritePermission, AGENT_COMMAND_RUNTIME_BINDING_SCHEMA_VERSION,
};
use crate::skills::{
    memory_resource_session_for_test, SkillId, SkillPackageUri, SkillResourceKind,
    SkillResourcePath, SkillRevision, SkillSourceId, APPLICATION_BUNDLED_SKILL_SOURCE_ID,
};
use crate::storage::models::{AgentActionAuditRecord, ChatConversationRecord, ChatMessageRecord};
use crate::storage::service::{ManagedArtifactAuthority, StorageService};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::sync::Arc;

#[derive(Clone)]
struct FakeProfileResolver {
    binding: AgentCommandRuntimeBinding,
}

impl crate::command::CommandRuntimeProfileResolver for FakeProfileResolver {
    fn resolve_profile(
        &self,
        profile: AgentCommandRuntimeProfile,
        kind: AgentCommandRuntimeKind,
    ) -> Result<AgentCommandRuntimeBinding, crate::command::CommandRuntimeProfileError> {
        assert_eq!(profile, self.binding.profile);
        assert_eq!(kind, self.binding.kind);
        Ok(self.binding.clone())
    }
}

fn test_binding(
    profile: AgentCommandRuntimeProfile,
    kind: AgentCommandRuntimeKind,
    packages: &[(&str, &str)],
) -> AgentCommandRuntimeBinding {
    let resolved_packages = packages
        .iter()
        .map(|(name, version)| AgentCommandRuntimeResolvedPackage {
            name: (*name).to_string(),
            version: (*version).to_string(),
        })
        .collect::<Vec<_>>();
    AgentCommandRuntimeBinding {
        schema_version: AGENT_COMMAND_RUNTIME_BINDING_SCHEMA_VERSION,
        profile,
        profile_revision: crate::command::runtime_profile_revision(
            profile,
            kind,
            &resolved_packages,
        ),
        provider_id: crate::artifact_runtime::ARTIFACT_RUNTIME_PROVIDER_ID.to_string(),
        bundle_version: crate::artifact_runtime::ARTIFACT_RUNTIME_BUNDLE_VERSION.to_string(),
        bundle_revision: "artifact-runtime-bundle-sha256-v1:test".to_string(),
        kind,
        runtime_version: "22.23.1".to_string(),
        runtime_fingerprint: "artifact-runtime-sha256-v1:test".to_string(),
        resolved_packages,
    }
}

fn with_profile_resolver(
    context: ToolExecutionContext,
    binding: AgentCommandRuntimeBinding,
) -> ToolExecutionContext {
    context.with_command_runtime_profile_resolver(Some(Arc::new(FakeProfileResolver { binding })))
}

fn record_materialized_builder(
    storage: &StorageService,
    run_id: &str,
    profile: AgentCommandRuntimeProfile,
    destination: &str,
) {
    let (local_id, template_path) = match profile {
        AgentCommandRuntimeProfile::Documents => (DOCUMENTS_LOCAL_ID, "templates/builder.py"),
        AgentCommandRuntimeProfile::Spreadsheets => (SPREADSHEETS_LOCAL_ID, "templates/builder.py"),
        AgentCommandRuntimeProfile::Presentations => {
            (PRESENTATIONS_LOCAL_ID, "templates/builder.mjs")
        }
        AgentCommandRuntimeProfile::Pdf => {
            panic!("PDF does not use the Office Builder materialization helper")
        }
    };
    let revision = SkillRevision::parse(format!("revision-{local_id}")).unwrap();
    let source = SkillPackageUri::new(
        SkillId::parse(format!("{APPLICATION_BUNDLED_SKILL_SOURCE_ID}:{local_id}")).unwrap(),
        revision.clone(),
    )
    .resource(SkillResourcePath::parse(template_path.to_string()).unwrap());
    let result = AgentSkillMaterializationResult {
        status: AgentSkillMaterializationResultStatus::Applied,
        source_uri: source.to_string(),
        source_prefix: None,
        destination: destination.to_string(),
        source_revision: revision.to_string(),
        file_count: 1,
        byte_count: 100,
        plan_digest: Some("skill-materialization-sha256-v1:test".to_string()),
        error: None,
        message: Some("created".to_string()),
    };
    let tool_result = AgentToolResult {
        exact_archive_file: None,
        call_id: format!("materialize-{local_id}"),
        tool: "skills_materialize_resource".to_string(),
        ok: true,
        result: Some(serde_json::to_value(result).unwrap()),
        error: None,
    };
    storage
        .upsert_agent_action_audit(AgentActionAuditRecord {
            action_id: format!("materialize-{local_id}"),
            run_id: run_id.to_string(),
            conversation_id: None,
            assistant_message_id: None,
            action_type: "skill_materialization".to_string(),
            tool_name: "skills_materialize_resource".to_string(),
            decision: Some("approved".to_string()),
            status: "completed".to_string(),
            action_json: "{}".to_string(),
            patch_result_json: None,
            command_result_json: None,
            tool_result_json: Some(serde_json::to_string(&tool_result).unwrap()),
            error: None,
            created_at: 1,
            decided_at: Some(2),
            completed_at: Some(3),
            effective_permissions_json: None,
            path_scope: None,
            command_cwd_scope: None,
            blocked_reason: None,
            decision_source: Some("manual".to_string()),
        })
        .unwrap();
}

fn record_materialized_presentation_editor(
    storage: &StorageService,
    run_id: &str,
    source_id: &str,
    destination: &str,
) {
    let revision = SkillRevision::parse("revision-presentations-editor").unwrap();
    let source = SkillPackageUri::new(
        SkillId::parse(format!("{source_id}:{PRESENTATIONS_LOCAL_ID}")).unwrap(),
        revision.clone(),
    )
    .resource(SkillResourcePath::parse("templates/editor.mjs".to_string()).unwrap());
    let result = AgentSkillMaterializationResult {
        status: AgentSkillMaterializationResultStatus::Applied,
        source_uri: source.to_string(),
        source_prefix: None,
        destination: destination.to_string(),
        source_revision: revision.to_string(),
        file_count: 1,
        byte_count: 100,
        plan_digest: Some("skill-materialization-sha256-v1:editor-test".to_string()),
        error: None,
        message: Some("created".to_string()),
    };
    let tool_result = AgentToolResult {
        exact_archive_file: None,
        call_id: format!("materialize-editor-{source_id}"),
        tool: "skills_materialize_resource".to_string(),
        ok: true,
        result: Some(serde_json::to_value(result).unwrap()),
        error: None,
    };
    storage
        .upsert_agent_action_audit(AgentActionAuditRecord {
            action_id: format!("materialize-editor-{source_id}"),
            run_id: run_id.to_string(),
            conversation_id: None,
            assistant_message_id: None,
            action_type: "skill_materialization".to_string(),
            tool_name: "skills_materialize_resource".to_string(),
            decision: Some("approved".to_string()),
            status: "completed".to_string(),
            action_json: "{}".to_string(),
            patch_result_json: None,
            command_result_json: None,
            tool_result_json: Some(serde_json::to_string(&tool_result).unwrap()),
            error: None,
            created_at: 1,
            decided_at: Some(2),
            completed_at: Some(3),
            effective_permissions_json: None,
            path_scope: None,
            command_cwd_scope: None,
            blocked_reason: None,
            decision_source: Some("manual".to_string()),
        })
        .unwrap();
}

fn record_materialized_python_editor(
    storage: &StorageService,
    run_id: &str,
    profile: AgentCommandRuntimeProfile,
    destination: &str,
) {
    let (local_id, template_path) = match profile {
        AgentCommandRuntimeProfile::Documents => (DOCUMENTS_LOCAL_ID, "templates/editor.py"),
        AgentCommandRuntimeProfile::Spreadsheets => (SPREADSHEETS_LOCAL_ID, "templates/editor.py"),
        _ => panic!("only Word and Excel use the Python Editor receipt helper"),
    };
    let revision = SkillRevision::parse(format!("revision-{local_id}-editor")).unwrap();
    let source = SkillPackageUri::new(
        SkillId::parse(format!("{APPLICATION_BUNDLED_SKILL_SOURCE_ID}:{local_id}")).unwrap(),
        revision.clone(),
    )
    .resource(SkillResourcePath::parse(template_path.to_string()).unwrap());
    let result = AgentSkillMaterializationResult {
        status: AgentSkillMaterializationResultStatus::Applied,
        source_uri: source.to_string(),
        source_prefix: None,
        destination: destination.to_string(),
        source_revision: revision.to_string(),
        file_count: 1,
        byte_count: 100,
        plan_digest: Some("skill-materialization-sha256-v1:python-editor-test".to_string()),
        error: None,
        message: Some("created".to_string()),
    };
    let tool_result = AgentToolResult {
        exact_archive_file: None,
        call_id: format!("materialize-{local_id}-editor"),
        tool: "skills_materialize_resource".to_string(),
        ok: true,
        result: Some(serde_json::to_value(result).unwrap()),
        error: None,
    };
    storage
        .upsert_agent_action_audit(AgentActionAuditRecord {
            action_id: format!("materialize-{local_id}-editor"),
            run_id: run_id.to_string(),
            conversation_id: None,
            assistant_message_id: None,
            action_type: "skill_materialization".to_string(),
            tool_name: "skills_materialize_resource".to_string(),
            decision: Some("approved".to_string()),
            status: "completed".to_string(),
            action_json: "{}".to_string(),
            patch_result_json: None,
            command_result_json: None,
            tool_result_json: Some(serde_json::to_string(&tool_result).unwrap()),
            error: None,
            created_at: 1,
            decided_at: Some(2),
            completed_at: Some(3),
            effective_permissions_json: None,
            path_scope: None,
            command_cwd_scope: None,
            blocked_reason: None,
            decision_source: Some("manual".to_string()),
        })
        .unwrap();
}

mod frozen_and_contract;
mod managed_builders;
mod pdf_binding;

use pdf_binding::{pdf_and_documents_skill_session, pdf_skill_session};
