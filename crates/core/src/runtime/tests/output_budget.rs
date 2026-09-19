use super::*;
use crate::protocol::AgentApiStyle;
use crate::provider_profile::{
    MoonshotK26ThinkingMode, ProviderFamilyReasoningPolicy, ProviderFamilySettings,
    ProviderProfileRef, ProviderReasoningEffort, ProviderVendorId, ReasoningMode,
};

fn input_for_profile(
    profile: ProviderProfileConfig,
    model: &str,
    api_style: AgentApiStyle,
) -> AgentChatInput {
    let mut input = conversation_context_input(vec![message("user", "Hello")]);
    input.provider_profile_config = Some(profile);
    input.model = model.into();
    input.api_style = Some(api_style);
    input.max_tokens = None;
    input.context_window_tokens = Some(512 * 1024);
    input
}

fn profile_cases() -> Vec<(AgentChatInput, Option<u32>, u32)> {
    let mut cases = vec![];
    for (dialect, api_style, wire) in [
        (
            ProviderProtocolDialect::OpenAiChatCompletions,
            AgentApiStyle::OpenAiCompatible,
            None,
        ),
        (
            ProviderProtocolDialect::AnthropicMessages,
            AgentApiStyle::AnthropicCompatible,
            Some(30_000),
        ),
    ] {
        cases.push((
            input_for_profile(
                ProviderProfileConfig::generic_for_dialect(dialect),
                "custom-model",
                api_style,
            ),
            wire,
            30_000,
        ));
    }
    for (model, profile_ref, pro) in [
        (
            "deepseek-flash",
            ProviderProfileRef::deepseek_v4_1_flash_chat(),
            false,
        ),
        (
            "deepseek-v4-pro",
            ProviderProfileRef::deepseek_v4_pro_0813_chat(),
            true,
        ),
    ] {
        for (mode, effort, reserve) in [
            (
                ReasoningMode::ProviderDefault,
                ProviderReasoningEffort::ProviderDefault,
                65_536,
            ),
            (ReasoningMode::Enabled, ProviderReasoningEffort::Low, 65_536),
            (
                ReasoningMode::Enabled,
                ProviderReasoningEffort::High,
                65_536,
            ),
            (
                ReasoningMode::Enabled,
                ProviderReasoningEffort::Max,
                131_072,
            ),
            (
                ReasoningMode::Disabled,
                ProviderReasoningEffort::ProviderDefault,
                8_192,
            ),
        ] {
            let reasoning = ProviderFamilyReasoningPolicy { mode, effort };
            let settings = if pro {
                ProviderFamilySettings::DeepseekProChat { reasoning }
            } else {
                ProviderFamilySettings::DeepseekFlashChat { reasoning }
            };
            cases.push((
                input_for_profile(
                    ProviderProfileConfig::from_family_settings(
                        profile_ref,
                        ProviderVendorId::DeepSeek,
                        settings,
                    ),
                    model,
                    AgentApiStyle::OpenAiCompatible,
                ),
                None,
                reserve,
            ));
        }
    }
    for (model, profile_ref, settings, reserve) in [
        (
            "kimi-k3",
            ProviderProfileRef::moonshot_k3_chat(),
            ProviderFamilySettings::MoonshotK3Chat {
                reasoning_effort: ProviderReasoningEffort::Max,
            },
            131_072,
        ),
        (
            "kimi-k2.7-code",
            ProviderProfileRef::moonshot_k2_7_code_chat(),
            ProviderFamilySettings::MoonshotK27CodeChat,
            30_000,
        ),
        (
            "kimi-k2.6",
            ProviderProfileRef::moonshot_k2_6_chat(),
            ProviderFamilySettings::MoonshotK26Chat {
                thinking_mode: MoonshotK26ThinkingMode::ProviderDefault,
            },
            30_000,
        ),
    ] {
        cases.push((
            input_for_profile(
                ProviderProfileConfig::from_family_settings(
                    profile_ref,
                    ProviderVendorId::Moonshot,
                    settings,
                ),
                model,
                AgentApiStyle::OpenAiCompatible,
            ),
            None,
            reserve,
        ));
    }
    cases
}

#[test]
fn all_profiles_share_output_reservations_between_preview_and_execution() {
    for (input, wire_limit, reserve) in profile_cases() {
        let capabilities =
            prepare_runtime_capabilities(&input, "budget-test", &[], true, None).unwrap();
        let tools = capabilities.initial_tool_set.stable_definitions();
        let mut prepared = build_llm_request(input.clone(), tools, None, None, None).unwrap();
        let snapshot = inspect_context_window(input.clone()).unwrap().unwrap();
        assert_eq!(prepared.template.max_tokens, wire_limit, "{}", input.model);
        assert_eq!(prepared.template.reserved_output_tokens, reserve);
        assert_eq!(snapshot.reserved_output_tokens, u64::from(reserve));
        let detector =
            ContextCapacityDetector::for_model(&input.model, input.api_style.unwrap(), tools);
        let actual = detector.inspect(
            &mut prepared.context,
            input.context_window_tokens,
            prepared.template.reserved_output_tokens,
        );
        assert_eq!(snapshot.input_tokens, actual.usage.request_input_tokens());
        assert_eq!(
            snapshot.input_capacity_tokens,
            actual.available_input_tokens
        );
        assert_eq!(snapshot.safety_margin_tokens, actual.safety_margin_tokens);
        detector.ensure_sendable(actual).unwrap();
        // Request construction (including subsequent tool exchanges) never promotes a reserve
        // to an HTTP limit. The same immutable template is used for each request in the run.
        assert_eq!(
            prepared.template.request(prepared.context, &[]).max_tokens,
            wire_limit
        );
    }
}

#[test]
fn explicit_legacy_and_large_limits_are_preserved_without_a_universal_clamp() {
    for (mut input, _, _) in profile_cases() {
        for limit in [1_024, 30_000, 131_072, 256_000] {
            input.max_tokens = Some(limit);
            let prepared = build_llm_request(input.clone(), &[], None, None, None).unwrap();
            let snapshot = create_conversation_context_state(input.clone())
                .unwrap()
                .snapshot();
            assert_eq!(prepared.template.max_tokens, Some(limit));
            assert_eq!(prepared.template.reserved_output_tokens, limit);
            assert_eq!(snapshot.reserved_output_tokens, u64::from(limit));
        }
    }
}

#[test]
fn zero_limit_is_rejected_instead_of_turning_into_a_provider_default() {
    let mut input = conversation_context_input(vec![]);
    input.max_tokens = Some(0);
    let error = build_llm_request(input.clone(), &[], None, None, None)
        .err()
        .unwrap();
    assert_eq!(error.code(), Some("agent.invalid_output_limit"));
    assert_eq!(
        create_conversation_context_state(input)
            .err()
            .unwrap()
            .code(),
        Some("agent.invalid_output_limit")
    );
}

#[test]
fn omission_and_explicit_equal_reserve_have_distinct_context_revisions() {
    let mut input = conversation_context_input(vec![]);
    let explicit_revision = conversation_context_configuration_revision(&input).unwrap();
    input.max_tokens = None;
    assert_eq!(reserved_output_tokens(&input), 30_000);
    assert_ne!(
        explicit_revision,
        conversation_context_configuration_revision(&input).unwrap()
    );
}

#[test]
fn small_or_missing_windows_do_not_erase_output_reservations() {
    for (mut input, _, reserve) in profile_cases() {
        for window in [None, Some(reserve), Some(1_024)] {
            input.context_window_tokens = window;
            let mut prepared = build_llm_request(input.clone(), &[], None, None, None).unwrap();
            let detector =
                ContextCapacityDetector::for_model(&input.model, input.api_style.unwrap(), &[]);
            let report = detector.inspect(
                &mut prepared.context,
                window,
                prepared.template.reserved_output_tokens,
            );
            assert_eq!(report.reserved_output_tokens, u64::from(reserve));
            assert_eq!(detector.ensure_sendable(report).is_err(), window.is_some());
            let snapshot = create_conversation_context_state(input.clone())
                .unwrap()
                .snapshot();
            assert_eq!(snapshot.reserved_output_tokens, u64::from(reserve));
            if window.is_some() {
                assert_eq!(snapshot.input_capacity_tokens, Some(0));
            }
        }
    }
}

#[test]
fn provider_default_larger_than_the_legacy_context_window_reports_configuration_error() {
    for (mut input, _, reserve) in profile_cases()
        .into_iter()
        .filter(|(_, _, reserve)| *reserve > 128_000)
    {
        input.context_window_tokens = Some(128_000);
        let mut prepared = build_llm_request(input.clone(), &[], None, None, None).unwrap();
        let detector =
            ContextCapacityDetector::for_model(&input.model, input.api_style.unwrap(), &[]);
        let report = detector.inspect(
            &mut prepared.context,
            input.context_window_tokens,
            prepared.template.reserved_output_tokens,
        );
        let error = detector.ensure_sendable(report).unwrap_err();
        assert_eq!(error.code(), Some("invalid_context_capacity_configuration"));
        assert!(error.to_string().contains("上下文窗口"));
        assert_eq!(prepared.template.reserved_output_tokens, reserve);
        assert_eq!(prepared.template.max_tokens, None);
    }
}

#[test]
fn capacity_gate_accounts_for_input_plus_reserve_and_safety_at_the_boundary() {
    let mut input = conversation_context_input(vec![message("user", &"hello ".repeat(2_000))]);
    input.max_tokens = None;
    let mut prepared = build_llm_request(input.clone(), &[], None, None, None).unwrap();
    let detector = ContextCapacityDetector::for_model(&input.model, input.api_style.unwrap(), &[]);
    let measured = detector.inspect(
        &mut prepared.context,
        None,
        prepared.template.reserved_output_tokens,
    );
    // Find the first sendable window using the production margin calculation; its immediate
    // predecessor must fail. No token count approximation is baked into this assertion.
    let base = u32::try_from(measured.usage.request_input_tokens()).unwrap()
        + prepared.template.reserved_output_tokens;
    let (mut low, mut high) = (base, base + 20_000);
    while low < high {
        let middle = low + (high - low) / 2;
        if detector
            .ensure_sendable(detector.inspect(
                &mut prepared.context,
                Some(middle),
                prepared.template.reserved_output_tokens,
            ))
            .is_ok()
        {
            high = middle;
        } else {
            low = middle + 1;
        }
    }
    let first_fitting_window = low;
    for (window, sendable) in [
        (first_fitting_window - 1, false),
        (first_fitting_window, true),
        (first_fitting_window + 1, true),
    ] {
        let report = detector.inspect(
            &mut prepared.context,
            Some(window),
            prepared.template.reserved_output_tokens,
        );
        assert_eq!(report.reserved_output_tokens, 30_000);
        assert_eq!(detector.ensure_sendable(report).is_ok(), sendable);
    }
}
