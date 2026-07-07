// Rust agent core.
use crate::protocol::AgentUsage;
use serde_json::Value;

pub(crate) fn extract_usage(value: &Value) -> Option<AgentUsage> {
    usage_from_usage_value(value.get("usage")?)
}

pub(crate) fn extract_anthropic_stream_usage(value: &Value) -> Option<AgentUsage> {
    value
        .get("usage")
        .and_then(usage_from_usage_value)
        .or_else(|| {
            value
                .get("message")
                .and_then(|message| message.get("usage"))
                .and_then(usage_from_usage_value)
        })
}

pub(crate) fn merge_stream_usage(target: &mut Option<AgentUsage>, next: Option<AgentUsage>) {
    let Some(next) = next else {
        return;
    };
    let existing = target.take();
    let input_tokens = next
        .input_tokens
        .or_else(|| existing.as_ref().and_then(|usage| usage.input_tokens));
    let output_tokens = next
        .output_tokens
        .or_else(|| existing.as_ref().and_then(|usage| usage.output_tokens));
    let total_tokens = next
        .total_tokens
        .or_else(|| existing.as_ref().and_then(|usage| usage.total_tokens))
        .or_else(|| match (input_tokens, output_tokens) {
            (Some(input), Some(output)) => Some(input + output),
            _ => None,
        });
    let cached_input_tokens = next.cached_input_tokens.or_else(|| {
        existing
            .as_ref()
            .and_then(|usage| usage.cached_input_tokens)
    });
    let cache_creation_input_tokens = next.cache_creation_input_tokens.or_else(|| {
        existing
            .as_ref()
            .and_then(|usage| usage.cache_creation_input_tokens)
    });
    let billable_request_count = next.billable_request_count.or_else(|| {
        existing
            .as_ref()
            .and_then(|usage| usage.billable_request_count)
    });

    *target = Some(AgentUsage {
        input_tokens,
        output_tokens,
        total_tokens,
        cached_input_tokens,
        cache_creation_input_tokens,
        billable_request_count,
    });
}

pub(crate) fn merge_total_usage(total: &mut Option<AgentUsage>, next: Option<AgentUsage>) {
    let Some(next) = next else {
        return;
    };

    match total {
        Some(total) => {
            total.input_tokens = sum_optional(total.input_tokens, next.input_tokens);
            total.output_tokens = sum_optional(total.output_tokens, next.output_tokens);
            total.total_tokens = sum_optional(total.total_tokens, next.total_tokens);
            total.cached_input_tokens =
                sum_optional(total.cached_input_tokens, next.cached_input_tokens);
            total.cache_creation_input_tokens = sum_optional(
                total.cache_creation_input_tokens,
                next.cache_creation_input_tokens,
            );
            total.billable_request_count = sum_optional(
                total.billable_request_count,
                next.billable_request_count.or(Some(1)),
            );
        }
        None => *total = Some(with_default_billable_request_count(next)),
    }
}

fn usage_from_usage_value(usage: &Value) -> Option<AgentUsage> {
    let input_tokens = usage
        .get("prompt_tokens")
        .or_else(|| usage.get("input_tokens"))
        .and_then(Value::as_u64);
    let output_tokens = usage
        .get("completion_tokens")
        .or_else(|| usage.get("output_tokens"))
        .and_then(Value::as_u64);
    let total_tokens = usage
        .get("total_tokens")
        .and_then(Value::as_u64)
        .or_else(|| match (input_tokens, output_tokens) {
            (Some(input), Some(output)) => Some(input + output),
            _ => None,
        });
    let cached_input_tokens = usage
        .get("prompt_tokens_details")
        .and_then(|details| details.get("cached_tokens"))
        .or_else(|| {
            usage
                .get("input_tokens_details")
                .and_then(|details| details.get("cached_tokens"))
        })
        .or_else(|| usage.get("cache_read_input_tokens"))
        .or_else(|| usage.get("cached_input_tokens"))
        .or_else(|| usage.get("cached_tokens"))
        .or_else(|| usage.get("prompt_cache_hit_tokens"))
        .and_then(Value::as_u64);
    let cache_creation_input_tokens = usage
        .get("cache_creation_input_tokens")
        .or_else(|| usage.get("cache_creation_tokens"))
        .or_else(|| usage.get("prompt_cache_miss_tokens"))
        .and_then(Value::as_u64);

    if input_tokens.is_none()
        && output_tokens.is_none()
        && total_tokens.is_none()
        && cached_input_tokens.is_none()
        && cache_creation_input_tokens.is_none()
    {
        return None;
    }

    Some(AgentUsage {
        input_tokens,
        output_tokens,
        total_tokens,
        cached_input_tokens,
        cache_creation_input_tokens,
        billable_request_count: Some(1),
    })
}

fn with_default_billable_request_count(mut usage: AgentUsage) -> AgentUsage {
    if usage.billable_request_count.is_none() {
        usage.billable_request_count = Some(1);
    }
    usage
}

fn sum_optional(left: Option<u64>, right: Option<u64>) -> Option<u64> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left + right),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extracts_openai_and_anthropic_usage() {
        let openai = json!({
            "usage": {
                "prompt_tokens": 7,
                "completion_tokens": 5,
                "total_tokens": 12,
                "prompt_tokens_details": {
                    "cached_tokens": 2
                }
            }
        });
        let anthropic = json!({
            "usage": {
                "input_tokens": 3,
                "output_tokens": 4,
                "cache_read_input_tokens": 2,
                "cache_creation_input_tokens": 1
            }
        });

        let openai_usage = extract_usage(&openai).unwrap();
        assert_eq!(openai_usage.total_tokens, Some(12));
        assert_eq!(openai_usage.cached_input_tokens, Some(2));
        assert_eq!(openai_usage.billable_request_count, Some(1));

        let anthropic_usage = extract_usage(&anthropic).unwrap();
        assert_eq!(anthropic_usage.total_tokens, Some(7));
        assert_eq!(anthropic_usage.cached_input_tokens, Some(2));
        assert_eq!(anthropic_usage.cache_creation_input_tokens, Some(1));
        assert_eq!(anthropic_usage.billable_request_count, Some(1));
    }

    #[test]
    fn merges_stream_usage_without_double_counting_one_request() {
        let mut total = None;
        merge_stream_usage(
            &mut total,
            Some(AgentUsage {
                input_tokens: Some(10),
                output_tokens: None,
                total_tokens: None,
                cached_input_tokens: Some(3),
                cache_creation_input_tokens: None,
                billable_request_count: Some(1),
            }),
        );
        merge_stream_usage(
            &mut total,
            Some(AgentUsage {
                input_tokens: None,
                output_tokens: Some(5),
                total_tokens: None,
                cached_input_tokens: None,
                cache_creation_input_tokens: Some(2),
                billable_request_count: Some(1),
            }),
        );

        let total = total.unwrap();
        assert_eq!(total.input_tokens, Some(10));
        assert_eq!(total.output_tokens, Some(5));
        assert_eq!(total.total_tokens, Some(15));
        assert_eq!(total.cached_input_tokens, Some(3));
        assert_eq!(total.cache_creation_input_tokens, Some(2));
        assert_eq!(total.billable_request_count, Some(1));
    }

    #[test]
    fn sums_usage_across_model_requests() {
        let mut total = None;
        merge_total_usage(
            &mut total,
            Some(AgentUsage {
                input_tokens: Some(10),
                output_tokens: Some(5),
                total_tokens: Some(15),
                cached_input_tokens: Some(2),
                cache_creation_input_tokens: None,
                billable_request_count: Some(1),
            }),
        );
        merge_total_usage(
            &mut total,
            Some(AgentUsage {
                input_tokens: Some(20),
                output_tokens: Some(7),
                total_tokens: Some(27),
                cached_input_tokens: None,
                cache_creation_input_tokens: Some(3),
                billable_request_count: Some(1),
            }),
        );

        let total = total.unwrap();
        assert_eq!(total.input_tokens, Some(30));
        assert_eq!(total.output_tokens, Some(12));
        assert_eq!(total.total_tokens, Some(42));
        assert_eq!(total.cached_input_tokens, Some(2));
        assert_eq!(total.cache_creation_input_tokens, Some(3));
        assert_eq!(total.billable_request_count, Some(2));
    }
}
