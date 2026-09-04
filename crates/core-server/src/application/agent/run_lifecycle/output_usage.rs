fn replace_output_usage(output: &mut AgentChatOutput, cumulative_usage: Option<AgentUsage>) {
    let usage = cumulative_usage.or_else(|| output.usage.clone());
    output.usage = usage.clone();
    for event in &mut output.events {
        if let AgentEvent::Done {
            run_id,
            usage: event_usage,
            ..
        } = event
        {
            if run_id == &output.run_id {
                *event_usage = usage.clone();
            }
        }
    }
}
