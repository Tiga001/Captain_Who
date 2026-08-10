use super::*;

pub(crate) fn handle_core_ping(id: JsonRpcId, params: Option<Value>) -> Value {
    let input =
        parse_params::<CorePingRequest>(params).unwrap_or(CorePingRequest { message: None });
    response_success(
        id,
        CorePingResponse {
            message: "pong",
            echo: input.message,
            server_time_ms: now_ms(),
        },
    )
}

pub(crate) fn handle_agent_start_conversation_turn(
    agent_service: &AgentService,
    notification_tx: agent::CoreServerNotificationSender,
    id: JsonRpcId,
    params: Option<Value>,
) -> Value {
    let input = match parse_params::<AgentConversationTurnInput>(params) {
        Ok(input) => input,
        Err(message) => return response_error(Some(id), -32602, message),
    };

    match agent_service.start_conversation_turn(input, notification_tx) {
        Ok(output) => response_success(id, output),
        Err(error) => agent_service_error_response(id, error),
    }
}

pub(crate) fn handle_agent_preflight_provider_transition(
    agent_service: &AgentService,
    id: JsonRpcId,
    params: Option<Value>,
) -> Value {
    let input = match parse_params::<AgentProviderTransitionPreflightInput>(params) {
        Ok(input) => input,
        Err(message) => return response_error(Some(id), -32602, message),
    };
    match agent_service.preflight_provider_transition(input) {
        Ok(output) => response_success(id, output),
        Err(error) => agent_service_error_response(id, error),
    }
}

pub(crate) fn handle_agent_start_provider_transition(
    agent_service: &AgentService,
    notification_tx: agent::CoreServerNotificationSender,
    id: JsonRpcId,
    params: Option<Value>,
) -> Value {
    let input = match parse_params::<AgentProviderTransitionStartInput>(params) {
        Ok(input) => input,
        Err(message) => return response_error(Some(id), -32602, message),
    };
    match agent_service.start_provider_transition(input, notification_tx) {
        Ok(output) => response_success(id, output),
        Err(error) => agent_service_error_response(id, error),
    }
}

pub(crate) fn handle_agent_get_provider_transition_status(
    agent_service: &AgentService,
    id: JsonRpcId,
    params: Option<Value>,
) -> Value {
    let input = match parse_params::<AgentProviderTransitionGetStatusInput>(params) {
        Ok(input) => input,
        Err(message) => return response_error(Some(id), -32602, message),
    };
    match agent_service.get_provider_transition_status(input) {
        Ok(output) => response_success(id, output),
        Err(error) => agent_service_error_response(id, error),
    }
}

pub(crate) fn handle_agent_cancel_run(
    agent_service: &AgentService,
    id: JsonRpcId,
    params: Option<Value>,
) -> Value {
    let input = match parse_params::<AgentCancelRunRequest>(params) {
        Ok(input) => input,
        Err(message) => return response_error(Some(id), -32602, message),
    };

    let cancelled = agent_service.cancel_run(&input.run_id);
    response_success(
        id,
        AgentCancelRunResponse {
            run_id: input.run_id,
            cancelled,
        },
    )
}

pub(crate) fn handle_agent_list_command_sessions(
    agent_service: &AgentService,
    id: JsonRpcId,
    params: Option<Value>,
) -> Value {
    let input = match parse_params::<AgentCommandSessionListInput>(params) {
        Ok(input) => input,
        Err(message) => return response_error(Some(id), -32602, message),
    };

    match agent_service.list_command_sessions(input) {
        Ok(output) => response_success(id, output),
        Err(message) => response_error(Some(id), -32000, message),
    }
}

pub(crate) fn handle_agent_get_command_session(
    agent_service: &AgentService,
    id: JsonRpcId,
    params: Option<Value>,
) -> Value {
    let input = match parse_params::<AgentCommandSessionGetInput>(params) {
        Ok(input) => input,
        Err(message) => return response_error(Some(id), -32602, message),
    };

    match agent_service.get_command_session(input) {
        Ok(output) => response_success(id, output),
        Err(message) => response_error(Some(id), -32000, message),
    }
}

pub(crate) fn handle_agent_steer_run(
    agent_service: &AgentService,
    notification_tx: agent::CoreServerNotificationSender,
    id: JsonRpcId,
    params: Option<Value>,
) -> Value {
    let input = match parse_params::<AgentSteerRunInput>(params) {
        Ok(input) => input,
        Err(message) => return response_error(Some(id), -32602, message),
    };

    match agent_service.steer_run(input, notification_tx) {
        Ok(output) => response_success(id, output),
        Err(error) => agent_service_error_response(id, error),
    }
}

pub(crate) fn handle_agent_approve_action(
    agent_service: &AgentService,
    notification_tx: agent::CoreServerNotificationSender,
    id: JsonRpcId,
    params: Option<Value>,
) -> Value {
    let input = match parse_params::<AgentActionIdRequest>(params) {
        Ok(input) => input,
        Err(message) => return response_error(Some(id), -32602, message),
    };

    match agent_service.approve_action(&input.run_id, &input.action_id, notification_tx) {
        Ok(output) => response_success(id, output),
        Err(message) => response_error(Some(id), -32000, message),
    }
}

pub(crate) fn handle_agent_reject_action(
    agent_service: &AgentService,
    notification_tx: agent::CoreServerNotificationSender,
    id: JsonRpcId,
    params: Option<Value>,
) -> Value {
    let input = match parse_params::<AgentRejectActionRequest>(params) {
        Ok(input) => input,
        Err(message) => return response_error(Some(id), -32602, message),
    };

    match agent_service.reject_action(
        &input.run_id,
        &input.action_id,
        input.message,
        notification_tx,
    ) {
        Ok(output) => response_success(id, output),
        Err(message) => response_error(Some(id), -32000, message),
    }
}

pub(crate) fn handle_agent_cancel_action(
    agent_service: &AgentService,
    id: JsonRpcId,
    params: Option<Value>,
) -> Value {
    let input = match parse_params::<AgentActionIdRequest>(params) {
        Ok(input) => input,
        Err(message) => return response_error(Some(id), -32602, message),
    };

    match agent_service.cancel_action(&input.run_id, &input.action_id) {
        Ok(cancelled) => response_success(id, cancelled),
        Err(message) => response_error(Some(id), -32000, message),
    }
}

pub(crate) fn handle_agent_usage_summary(
    agent_service: &AgentService,
    id: JsonRpcId,
    params: Option<Value>,
) -> Value {
    let input = match parse_params::<AgentUsageSummaryInput>(params) {
        Ok(input) => input,
        Err(message) => return response_error(Some(id), -32602, message),
    };

    match agent_service.get_usage_summary(&input) {
        Ok(summary) => response_success(id, summary),
        Err(message) => response_error(Some(id), -32000, message),
    }
}

pub(crate) fn handle_agent_clear_usage_records(
    agent_service: &AgentService,
    id: JsonRpcId,
    params: Option<Value>,
) -> Value {
    let input = match parse_params::<AgentUsageClearInput>(params) {
        Ok(input) => input,
        Err(message) => return response_error(Some(id), -32602, message),
    };

    match agent_service.clear_usage_records(&input) {
        Ok(output) => response_success(id, output),
        Err(message) => response_error(Some(id), -32000, message),
    }
}
