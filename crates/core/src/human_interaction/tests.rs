use super::*;
use serde_json::json;

#[test]
fn shared_host_fixture_roundtrips_without_changing_wire_shape() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../packages/protocol/fixtures/human-interaction-v1.json"
    ))
    .unwrap();
    fn check<T: serde::de::DeserializeOwned + serde::Serialize>(value: &serde_json::Value) {
        let decoded: T = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(decoded).unwrap(), *value);
    }
    check::<HumanInteractionSettings>(&fixture["settings"]);
    check::<HumanInteractionSettingsUpdate>(&fixture["settingsUpdate"]);
    check::<HumanInteractionSettings>(&fixture["settingsUpdated"]);
    check::<HumanInteractionToolInput>(&fixture["toolInput"]);
    check::<HumanInteractionRequestSnapshot>(&fixture["request"]);
    check::<HumanInteractionListInput>(&fixture["listInput"]);
    check::<HumanInteractionSubmitInput>(&fixture["submit"]);
    check::<HumanInteractionIgnoreInput>(&fixture["ignore"]);
    assert!(serde_json::from_value::<HumanInteractionListInput>(
        json!({"conversationId":"c", "limit":50})
    )
    .is_err());
}

#[test]
fn model_input_rejects_authority_and_invalid_questions_without_question_count_cap() {
    assert!(serde_json::from_value::<HumanInteractionToolInput>(
        json!({"questions":[{"title":"why"}],"rootAgentId":"root"})
    )
    .is_err());
    for questions in [
        json!([]),
        json!([{"title":" "}]),
        json!([{"title":"why","options":[]}]),
        json!([{"title":"why","options":["A"," A "]}]),
    ] {
        let input = serde_json::from_value(json!({"questions":questions})).unwrap();
        assert!(validate_human_interaction_tool_input(&input).is_err());
    }
    let input = HumanInteractionToolInput {
        questions: (0..100)
            .map(|i| HumanInteractionQuestionInput {
                title: format!("问题 {i}"),
                options: None,
            })
            .collect(),
    };
    assert!(validate_human_interaction_tool_input(&input).is_ok());
}

#[test]
fn whole_batch_answers_are_exclusive_complete_and_ordered() {
    let questions = vec![
        HumanInteractionQuestion {
            id: "q1".into(),
            title: "格式".into(),
            options: Some(vec![HumanInteractionOption {
                id: "o1".into(),
                label: "CSV".into(),
            }]),
        },
        HumanInteractionQuestion {
            id: "q2".into(),
            title: "名称".into(),
            options: None,
        },
    ];
    let answers = vec![
        HumanInteractionAnswer::Skipped {
            question_id: "q2".into(),
        },
        HumanInteractionAnswer::Option {
            question_id: "q1".into(),
            option_id: "o1".into(),
        },
    ];
    assert_eq!(
        validate_human_interaction_answers(&questions, &answers).unwrap()[0].question_id(),
        "q1"
    );
    assert!(validate_human_interaction_answers(&questions, &answers[..1]).is_err());
    assert!(validate_human_interaction_answers(
        &questions,
        &[answers[0].clone(), answers[0].clone()]
    )
    .is_err());
    let mut wrong = answers.clone();
    wrong[1] = HumanInteractionAnswer::Option {
        question_id: "q1".into(),
        option_id: "foreign".into(),
    };
    assert!(validate_human_interaction_answers(&questions, &wrong).is_err());
    assert!(serde_json::from_value::<HumanInteractionAnswer>(
        json!({"kind":"skipped","questionId":"q1","text":"hidden"})
    )
    .is_err());
}

#[test]
fn text_and_payload_limits_count_utf8_bytes_without_silently_truncating() {
    let mut input = HumanInteractionToolInput {
        questions: vec![HumanInteractionQuestionInput {
            title: "中".repeat(HUMAN_INTERACTION_MAX_TITLE_BYTES / 3),
            options: None,
        }],
    };
    assert!(validate_human_interaction_tool_input(&input).is_ok());
    input.questions[0].title.push('中');
    assert!(validate_human_interaction_tool_input(&input).is_err());
    input.questions[0].title = "a".repeat(HUMAN_INTERACTION_MAX_TITLE_BYTES);
    input.questions = vec![
        input.questions[0].clone();
        HUMAN_INTERACTION_MAX_INPUT_BYTES / HUMAN_INTERACTION_MAX_TITLE_BYTES
    ];
    assert!(
        validate_human_interaction_tool_input(&input).is_err(),
        "JSON overhead is part of the request limit"
    );
    let question = HumanInteractionQuestion {
        id: "q".into(),
        title: "约束".into(),
        options: None,
    };
    for text in [
        " ".into(),
        "隐藏\0字符".into(),
        "x".repeat(HUMAN_INTERACTION_MAX_ANSWER_BYTES + 1),
    ] {
        let answers = [HumanInteractionAnswer::Text {
            question_id: "q".into(),
            text,
        }];
        assert!(
            validate_human_interaction_answers(std::slice::from_ref(&question), &answers).is_err()
        );
    }
    for id in ["", " id", "id\n", "id\u{80}"] {
        assert!(validate_human_interaction_id(id).is_err());
    }
}
