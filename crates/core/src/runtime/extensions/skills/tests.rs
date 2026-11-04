use super::*;
use crate::context::{ContextFrame, ContextTextBudget};
use crate::llm::LlmMessageRole;
use crate::protocol::{AgentActivatedSkillResources, AgentRunContext, AgentToolResult};
use crate::skills::{
    memory_resource_session_for_test, SkillId, SkillResourceKind, SkillRevision, SkillSourceId,
};
use crate::tools::{
    AgentSkillInstallationCommitPreparationRequest, AgentSkillInstallationCommitPreparer,
    AgentSkillInstallationPrepareExecutor, AgentSkillInstallationPrepareRequest, AgentToolExposure,
    ToolRegistry,
};
use std::sync::atomic::{AtomicUsize, Ordering};

const SECRET_INSTRUCTIONS: &str =
    "PRIVATE COMPLETE SKILL INSTRUCTIONS: always verify the generated artifact.";

struct CapacityDynamicTool;

struct CapacitySkillInstallationPrepare;
struct CapacitySkillInstallationCommit;

impl AgentSkillInstallationPrepareExecutor for CapacitySkillInstallationPrepare {
    fn prepare(&self, _request: AgentSkillInstallationPrepareRequest) -> AgentResult<Value> {
        unreachable!("capacity tests never execute the projected Tool")
    }
}

impl AgentSkillInstallationCommitPreparer for CapacitySkillInstallationCommit {
    fn prepare_commit_action(
        &self,
        _request: AgentSkillInstallationCommitPreparationRequest,
    ) -> AgentResult<crate::protocol::AgentSkillInstallationRequest> {
        unreachable!("capacity tests never execute the projected Tool")
    }

    fn invalidate_commit_action(
        &self,
        _action: &crate::protocol::AgentSkillInstallationRequest,
    ) -> AgentResult<()> {
        Ok(())
    }
}

impl AgentTool for CapacityDynamicTool {
    fn exposure(&self) -> AgentToolExposure {
        AgentToolExposure::RequiresCapability(ToolCapabilityId::application_owned(
            OFFICE_DOCUMENTS_CAPABILITY,
        ))
    }

    fn permission_policy(&self) -> AgentToolPermissionPolicy {
        AgentToolPermissionPolicy::Default
    }

    fn definition(&self) -> AgentToolDefinition {
        AgentToolDefinition {
            name: "capacity_document_tool".to_string(),
            description:
                "A deliberately non-empty document Tool contract for activation capacity tests."
                    .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "filePath": {
                        "type": "string",
                        "description": "Document path."
                    }
                },
                "required": ["filePath"],
                "additionalProperties": false
            }),
            safety: AgentToolSafety::ReadOnly,
            requires_workspace: false,
            requires_approval: false,
            approval_mode: AgentToolApprovalMode::Never,
        }
    }

    fn execute(&self, _context: &ToolExecutionContext, _args: Value) -> AgentResult<Value> {
        unreachable!("capacity tests never execute the projected Tool")
    }
}

fn effective_tool_set_with_document_tool() -> EffectiveToolSet {
    let mut registry = ToolRegistry::empty();
    registry.register_test_tool(CapacityDynamicTool);
    EffectiveToolSet::from_permitted_definitions(
        &registry,
        registry.definitions(),
        &BTreeSet::new(),
    )
    .unwrap()
}

fn effective_tool_set_with_skill_installation_tool() -> EffectiveToolSet {
    let mut registry = ToolRegistry::empty();
    registry.register_skill_installation_prepare(Arc::new(CapacitySkillInstallationPrepare));
    registry.register_skill_installation_commit(Arc::new(CapacitySkillInstallationCommit));
    EffectiveToolSet::from_permitted_definitions(
        &registry,
        registry.definitions(),
        &BTreeSet::new(),
    )
    .unwrap()
}

fn revision(fill: char) -> String {
    format!("skill-package-sha256-v2:{}", fill.to_string().repeat(64))
}

fn discoverable_skill(
    _activation_ref: &str,
    local_id: &str,
    name: &str,
    revision: &str,
) -> AgentDiscoverableSkill {
    let id = format!("bundled:application:{local_id}");
    AgentDiscoverableSkill {
        activation_ref: crate::skills::derive_skill_activation_ref(
            "catalog-sha256-v1:test",
            &id,
            revision,
        ),
        id,
        revision: revision.to_string(),
        name: name.to_string(),
        description: format!("Use {name} for focused work."),
        source_kind: "bundled".to_string(),
    }
}

fn discovery(
    skills: Vec<AgentDiscoverableSkill>,
    max_activated_skills: usize,
    max_total_source_bytes: usize,
) -> AgentSkillDiscoverySnapshot {
    AgentSkillDiscoverySnapshot {
        schema_version: crate::skills::AGENT_SKILL_DISCOVERY_SCHEMA_VERSION,
        catalog_revision: "catalog-sha256-v1:test".to_string(),
        prompt_token_budget: 2_000,
        skills,
        max_activated_skills,
        max_total_source_bytes,
    }
}

fn activated_skill(entry: &AgentDiscoverableSkill, instructions: &str) -> AgentActivatedSkill {
    AgentActivatedSkill {
        id: entry.id.clone(),
        name: entry.name.clone(),
        revision: entry.revision.clone(),
        source: "bundled:application".to_string(),
        instructions: instructions.to_string(),
        source_bytes: u64::try_from(instructions.len()).unwrap(),
        resources: None,
    }
}

fn resolved_without_resources(
    entry: &AgentDiscoverableSkill,
    instructions: &str,
) -> AgentResolvedSkillActivation {
    AgentResolvedSkillActivation {
        skill: activated_skill(entry, instructions),
        resources: Arc::new(candidate_resource_session(entry, Vec::new())),
    }
}

fn exact_empty_authority(entry: &AgentDiscoverableSkill) -> Arc<SkillResourceSession> {
    Arc::new(candidate_resource_session(entry, Vec::new()))
}

fn candidate_resource_session(
    entry: &AgentDiscoverableSkill,
    resources: Vec<(String, SkillResourceKind, Vec<u8>)>,
) -> SkillResourceSession {
    let skill_id = SkillId::parse(entry.id.clone()).unwrap();
    let source_id = skill_id.source_id().clone();
    memory_resource_session_for_test(
        skill_id,
        SkillRevision::parse(entry.revision.clone()).unwrap(),
        source_id,
        resources,
    )
    .unwrap()
}

fn resolved_with_resources(
    entry: &AgentDiscoverableSkill,
    resources: Vec<(String, SkillResourceKind, Vec<u8>)>,
) -> AgentResolvedSkillActivation {
    let session = candidate_resource_session(entry, resources);
    let (package, resource_count, kinds) = session
        .sole_activation_binding_manifest()
        .expect("fixture binding manifest");
    let mut skill = activated_skill(entry, SECRET_INSTRUCTIONS);
    skill.resources = Some(AgentActivatedSkillResources {
        root_uri: package.to_string(),
        resource_count,
        kinds: kinds
            .iter()
            .map(|kind| kind.stable_name().to_string())
            .collect(),
    });
    AgentResolvedSkillActivation {
        skill,
        resources: Arc::new(session),
    }
}

fn resolver_for(
    candidates: Vec<AgentResolvedSkillActivation>,
    calls: Arc<AtomicUsize>,
) -> AgentSkillActivationResolver {
    Arc::new(move |selection| {
        calls.fetch_add(1, Ordering::SeqCst);
        candidates
            .iter()
            .find(|candidate| {
                candidate.skill.id == selection.skill_id().as_str()
                    && candidate.skill.revision == selection.expected_revision().as_str()
            })
            .cloned()
            .ok_or_else(|| AgentError::new("fixture resolver received an unknown selection"))
    })
}

fn tool_context() -> ToolExecutionContext {
    ToolExecutionContext::from_run_context(None::<&AgentRunContext>)
        .with_tool_call_id("activate-call-1".to_string())
}

fn successful_result(value: Value) -> AgentToolResult {
    AgentToolResult {
        exact_archive_file: None,
        call_id: "activate-call-1".to_string(),
        tool: SKILL_ACTIVATE_TOOL_NAME.to_string(),
        ok: true,
        result: Some(value),
        error: None,
    }
}

#[test]
fn activation_tool_schema_requires_a_bounded_reason_and_rejects_invalid_reasons() {
    let extension = SkillActivationExtension::new(
        "run-1".to_string(),
        None,
        None,
        None,
        Some(Arc::new(SkillResourceSession::empty())),
    )
    .unwrap();
    let tool = SkillsActivateTool {
        state: extension.state.clone(),
    };
    let definition = tool.definition();

    assert_eq!(definition.name, "skills_activate");
    assert_eq!(definition.safety, AgentToolSafety::ReadOnly);
    assert_eq!(definition.approval_mode, AgentToolApprovalMode::Never);
    assert_eq!(
        definition.input_schema["required"],
        json!(["skillRef", "reason"])
    );
    assert_eq!(
        definition.input_schema["properties"]["reason"]["maxLength"],
        json!(MAX_ACTIVATION_REASON_CHARS)
    );
    assert_eq!(
        definition.input_schema["properties"]["skillRef"]["maxLength"],
        json!(SKILL_ACTIVATION_REF_CHARS)
    );
    assert_eq!(
        definition.input_schema["properties"]["skillRef"]["pattern"],
        json!("^s_[0-9a-f]{24}$")
    );
    assert_eq!(definition.input_schema["additionalProperties"], false);
    let valid_ref = "s_000000000000000000000000";

    let missing = tool
        .execute(&tool_context(), json!({ "skillRef": valid_ref }))
        .unwrap_err();
    assert_eq!(missing.code(), Some("skill.invalidActivationRequest"));

    let blank = tool
        .execute(
            &tool_context(),
            json!({ "skillRef": valid_ref, "reason": " \n\t " }),
        )
        .unwrap_err();
    assert_eq!(blank.code(), Some("skill.invalidActivationRequest"));

    let too_long = tool
        .execute(
            &tool_context(),
            json!({
                "skillRef": valid_ref,
                "reason": "x".repeat(MAX_ACTIVATION_REASON_CHARS + 1),
            }),
        )
        .unwrap_err();
    assert_eq!(too_long.code(), Some("skill.activationReasonTooLong"));
}

#[test]
fn user_selection_and_model_activation_unlock_the_same_exact_bundled_capability() {
    let entry = discoverable_skill("s1", DOCUMENTS_LOCAL_ID, "Documents", &revision('a'));
    let initial = AgentSkillActivation {
        activation_revision: "activation-sha256-v1:user".to_string(),
        skills: vec![activated_skill(&entry, SECRET_INSTRUCTIONS)],
    };
    let user_extension = SkillActivationExtension::new(
        "run-user".to_string(),
        Some(discovery(vec![entry.clone()], 4, 16_384)),
        Some(&initial),
        None,
        Some(exact_empty_authority(&entry)),
    )
    .unwrap();

    let calls = Arc::new(AtomicUsize::new(0));
    let model_extension = SkillActivationExtension::new(
        "run-model".to_string(),
        Some(discovery(vec![entry.clone()], 4, 16_384)),
        None,
        Some(resolver_for(
            vec![resolved_without_resources(&entry, SECRET_INSTRUCTIONS)],
            calls.clone(),
        )),
        Some(Arc::new(SkillResourceSession::empty())),
    )
    .unwrap();
    SkillsActivateTool {
        state: model_extension.state.clone(),
    }
    .execute(
        &tool_context(),
        json!({
            "skillRef": entry.activation_ref,
            "reason": "Need document creation guidance"
        }),
    )
    .unwrap();

    let expected = BTreeSet::from([ToolCapabilityId::application_owned(
        OFFICE_DOCUMENTS_CAPABILITY,
    )]);
    assert_eq!(user_extension.active_tool_capabilities().unwrap(), expected);
    assert_eq!(
        model_extension.active_tool_capabilities().unwrap(),
        expected
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn initial_managed_activation_requires_frozen_catalog_and_exact_host_authority() {
    let entry = discoverable_skill("s1", DOCUMENTS_LOCAL_ID, "Documents", &revision('a'));
    let initial = AgentSkillActivation {
        activation_revision: "activation-sha256-v1:user".to_string(),
        skills: vec![activated_skill(&entry, SECRET_INSTRUCTIONS)],
    };

    let missing_package = SkillActivationExtension::new(
        "run-forged-package".to_string(),
        Some(discovery(vec![entry.clone()], 4, 16_384)),
        Some(&initial),
        None,
        Some(Arc::new(SkillResourceSession::empty())),
    )
    .err()
    .expect("an AgentChatInput cannot manufacture Host package authority");
    assert!(missing_package
        .to_string()
        .contains("no exact Host package authority"));

    let missing_catalog = SkillActivationExtension::new(
        "run-forged-catalog".to_string(),
        None,
        Some(&initial),
        None,
        Some(exact_empty_authority(&entry)),
    )
    .err()
    .expect("managed user selection must belong to the frozen enabled catalog");
    assert!(missing_catalog
        .to_string()
        .contains("absent from the frozen discovery catalog"));
}

#[test]
fn explicit_workspace_activation_uses_exact_host_authority_without_global_discovery() {
    let id = SkillId::parse("workspace:w:review").unwrap();
    let revision = SkillRevision::parse(revision('a')).unwrap();
    let source = id.source_id().clone();
    let activation = AgentSkillActivation {
        activation_revision: "activation-sha256-v1:workspace".to_string(),
        skills: vec![AgentActivatedSkill {
            id: id.to_string(),
            name: "Workspace Review".to_string(),
            revision: revision.to_string(),
            source: source.to_string(),
            instructions: SECRET_INSTRUCTIONS.to_string(),
            source_bytes: u64::try_from(SECRET_INSTRUCTIONS.len()).unwrap(),
            resources: None,
        }],
    };
    let authority =
        Arc::new(memory_resource_session_for_test(id, revision, source, Vec::new()).unwrap());

    SkillActivationExtension::new(
        "run-workspace".to_string(),
        None,
        Some(&activation),
        None,
        Some(authority),
    )
    .expect("workspace selection is authorized by its exact request-scoped Host binding");
}

#[test]
fn native_capabilities_require_exact_application_owned_skill_identity() {
    let exact = ActivatedSkillRecord {
        id: format!("{APPLICATION_BUNDLED_SKILL_SOURCE_ID}:{IMAGE_GENERATION_LOCAL_ID}"),
        name: "Image Generation".to_string(),
        revision: revision('a'),
        source: APPLICATION_BUNDLED_SKILL_SOURCE_ID.to_string(),
        source_bytes: 128,
        has_resources: false,
        resource_kinds: Vec::new(),
        activated_by: AgentSkillActivationActor::Model,
    };
    let mut forged_source = exact.clone();
    forged_source.source = "installed:user".to_string();
    let mut forged_id = exact.clone();
    forged_id.id = format!("{APPLICATION_BUNDLED_SKILL_SOURCE_ID}:image-generation-copy");

    assert_eq!(
        tool_capabilities_for_skill(&exact),
        BTreeSet::from([ToolCapabilityId::application_owned(
            IMAGE_GENERATION_CAPABILITY,
        )])
    );
    assert!(tool_capabilities_for_skill(&forged_source).is_empty());
    assert!(tool_capabilities_for_skill(&forged_id).is_empty());

    let installer = ActivatedSkillRecord {
        id: format!("{APPLICATION_BUNDLED_SKILL_SOURCE_ID}:{SKILL_INSTALLER_LOCAL_ID}"),
        name: "Skill Installer".to_string(),
        revision: revision('b'),
        source: APPLICATION_BUNDLED_SKILL_SOURCE_ID.to_string(),
        source_bytes: 256,
        has_resources: false,
        resource_kinds: Vec::new(),
        activated_by: AgentSkillActivationActor::Model,
    };
    let mut installed_same_name = installer.clone();
    installed_same_name.id = "installed:user:skill-installer".to_string();
    installed_same_name.source = "installed:user".to_string();
    let mut bundled_lookalike = installer.clone();
    bundled_lookalike.id = format!("{APPLICATION_BUNDLED_SKILL_SOURCE_ID}:skill-installer-copy");

    assert_eq!(
        tool_capabilities_for_skill(&installer),
        BTreeSet::from([ToolCapabilityId::application_owned(
            SKILL_INSTALLATION_CAPABILITY,
        )])
    );
    assert!(tool_capabilities_for_skill(&installed_same_name).is_empty());
    assert!(tool_capabilities_for_skill(&bundled_lookalike).is_empty());

    let creator = ActivatedSkillRecord {
        id: format!("{APPLICATION_BUNDLED_SKILL_SOURCE_ID}:{SKILL_CREATOR_LOCAL_ID}"),
        name: "Skill Creator".to_string(),
        revision: revision('c'),
        source: APPLICATION_BUNDLED_SKILL_SOURCE_ID.to_string(),
        source_bytes: 512,
        has_resources: true,
        resource_kinds: vec!["other".to_string(), "reference".to_string()],
        activated_by: AgentSkillActivationActor::Model,
    };
    assert_eq!(
        tool_capabilities_for_skill(&creator),
        BTreeSet::from([
            ToolCapabilityId::application_owned(SKILL_RESOURCES_MATERIALIZE_CAPABILITY),
            ToolCapabilityId::application_owned(SKILL_RESOURCES_READ_CAPABILITY),
        ])
    );
}

#[test]
fn verified_resource_kinds_derive_only_the_matching_resource_capabilities() {
    let record = |kinds: &[&str]| ActivatedSkillRecord {
        id: "installed:user:financial-analysis".to_string(),
        name: "Financial Analysis".to_string(),
        revision: revision('a'),
        source: "installed:user".to_string(),
        source_bytes: 128,
        has_resources: true,
        resource_kinds: kinds.iter().map(|kind| (*kind).to_string()).collect(),
        activated_by: AgentSkillActivationActor::Model,
    };
    let read = ToolCapabilityId::application_owned(SKILL_RESOURCES_READ_CAPABILITY);
    let materialize = ToolCapabilityId::application_owned(SKILL_RESOURCES_MATERIALIZE_CAPABILITY);
    let scripts = ToolCapabilityId::application_owned(SKILL_SCRIPTS_CAPABILITY);

    assert_eq!(
        tool_capabilities_for_skill(&record(&["reference"])),
        BTreeSet::from([read.clone()])
    );
    assert_eq!(
        tool_capabilities_for_skill(&record(&["asset"])),
        BTreeSet::from([materialize.clone(), read.clone()])
    );
    assert_eq!(
        tool_capabilities_for_skill(&record(&["script"])),
        BTreeSet::from([read.clone(), scripts.clone()])
    );
    assert_eq!(
        tool_capabilities_for_skill(&record(&["other", "script"])),
        BTreeSet::from([materialize, read, scripts])
    );
}

#[test]
fn initial_activation_cannot_forge_resource_kinds_to_unlock_tools() {
    let entry = discoverable_skill(
        "s1",
        "financial-analysis",
        "Financial Analysis",
        &revision('a'),
    );
    let resolved = resolved_with_resources(
        &entry,
        vec![(
            "references/workflows.md".to_string(),
            SkillResourceKind::Reference,
            b"trusted workflow".to_vec(),
        )],
    );
    let mut skill = resolved.skill;
    skill.resources.as_mut().unwrap().kinds = vec!["script".to_string()];
    let initial = AgentSkillActivation {
        activation_revision: "activation-sha256-v1:forged".to_string(),
        skills: vec![skill],
    };

    let error = SkillActivationExtension::new(
        "run-forged".to_string(),
        Some(discovery(vec![entry], 4, 16_384)),
        Some(&initial),
        None,
        Some(resolved.resources),
    )
    .err()
    .expect("forged resource kinds must be rejected");

    assert!(error
        .to_string()
        .contains("resource capability metadata does not match exact run resource authority"));
}

#[test]
fn activation_rejects_refs_outside_the_frozen_catalog_before_resolving() {
    let entry = discoverable_skill("s1", "documents", "Documents", &revision('a'));
    let calls = Arc::new(AtomicUsize::new(0));
    let extension = SkillActivationExtension::new(
        "run-1".to_string(),
        Some(discovery(vec![entry.clone()], 4, 16_384)),
        None,
        Some(resolver_for(
            vec![resolved_without_resources(&entry, SECRET_INSTRUCTIONS)],
            calls.clone(),
        )),
        Some(Arc::new(SkillResourceSession::empty())),
    )
    .unwrap();
    let tool = SkillsActivateTool {
        state: extension.state,
    };

    let error = tool
        .execute(
            &tool_context(),
            json!({ "skillRef": "s_000000000000000000000000", "reason": "Need document guidance" }),
        )
        .unwrap_err();

    assert_eq!(error.code(), Some("skill.unknownActivationRef"));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn full_instructions_are_disclosed_only_by_the_post_tool_context_effect() {
    let entry = discoverable_skill("s1", "documents", "Documents", &revision('a'));
    let calls = Arc::new(AtomicUsize::new(0));
    let mut extension = SkillActivationExtension::new(
        "run-1".to_string(),
        Some(discovery(vec![entry.clone()], 4, 16_384)),
        None,
        Some(resolver_for(
            vec![resolved_without_resources(&entry, SECRET_INSTRUCTIONS)],
            calls,
        )),
        Some(Arc::new(SkillResourceSession::empty())),
    )
    .unwrap();
    let tool = SkillsActivateTool {
        state: extension.state.clone(),
    };

    assert!(extension
        .request_context(&ModelRequestContext::agent_work())
        .unwrap()
        .is_empty());
    let value = tool
            .execute(
                &tool_context(),
                json!({ "skillRef": entry.activation_ref.clone(), "reason": "Create a polished document" }),
            )
            .unwrap();
    let serialized_result = serde_json::to_string(&value).unwrap();
    assert_eq!(value["status"], "activated");
    assert!(!serialized_result.contains(SECRET_INSTRUCTIONS));
    assert!(extension
        .request_context(&ModelRequestContext::agent_work())
        .unwrap()
        .is_empty());

    let expected_activation_revision = value["activationRevision"]
        .as_str()
        .expect("activation result revision")
        .to_string();
    let result = successful_result(value);
    let effects = extension
        .on_event(&RuntimeExtensionEvent::ToolCompleted { result: &result })
        .unwrap();
    assert_eq!(effects.len(), 2);

    let mut emitted_event = false;
    let mut context = None;
    for effect in effects {
        match effect {
            RuntimeEffect::EmitEvent(event) => match event.as_ref() {
                AgentEvent::SkillActivated {
                    run_id,
                    activation_revision,
                    activated_by,
                    skill,
                } => {
                    emitted_event = true;
                    assert_eq!(run_id, "run-1");
                    assert_eq!(skill.id, entry.id);
                    assert_eq!(*activated_by, AgentSkillActivationActor::Model);
                    assert_eq!(activation_revision, &expected_activation_revision);
                    assert_eq!(skill.source.kind, "bundled");
                    assert_eq!(skill.source.id, "bundled:application");
                }
                _ => panic!("unexpected runtime event"),
            },
            RuntimeEffect::AppendRetainedContext(item) => context = Some(*item),
        }
    }
    assert!(emitted_event);
    let frame = ContextFrame::new(vec![context.expect("missing Skill context")]);
    assert_eq!(
        frame.manifest().entries[0].sources,
        vec!["skill_instructions"]
    );
    let messages = frame.to_messages();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].role(), LlmMessageRole::User);
    assert!(messages[0].content().contains(SECRET_INSTRUCTIONS));
    assert!(messages[0].content().contains("<backend_activated_skill>"));
}

#[test]
fn repeated_activation_is_idempotent_and_does_not_resolve_or_append_twice() {
    let entry = discoverable_skill("s1", "documents", "Documents", &revision('a'));
    let calls = Arc::new(AtomicUsize::new(0));
    let mut extension = SkillActivationExtension::new(
        "run-1".to_string(),
        Some(discovery(vec![entry.clone()], 4, 16_384)),
        None,
        Some(resolver_for(
            vec![resolved_without_resources(&entry, SECRET_INSTRUCTIONS)],
            calls.clone(),
        )),
        Some(Arc::new(SkillResourceSession::empty())),
    )
    .unwrap();
    let tool = SkillsActivateTool {
        state: extension.state.clone(),
    };
    let args =
        json!({ "skillRef": entry.activation_ref.clone(), "reason": "Need document guidance" });

    let first = tool.execute(&tool_context(), args.clone()).unwrap();
    assert_eq!(first["status"], "activated");
    let effects = extension
        .on_event(&RuntimeExtensionEvent::ToolCompleted {
            result: &successful_result(first),
        })
        .unwrap();
    assert_eq!(effects.len(), 2);

    let second = tool.execute(&tool_context(), args).unwrap();
    assert_eq!(second["status"], "alreadyActivated");
    assert_eq!(second["activatedBy"], "model");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(extension
        .on_event(&RuntimeExtensionEvent::ToolCompleted {
            result: &successful_result(second),
        })
        .unwrap()
        .is_empty());
}

#[test]
fn an_explicitly_selected_skill_keeps_user_attribution_when_the_model_reuses_it() {
    let entry = discoverable_skill("s1", "documents", "Documents", &revision('a'));
    let initial_skill = activated_skill(&entry, SECRET_INSTRUCTIONS);
    let initial_activation = AgentSkillActivation {
        activation_revision: "initial-user-selection".to_string(),
        skills: vec![initial_skill],
    };
    let calls = Arc::new(AtomicUsize::new(0));
    let extension = SkillActivationExtension::new(
        "run-user-selected".to_string(),
        Some(discovery(vec![entry.clone()], 4, 16_384)),
        Some(&initial_activation),
        Some(resolver_for(Vec::new(), calls.clone())),
        Some(exact_empty_authority(&entry)),
    )
    .unwrap();
    let tool = SkillsActivateTool {
        state: extension.state,
    };

    let result = tool
        .execute(
            &tool_context(),
            json!({
                "skillRef": entry.activation_ref,
                "reason": "Continue using the explicitly selected Skill"
            }),
        )
        .unwrap();

    assert_eq!(result["status"], "alreadyActivated");
    assert_eq!(result["activatedBy"], "user");
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn model_activation_extends_user_selection_with_the_canonical_inventory_revision() {
    let user_entry = discoverable_skill("user", "documents", "Documents", &revision('a'));
    let model_entry = discoverable_skill("model", "spreadsheets", "Spreadsheets", &revision('b'));
    let initial_activation = AgentSkillActivation {
        // The extension derives the current inventory revision from the frozen identities;
        // callers cannot give progressive activation a second revision meaning.
        activation_revision: "untrusted-caller-revision".to_string(),
        skills: vec![activated_skill(&user_entry, "user-selected instructions")],
    };
    let calls = Arc::new(AtomicUsize::new(0));
    let mut extension = SkillActivationExtension::new(
        "run-mixed-activation".to_string(),
        Some(discovery(
            vec![user_entry.clone(), model_entry.clone()],
            4,
            16_384,
        )),
        Some(&initial_activation),
        Some(resolver_for(
            vec![resolved_without_resources(
                &model_entry,
                "model-activated instructions",
            )],
            calls.clone(),
        )),
        Some(exact_empty_authority(&user_entry)),
    )
    .unwrap();
    let tool = SkillsActivateTool {
        state: extension.state.clone(),
    };

    let value = tool
        .execute(
            &tool_context(),
            json!({
                "skillRef": model_entry.activation_ref,
                "reason": "Need spreadsheet guidance"
            }),
        )
        .unwrap();
    let expected_revision = activation_revision_for_identities([
        (user_entry.id.as_str(), user_entry.revision.as_str()),
        (model_entry.id.as_str(), model_entry.revision.as_str()),
    ])
    .as_str()
    .to_string();

    assert_eq!(value["activationRevision"], expected_revision);
    assert_eq!(value["activatedBy"], "model");
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    let effects = extension
        .on_event(&RuntimeExtensionEvent::ToolCompleted {
            result: &successful_result(value),
        })
        .unwrap();
    let activated_event = effects
        .iter()
        .find_map(|effect| match effect {
            RuntimeEffect::EmitEvent(event) => match event.as_ref() {
                AgentEvent::SkillActivated {
                    activation_revision,
                    activated_by,
                    skill,
                    ..
                } => Some((activation_revision, activated_by, skill)),
                _ => None,
            },
            _ => None,
        })
        .expect("model activation event");
    assert_eq!(activated_event.0, &expected_revision);
    assert_eq!(activated_event.1, &AgentSkillActivationActor::Model);
    assert_eq!(activated_event.2.id, model_entry.id);

    let snapshot = extension.snapshot_state().unwrap();
    assert_eq!(
        snapshot["skills"]
            .as_array()
            .unwrap()
            .iter()
            .map(|skill| skill["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec![user_entry.id.as_str(), model_entry.id.as_str()]
    );
    assert_eq!(snapshot["skills"][0]["activatedBy"], "user");
    assert_eq!(snapshot["skills"][1]["activatedBy"], "model");
}

#[test]
fn activation_enforces_skill_count_and_source_byte_budgets() {
    let first = discoverable_skill("s1", "documents", "Documents", &revision('a'));
    let second = discoverable_skill("s2", "spreadsheets", "Spreadsheets", &revision('b'));
    let count_calls = Arc::new(AtomicUsize::new(0));
    let count_extension = SkillActivationExtension::new(
        "run-count".to_string(),
        Some(discovery(vec![first.clone(), second.clone()], 1, 16_384)),
        None,
        Some(resolver_for(
            vec![
                resolved_without_resources(&first, "first instructions"),
                resolved_without_resources(&second, "second instructions"),
            ],
            count_calls,
        )),
        Some(Arc::new(SkillResourceSession::empty())),
    )
    .unwrap();
    let count_tool = SkillsActivateTool {
        state: count_extension.state,
    };
    count_tool
        .execute(
            &tool_context(),
            json!({ "skillRef": first.activation_ref.clone(), "reason": "Need documents" }),
        )
        .unwrap();
    let count_error = count_tool
        .execute(
            &tool_context(),
            json!({ "skillRef": second.activation_ref.clone(), "reason": "Need spreadsheets too" }),
        )
        .unwrap_err();
    assert_eq!(count_error.code(), Some("skill.tooManySkills"));

    let byte_calls = Arc::new(AtomicUsize::new(0));
    let mut byte_candidate = resolved_without_resources(&first, "123456");
    // A host resolver cannot evade the byte budget by understating sourceBytes: the runtime
    // normalizes it against the actual instruction byte length before checking and storing.
    byte_candidate.skill.source_bytes = 1;
    let byte_extension = SkillActivationExtension::new(
        "run-bytes".to_string(),
        Some(discovery(vec![first.clone()], 1, 5)),
        None,
        Some(resolver_for(vec![byte_candidate], byte_calls)),
        Some(Arc::new(SkillResourceSession::empty())),
    )
    .unwrap();
    let byte_tool = SkillsActivateTool {
        state: byte_extension.state.clone(),
    };
    let byte_error = byte_tool
        .execute(
            &tool_context(),
            json!({ "skillRef": first.activation_ref.clone(), "reason": "Need documents" }),
        )
        .unwrap_err();
    assert_eq!(byte_error.code(), Some("skill.sourceBudgetExceeded"));
    assert!(byte_extension.state.lock().active.is_empty());
}

#[test]
fn activation_rejects_instructions_that_cannot_fit_the_next_model_request() {
    let entry = discoverable_skill("s1", "documents", "Documents", &revision('a'));
    let calls = Arc::new(AtomicUsize::new(0));
    let mut extension = SkillActivationExtension::new(
        "run-capacity".to_string(),
        Some(discovery(vec![entry.clone()], 4, 64 * 1024)),
        None,
        Some(resolver_for(
            vec![resolved_without_resources(
                &entry,
                "complete Skill instructions that require retained context",
            )],
            calls,
        )),
        Some(Arc::new(SkillResourceSession::empty())),
    )
    .unwrap();
    extension.update_model_input_capacity(Some(ModelInputCapacity {
        remaining_tokens: 1,
        text_budget: ContextTextBudget::heuristic(1),
        effective_tool_set: effective_tool_set_with_document_tool(),
    }));
    let tool = SkillsActivateTool {
        state: extension.state.clone(),
    };

    let error = tool
        .execute(
            &tool_context(),
            json!({
                "skillRef": entry.activation_ref,
                "reason": "Need document guidance"
            }),
        )
        .unwrap_err();

    assert_eq!(error.code(), Some("skill.contextCapacityExceeded"));
    let state = extension.state.lock();
    assert!(state.active.is_empty());
    assert!(state.pending_context.is_empty());
    assert_eq!(
        state
            .model_input_capacity
            .as_ref()
            .unwrap()
            .remaining_tokens,
        1
    );
    assert!(state.resources.as_ref().unwrap().is_empty());
}

#[test]
fn activation_capacity_accounts_for_the_retained_assistant_response() {
    let entry = discoverable_skill("s1", "documents", "Documents", &revision('a'));
    let calls = Arc::new(AtomicUsize::new(0));
    let mut extension = SkillActivationExtension::new(
        "run-response-capacity".to_string(),
        Some(discovery(vec![entry.clone()], 4, 64 * 1024)),
        None,
        Some(resolver_for(
            vec![resolved_without_resources(
                &entry,
                "complete Skill instructions that require retained context",
            )],
            calls,
        )),
        Some(Arc::new(SkillResourceSession::empty())),
    )
    .unwrap();
    extension.update_model_input_capacity(Some(ModelInputCapacity {
        remaining_tokens: 257,
        text_budget: ContextTextBudget::heuristic(257),
        effective_tool_set: effective_tool_set_with_document_tool(),
    }));
    extension.consume_model_input_capacity(256);
    let tool = SkillsActivateTool {
        state: extension.state.clone(),
    };

    let error = tool
        .execute(
            &tool_context(),
            json!({
                "skillRef": entry.activation_ref,
                "reason": "Need document guidance"
            }),
        )
        .unwrap_err();

    assert_eq!(error.code(), Some("skill.contextCapacityExceeded"));
    let state = extension.state.lock();
    assert!(state.active.is_empty());
    assert!(state.pending_context.is_empty());
    assert_eq!(
        state
            .model_input_capacity
            .as_ref()
            .unwrap()
            .remaining_tokens,
        1
    );
    assert!(state.resources.as_ref().unwrap().is_empty());
}

#[test]
fn activation_capacity_includes_dynamic_tool_contract_before_commit() {
    let entry = discoverable_skill("s1", "documents", "Documents", &revision('a'));
    let candidate = resolved_without_resources(&entry, "x");
    let record = record_from_skill(&candidate.skill, AgentSkillActivationActor::Model);
    let reason = "Need document guidance";
    let calls = Arc::new(AtomicUsize::new(0));
    let mut extension = SkillActivationExtension::new(
        "run-result-capacity".to_string(),
        Some(discovery(vec![entry.clone()], 4, 64 * 1024)),
        None,
        Some(resolver_for(vec![candidate.clone()], calls)),
        Some(Arc::new(SkillResourceSession::empty())),
    )
    .unwrap();
    let activation_revision = extension
        .state
        .lock()
        .activation_revision_with(&record)
        .unwrap();
    let pending = activated_skill_context_item(&activation_revision, &candidate.skill).unwrap();
    let budget = ContextTextBudget::heuristic(64 * 1024);
    let instruction_tokens = ContextFrame::new(vec![pending.clone()])
        .into_messages()
        .iter()
        .map(|message| budget.estimate_message(message))
        .sum::<u64>();
    let result = activation_result("activated", &record, &activation_revision, reason).unwrap();
    let canonical = canonical_tool_result_for_context(&AgentToolResult {
        exact_archive_file: None,
        call_id: "activate-call-1".to_string(),
        tool: SKILL_ACTIVATE_TOOL_NAME.to_string(),
        ok: true,
        result: Some(result),
        error: None,
    });
    let result_tokens = budget.estimate_message(&LlmMessage::tool_result(
        &canonical.call_id,
        render_tool_observation(&canonical),
        false,
    ));
    let retained_without_dynamic_contract = instruction_tokens.saturating_add(result_tokens);
    extension.update_model_input_capacity(Some(ModelInputCapacity {
        remaining_tokens: retained_without_dynamic_contract,
        text_budget: budget,
        effective_tool_set: effective_tool_set_with_document_tool(),
    }));
    let tool = SkillsActivateTool {
        state: extension.state.clone(),
    };

    let error = tool
        .execute(
            &tool_context(),
            json!({
                "skillRef": entry.activation_ref,
                "reason": reason
            }),
        )
        .unwrap_err();

    assert_eq!(error.code(), Some("skill.contextCapacityExceeded"));
    let state = extension.state.lock();
    assert!(state.active.is_empty());
    assert!(state.pending_context.is_empty());
    assert_eq!(
        state
            .model_input_capacity
            .as_ref()
            .unwrap()
            .remaining_tokens,
        retained_without_dynamic_contract
    );
    assert!(state.resources.as_ref().unwrap().is_empty());
}

#[test]
fn dynamic_tool_capacity_projection_charges_schema_without_host_only_inventory() {
    let budget = ContextTextBudget::heuristic(64 * 1024);
    let capacity = ModelInputCapacity {
        remaining_tokens: 64 * 1024,
        text_budget: budget.clone(),
        effective_tool_set: effective_tool_set_with_document_tool(),
    };
    let projection = capacity
        .project_additional_tool_capabilities(&BTreeSet::from([
            ToolCapabilityId::application_owned(OFFICE_DOCUMENTS_CAPABILITY),
        ]))
        .unwrap();
    let schema_tokens =
        budget.estimate_tool_definitions(projection.effective_tool_set.dynamic_definitions());

    assert!(schema_tokens > 0);
    assert_eq!(
        projection.additional_tokens, schema_tokens,
        "Host-only Tool inventory must not reserve a fabricated model message"
    );
    assert!(projection
        .effective_tool_set
        .contains("capacity_document_tool"));
}

#[test]
fn skill_installer_activation_capacity_charges_both_installation_tool_schemas() {
    let budget = ContextTextBudget::heuristic(64 * 1024);
    let capacity = ModelInputCapacity {
        remaining_tokens: 64 * 1024,
        text_budget: budget.clone(),
        effective_tool_set: effective_tool_set_with_skill_installation_tool(),
    };
    let projection = capacity
        .project_additional_tool_capabilities(&BTreeSet::from([
            ToolCapabilityId::application_owned(SKILL_INSTALLATION_CAPABILITY),
        ]))
        .unwrap();
    let definitions = projection.effective_tool_set.dynamic_definitions();
    let schema_tokens = budget.estimate_tool_definitions(definitions);

    assert_eq!(definitions.len(), 2);
    assert_eq!(definitions[0].name, "skills_commit_install");
    assert_eq!(definitions[1].name, "skills_prepare_install");
    assert!(schema_tokens > 0);
    assert_eq!(projection.additional_tokens, schema_tokens);
}

#[test]
fn failed_activation_results_reduce_capacity_before_later_activations() {
    let entry = discoverable_skill("s1", "documents", "Documents", &revision('a'));
    let calls = Arc::new(AtomicUsize::new(0));
    let mut extension = SkillActivationExtension::new(
        "run-failed-result-capacity".to_string(),
        Some(discovery(vec![entry.clone()], 4, 64 * 1024)),
        None,
        Some(resolver_for(
            vec![resolved_without_resources(&entry, "instructions")],
            calls.clone(),
        )),
        Some(Arc::new(SkillResourceSession::empty())),
    )
    .unwrap();
    let failed_result = AgentToolResult {
        exact_archive_file: None,
        call_id: "failed-activate".to_string(),
        tool: SKILL_ACTIVATE_TOOL_NAME.to_string(),
        ok: false,
        result: Some(json!({
            "type": "skillActivation",
            "code": "skill.unknownActivationRef",
            "recovery": "useAvailableRef"
        })),
        error: Some("Skill activation failed.".to_string()),
    };
    let canonical = canonical_tool_result_for_context(&failed_result);
    let budget = ContextTextBudget::heuristic(64 * 1024);
    let failed_result_tokens = budget.estimate_message(&LlmMessage::tool_result(
        &canonical.call_id,
        render_tool_observation(&canonical),
        true,
    ));
    extension.update_model_input_capacity(Some(ModelInputCapacity {
        remaining_tokens: failed_result_tokens,
        text_budget: budget,
        effective_tool_set: effective_tool_set_with_document_tool(),
    }));

    assert!(extension
        .on_event(&RuntimeExtensionEvent::ToolCompleted {
            result: &failed_result,
        })
        .unwrap()
        .is_empty());
    assert_eq!(
        extension
            .state
            .lock()
            .model_input_capacity
            .as_ref()
            .unwrap()
            .remaining_tokens,
        0
    );

    let tool = SkillsActivateTool {
        state: extension.state.clone(),
    };
    let error = tool
        .execute(
            &tool_context(),
            json!({
                "skillRef": entry.activation_ref,
                "reason": "Need document guidance"
            }),
        )
        .unwrap_err();

    assert_eq!(error.code(), Some("skill.contextCapacityExceeded"));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let state = extension.state.lock();
    assert!(state.active.is_empty());
    assert!(state.pending_context.is_empty());
    assert!(state.resources.as_ref().unwrap().is_empty());
}

#[test]
fn successful_activation_result_and_instructions_are_not_charged_twice() {
    let entry = discoverable_skill("s1", "documents", "Documents", &revision('a'));
    let mut extension = SkillActivationExtension::new(
        "run-success-capacity".to_string(),
        Some(discovery(vec![entry.clone()], 4, 64 * 1024)),
        None,
        Some(resolver_for(
            vec![resolved_without_resources(&entry, "instructions")],
            Arc::new(AtomicUsize::new(0)),
        )),
        Some(Arc::new(SkillResourceSession::empty())),
    )
    .unwrap();
    extension.update_model_input_capacity(Some(ModelInputCapacity {
        remaining_tokens: 64 * 1024,
        text_budget: ContextTextBudget::heuristic(64 * 1024),
        effective_tool_set: effective_tool_set_with_document_tool(),
    }));
    let tool = SkillsActivateTool {
        state: extension.state.clone(),
    };
    let value = tool
        .execute(
            &tool_context(),
            json!({
                "skillRef": entry.activation_ref,
                "reason": "Need document guidance"
            }),
        )
        .unwrap();
    let remaining_after_execute = extension
        .state
        .lock()
        .model_input_capacity
        .as_ref()
        .unwrap()
        .remaining_tokens;
    assert!(extension
        .state
        .lock()
        .model_input_capacity
        .as_ref()
        .unwrap()
        .effective_tool_set
        .contains("capacity_document_tool"));

    let effects = extension
        .on_event(&RuntimeExtensionEvent::ToolCompleted {
            result: &successful_result(value),
        })
        .unwrap();

    assert_eq!(effects.len(), 2);
    assert_eq!(
        extension
            .state
            .lock()
            .model_input_capacity
            .as_ref()
            .unwrap()
            .remaining_tokens,
        remaining_after_execute
    );
}

#[test]
fn dynamic_activation_fails_closed_when_model_capacity_is_unknown() {
    let entry = discoverable_skill("s1", "documents", "Documents", &revision('a'));
    let calls = Arc::new(AtomicUsize::new(0));
    let mut extension = SkillActivationExtension::new(
        "run-unknown-capacity".to_string(),
        Some(discovery(vec![entry.clone()], 4, 64 * 1024)),
        None,
        Some(resolver_for(
            vec![resolved_without_resources(&entry, "instructions")],
            calls.clone(),
        )),
        Some(Arc::new(SkillResourceSession::empty())),
    )
    .unwrap();
    // Production runtime invokes this after every accepted model request. `None` means the
    // host supplied no authoritative context window, not that capacity is unlimited.
    extension.update_model_input_capacity(None);
    let tool = SkillsActivateTool {
        state: extension.state.clone(),
    };

    let error = tool
        .execute(
            &tool_context(),
            json!({
                "skillRef": entry.activation_ref,
                "reason": "Need document guidance"
            }),
        )
        .unwrap_err();

    assert_eq!(error.code(), Some("skill.contextCapacityUnavailable"));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(extension.state.lock().active.is_empty());
}

#[test]
fn resolved_candidate_rejects_additional_or_foreign_resource_packages() {
    let entry = discoverable_skill("s1", "documents", "Documents", &revision('a'));
    let other = discoverable_skill("s2", "spreadsheets", "Spreadsheets", &revision('b'));
    let candidate = resolved_with_resources(
        &entry,
        vec![(
            "references/workflows.md".to_string(),
            SkillResourceKind::Reference,
            b"trusted workflow".to_vec(),
        )],
    );
    let other_session = candidate_resource_session(&other, Vec::new());
    candidate.resources.extend_from(&other_session).unwrap();

    let error = validate_resolved_candidate(&entry, &candidate).unwrap_err();

    assert_eq!(error.code(), Some("skill.activationContractViolation"));
    assert!(error.to_string().contains("package bindings"));

    let foreign_only = AgentResolvedSkillActivation {
        skill: activated_skill(&entry, SECRET_INSTRUCTIONS),
        resources: Arc::new(other_session),
    };
    let error = validate_resolved_candidate(&entry, &foreign_only).unwrap_err();
    assert_eq!(error.code(), Some("skill.activationContractViolation"));
    assert!(error.to_string().contains("instead of"));
}

#[test]
fn resolved_candidate_requires_resource_metadata_to_match_the_exact_binding() {
    let entry = discoverable_skill("s1", "documents", "Documents", &revision('a'));
    let candidate = resolved_with_resources(
        &entry,
        vec![
            (
                "assets/theme.bin".to_string(),
                SkillResourceKind::Asset,
                b"theme".to_vec(),
            ),
            (
                "references/workflows.md".to_string(),
                SkillResourceKind::Reference,
                b"workflow".to_vec(),
            ),
        ],
    );
    assert!(validate_resolved_candidate(&entry, &candidate)
        .unwrap()
        .is_some());

    let mut wrong_root = candidate.clone();
    wrong_root.skill.resources.as_mut().unwrap().root_uri = SkillPackageUri::new(
        SkillId::parse("bundled:application:other").unwrap(),
        SkillRevision::parse(revision('c')).unwrap(),
    )
    .to_string();
    let mut wrong_count = candidate.clone();
    wrong_count.skill.resources.as_mut().unwrap().resource_count += 1;
    let mut wrong_kinds = candidate.clone();
    wrong_kinds
        .skill
        .resources
        .as_mut()
        .unwrap()
        .kinds
        .reverse();
    let mut missing_binding = candidate.clone();
    missing_binding.resources = Arc::new(SkillResourceSession::empty());

    for invalid in [wrong_root, wrong_count, wrong_kinds, missing_binding] {
        let error = validate_resolved_candidate(&entry, &invalid).unwrap_err();
        assert_eq!(error.code(), Some("skill.activationContractViolation"));
    }
}

#[test]
fn resolved_candidate_without_metadata_requires_exact_empty_binding() {
    let entry = discoverable_skill("s1", "documents", "Documents", &revision('a'));
    let no_binding = AgentResolvedSkillActivation {
        skill: activated_skill(&entry, SECRET_INSTRUCTIONS),
        resources: Arc::new(SkillResourceSession::empty()),
    };
    let error = validate_resolved_candidate(&entry, &no_binding).unwrap_err();
    assert_eq!(error.code(), Some("skill.activationContractViolation"));
    assert!(error.to_string().contains("no exact package authority"));

    let exact_empty = AgentResolvedSkillActivation {
        skill: activated_skill(&entry, SECRET_INSTRUCTIONS),
        resources: Arc::new(candidate_resource_session(&entry, Vec::new())),
    };
    assert!(validate_resolved_candidate(&entry, &exact_empty)
        .unwrap()
        .is_some());

    let undeclared = AgentResolvedSkillActivation {
        skill: activated_skill(&entry, SECRET_INSTRUCTIONS),
        resources: Arc::new(candidate_resource_session(
            &entry,
            vec![(
                "references/hidden.md".to_string(),
                SkillResourceKind::Reference,
                b"hidden".to_vec(),
            )],
        )),
    };
    let error = validate_resolved_candidate(&entry, &undeclared).unwrap_err();
    assert_eq!(error.code(), Some("skill.activationContractViolation"));
    assert!(error.to_string().contains("undeclared resources"));
}

#[test]
fn checkpoint_contains_only_activation_summaries_and_restores_resource_selections() {
    let entry = discoverable_skill("s1", "documents", "Documents", &revision('a'));
    let skill_id = SkillId::parse(entry.id.clone()).unwrap();
    let skill_revision = SkillRevision::parse(entry.revision.clone()).unwrap();
    let source_id = SkillSourceId::parse("bundled:application").unwrap();
    let candidate_session = memory_resource_session_for_test(
        skill_id,
        skill_revision,
        source_id,
        vec![(
            "references/workflows.md".to_string(),
            SkillResourceKind::Reference,
            b"trusted workflow".to_vec(),
        )],
    )
    .unwrap();
    let package_uri = candidate_session.package_uris()[0].to_string();
    let mut skill = activated_skill(&entry, SECRET_INSTRUCTIONS);
    skill.resources = Some(AgentActivatedSkillResources {
        root_uri: package_uri,
        resource_count: 1,
        kinds: vec!["reference".to_string()],
    });
    let candidate = AgentResolvedSkillActivation {
        skill,
        resources: Arc::new(candidate_session),
    };
    let calls = Arc::new(AtomicUsize::new(0));
    let extension = SkillActivationExtension::new(
        "run-1".to_string(),
        Some(discovery(vec![entry.clone()], 2, 16_384)),
        None,
        Some(resolver_for(vec![candidate], calls)),
        Some(Arc::new(SkillResourceSession::empty())),
    )
    .unwrap();
    let tool = SkillsActivateTool {
        state: extension.state.clone(),
    };
    tool.execute(
        &tool_context(),
        json!({ "skillRef": entry.activation_ref.clone(), "reason": "Need the bundled workflow" }),
    )
    .unwrap();

    let state = extension.snapshot_state().unwrap();
    let serialized = serde_json::to_string(&state).unwrap();
    assert!(!serialized.contains(SECRET_INSTRUCTIONS));
    assert!(!serialized.contains("instructions"));
    assert!(!serialized.contains("pendingContext"));
    assert_eq!(state["skills"][0]["id"], entry.id);
    assert_eq!(state["skills"][0]["hasResources"], true);
    assert_eq!(state["skills"][0]["resourceKinds"], json!(["reference"]));

    let (_, selections) = checkpoint_authority_from_snapshots(&[AgentExtensionSnapshot {
        extension_id: SKILL_EXTENSION_ID.to_string(),
        version: SKILL_EXTENSION_VERSION,
        state,
    }])
    .unwrap()
    .expect("Skill checkpoint authority");
    assert_eq!(selections.len(), 1);
    assert_eq!(selections[0].skill_id().as_str(), entry.id);
    assert_eq!(selections[0].expected_revision().as_str(), entry.revision);
}

#[test]
fn checkpoint_restores_instruction_only_managed_package_authority() {
    let entry = discoverable_skill("s1", DOCUMENTS_LOCAL_ID, "Documents", &revision('a'));
    let extension = SkillActivationExtension::new(
        "run-checkpoint".to_string(),
        Some(discovery(vec![entry.clone()], 2, 16_384)),
        None,
        Some(resolver_for(
            vec![resolved_without_resources(&entry, SECRET_INSTRUCTIONS)],
            Arc::new(AtomicUsize::new(0)),
        )),
        Some(Arc::new(SkillResourceSession::empty())),
    )
    .unwrap();
    SkillsActivateTool {
        state: extension.state.clone(),
    }
    .execute(
        &tool_context(),
        json!({
            "skillRef": entry.activation_ref,
            "reason": "Need document capabilities"
        }),
    )
    .unwrap();
    let state = extension.snapshot_state().unwrap();
    let snapshots = [AgentExtensionSnapshot {
        extension_id: SKILL_EXTENSION_ID.to_string(),
        version: SKILL_EXTENSION_VERSION,
        state: state.clone(),
    }];

    let (_, selections) = checkpoint_authority_from_snapshots(&snapshots)
        .unwrap()
        .expect("managed checkpoint authority");
    assert_eq!(selections.len(), 1);
    assert_eq!(selections[0].skill_id().as_str(), entry.id);

    let mut missing_authority = SkillActivationExtension::new_for_checkpoint_restore(
        "run-checkpoint".to_string(),
        None,
        Some(Arc::new(SkillResourceSession::empty())),
    );
    let error = missing_authority
        .restore_state(SKILL_EXTENSION_VERSION, state.clone())
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("no exact Host package authority"));

    let mut restored = SkillActivationExtension::new_for_checkpoint_restore(
        "run-checkpoint".to_string(),
        None,
        Some(exact_empty_authority(&entry)),
    );
    restored
        .restore_state(SKILL_EXTENSION_VERSION, state)
        .expect("exact immutable package authority restores the managed activation");
    assert_eq!(
        restored.active_tool_capabilities().unwrap(),
        BTreeSet::from([ToolCapabilityId::application_owned(
            OFFICE_DOCUMENTS_CAPABILITY,
        )])
    );
}

#[test]
fn restore_rejects_model_activations_outside_the_frozen_catalog() {
    let entry = discoverable_skill("s1", "documents", "Documents", &revision('a'));
    let mut extension = SkillActivationExtension::new(
        "run-restore".to_string(),
        Some(discovery(vec![entry.clone()], 2, 16_384)),
        None,
        None,
        Some(Arc::new(SkillResourceSession::empty())),
    )
    .unwrap();
    let tampered = SkillExtensionSnapshot {
        discovery: Some(discovery(vec![entry], 2, 16_384)),
        skills: vec![ActivatedSkillRecord {
            id: "bundled:application:spreadsheets".to_string(),
            name: "Spreadsheets".to_string(),
            revision: revision('b'),
            source: "bundled:application".to_string(),
            source_bytes: 128,
            has_resources: false,
            resource_kinds: Vec::new(),
            activated_by: AgentSkillActivationActor::Model,
        }],
    };

    let error = extension
        .restore_state(
            SKILL_EXTENSION_VERSION,
            serde_json::to_value(tampered).unwrap(),
        )
        .unwrap_err();

    assert!(error
        .to_string()
        .contains("absent from the frozen discovery catalog"));
    assert!(extension.state.lock().active.is_empty());
}

#[test]
fn restore_rejects_legacy_or_noncanonical_resource_capability_metadata() {
    let mut extension = SkillActivationExtension::new_for_checkpoint_restore(
        "run-restore".to_string(),
        None,
        Some(Arc::new(SkillResourceSession::empty())),
    );
    let state = SkillExtensionSnapshot {
        discovery: None,
        skills: vec![ActivatedSkillRecord {
            id: "installed:user:financial-analysis".to_string(),
            name: "Financial Analysis".to_string(),
            revision: revision('a'),
            source: "installed:user".to_string(),
            source_bytes: 128,
            has_resources: true,
            resource_kinds: vec!["script".to_string(), "reference".to_string()],
            activated_by: AgentSkillActivationActor::User,
        }],
    };

    let legacy_error = extension
        .restore_state(
            SKILL_EXTENSION_VERSION - 1,
            serde_json::to_value(&state).unwrap(),
        )
        .unwrap_err();
    assert!(legacy_error.to_string().contains("expected 3"));

    let metadata_error = extension
        .restore_state(
            SKILL_EXTENSION_VERSION,
            serde_json::to_value(state).unwrap(),
        )
        .unwrap_err();
    assert!(metadata_error
        .to_string()
        .contains("sorted, unique, and use known resource kinds"));
    assert!(extension.state.lock().active.is_empty());
}

#[test]
fn terminal_redaction_removes_catalog_text_and_drops_unknown_skill_state() {
    let entry = discoverable_skill("s1", "documents", "Documents", &revision('a'));
    let state = SkillExtensionSnapshot {
        discovery: Some(discovery(vec![entry.clone()], 2, 16_384)),
        skills: vec![ActivatedSkillRecord {
            id: entry.id,
            name: entry.name,
            revision: entry.revision,
            source: "bundled:application".to_string(),
            source_bytes: 128,
            has_resources: false,
            resource_kinds: Vec::new(),
            activated_by: AgentSkillActivationActor::Model,
        }],
    };
    let mut snapshots = vec![
        AgentExtensionSnapshot {
            extension_id: SKILL_EXTENSION_ID.to_string(),
            version: SKILL_EXTENSION_VERSION,
            state: serde_json::to_value(state).unwrap(),
        },
        AgentExtensionSnapshot {
            extension_id: SKILL_EXTENSION_ID.to_string(),
            version: SKILL_EXTENSION_VERSION + 1,
            state: json!({ "futureCatalog": "MUST_NOT_SURVIVE" }),
        },
        AgentExtensionSnapshot {
            extension_id: "todo".to_string(),
            version: 1,
            state: json!({ "safe": true }),
        },
    ];

    redact_discovery_from_snapshots(&mut snapshots);

    assert_eq!(snapshots.len(), 2);
    assert_eq!(snapshots[0].extension_id, SKILL_EXTENSION_ID);
    assert_eq!(snapshots[0].state["discovery"], Value::Null);
    assert_eq!(snapshots[0].state["skills"].as_array().unwrap().len(), 1);
    assert_eq!(snapshots[1].extension_id, "todo");
    assert!(!serde_json::to_string(&snapshots)
        .unwrap()
        .contains("MUST_NOT_SURVIVE"));
}
