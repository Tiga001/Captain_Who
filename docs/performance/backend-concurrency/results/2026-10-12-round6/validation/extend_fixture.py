from pathlib import Path
p=Path('crates/core-server/src/application/agent/tests/backend_concurrency_baseline.rs')
s=p.read_text().replace('    SteadyText,\n','    SteadyText,\n    Mixed,\n',1)
s=s.replace('    response: Option<Value>,','    response: Option<Value>,\n    response_bytes: usize,',1)
s=s.replace('    timer_lag_us: Vec<u64>,','    timer_lag_us: Vec<u64>,\n    resource_samples: Vec<Value>,',1)
s=s.replace('                response: None,','                response: None,\n                response_bytes: 0,',1)
s=s.replace('    let needs_approval = matches!(case.work, Work::Approval)','    let wave = unit.split("-wave-").nth(1).and_then(|part| part.parse::<usize>().ok()).unwrap_or(0);\n    let mixed = matches!(case.work, Work::Mixed);\n    let needs_approval = mixed && wave % 5 == 3 || matches!(case.work, Work::Approval)',1)
s=s.replace('    if matches!(case.work, Work::Stop) {','    if matches!(case.work, Work::Stop) || mixed && wave % 5 == 4 {',1)
s=s.replace('    write_text_stream(\n        stream,\n        &format!("Baseline complete. {}", "output ".repeat(32)),\n    )\n    .await;', '''    if mixed {
        stream.write_all(b"HTTP/1.1 200 OK\\r\\nContent-Type: text/event-stream\\r\\nConnection: close\\r\\n\\r\\n").await.unwrap();
        // Same complete 243-byte answer as the baseline, delivered over 320ms to exercise ticks.
        let content = format!("Baseline complete. {}", "output ".repeat(32));
        for chunk in content.as_bytes().chunks(16) {
            let frame = json!({"choices":[{"delta":{"content":std::str::from_utf8(chunk).unwrap()},"finish_reason":null}]});
            stream.write_all(format!("data: {frame}\\n\\n").as_bytes()).await.unwrap();
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        stream.write_all(b"data: {\\"choices\\":[{\\"delta\\":{},\\"finish_reason\\":\\"stop\\"}]}\\n\\ndata: [DONE]\\n\\n").await.unwrap();
    } else {
        write_text_stream(stream, &format!("Baseline complete. {}", "output ".repeat(32))).await;
    }''',1)
s=s.replace('command:if matches!(case.work,Work::Approval|Work::ApprovalBehindPreparation)', 'command:if matches!(case.work, Work::Mixed) && wave % 5 == 3 || matches!(case.work,Work::Approval|Work::ApprovalBehindPreparation)',1)
s=s.replace('async fn run_case(case: Case) -> Value {','''fn mixed_interval_ms(case: &Case) -> u64 {
    if matches!(case.work, Work::Mixed) {
        std::env::var("BACKEND_BASELINE_INTERVAL_MS").ok().map(|value| value.parse().expect("integer interval")).unwrap_or(2_000)
    } else { 2_000 }
}

async fn run_case(case: Case) -> Value {
    let interval_ms = mixed_interval_ms(&case);
    assert!((250..=10_000).contains(&interval_ms));''',1)
s=s.replace('case.waves.saturating_sub(1) as u64 * 2_000','case.waves.saturating_sub(1) as u64 * interval_ms',1)
s=s.replace('                        record.response = Some(value);','''                        record.response_bytes = value.to_string().len();
                        // Bound the harness itself: never retain full growing history responses.
                        record.response = Some(if record.operation == "storage.loadConversation" && value.get("error").is_none() {
                            json!({"id":id,"result":{"observed":true,"messages":value["result"]["messages"].as_array().map(Vec::len)}})
                        } else { value });''',1)
needle='    let mut next_id = 1_i64;\n    for wave in 0..case.waves {'
replacement='''    let mut next_id = 1_i64;
    let mut controlled = HashSet::<String>::new();
    let mut next_read = scheduled_epoch;
    let mut next_resource = scheduled_epoch;
    let mut read_owner = 0;
    let mut drive_mixed = |next_id: &mut i64| {
        if !matches!(case.work, Work::Mixed) { return; }
        let now = us(epoch.elapsed());
        // Read frequency is fixed and independent of response/completion; overload is retained.
        while now >= next_read {
            submit(&rpc, &storage, &service, &outbound, &events, epoch, *next_id, read_owner,
                "background_query", next_read, "storage.loadConversation",
                json!({"conversationId":format!("baseline-{read_owner}")}));
            *next_id += 1;
            read_owner = (read_owner + 1) % case.roots;
            next_read += 500_000;
        }
        for action in service.list_pending_actions() {
            if controlled.insert(action.run_id.clone()) {
                let owner = events.lock().unwrap().rpcs.values().find(|record| record.response.as_ref()
                    .is_some_and(|response| response["result"]["runId"] == action.run_id)).map(|record| record.owner).unwrap_or(0);
                submit(&rpc, &storage, &service, &outbound, &events, epoch, *next_id, owner,
                    "mixed_approval", now, AGENT_APPROVE_ACTION_METHOD,
                    json!({"runId":action.run_id,"actionId":action.action_id,"approvalScope":"singleAction"}));
                *next_id += 1;
            }
        }
        let stops = {
            let state = events.lock().unwrap();
            state.rpcs.values().filter_map(|record| {
                let key = record.provider_key.as_ref()?;
                let wave: usize = key.split("-wave-").nth(1)?.parse().ok()?;
                if wave % 5 != 4 || !state.provider_first_us.contains_key(key) { return None; }
                Some((record.owner, record.response.as_ref()?["result"]["runId"].as_str()?.to_owned()))
            }).collect::<Vec<_>>()
        };
        for (owner, run) in stops {
            if controlled.insert(run.clone()) {
                submit(&rpc, &storage, &service, &outbound, &events, epoch, *next_id, owner,
                    "mixed_stop", now, AGENT_CANCEL_RUN_METHOD, json!({"runId":run}));
                *next_id += 1;
            }
        }
        if now >= next_resource {
            sample(&events, |state| state.resource_samples.push(json!({"at_us":now,
                "active_turns":service.turn_concurrency_gate().active(),"disk":disk_files(&path),
                "usage":usage(),"queues":mycopilot_core::performance::take_queue_snapshot()})));
            next_resource += 1_000_000;
        }
    };
    for wave in 0..case.waves {'''
assert needle in s;s=s.replace(needle,replacement,1)
s=s.replace('        tokio::time::sleep_until(start_at + Duration::from_millis(wave as u64 * 2_000)).await;', '''        let deadline = start_at + Duration::from_millis(wave as u64 * interval_ms);
        while tokio::time::Instant::now() < deadline {
            drive_mixed(&mut next_id);
            tokio::time::sleep_until(deadline.min(tokio::time::Instant::now() + Duration::from_millis(20))).await;
        }
        drive_mixed(&mut next_id);''',1)
s=s.replace('scheduled_epoch + wave as u64 * 2_000_000,','scheduled_epoch + wave as u64 * interval_ms * 1_000,',1)
s=s.replace('    let starts_responded = until(Duration::from_secs(60), || {','''    if matches!(case.work, Work::Mixed) {
        let drain = tokio::time::Instant::now() + Duration::from_secs(30);
        loop {
            drive_mixed(&mut next_id);
            let state = events.lock().unwrap();
            let awaiting = state.rpcs.values().any(|record| record.response.is_none());
            drop(state);
            if !awaiting && service.turn_concurrency_gate().active() == 0 { break; }
            if tokio::time::Instant::now() >= drain { break; }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
    drop(drive_mixed);
    let starts_responded = until(Duration::from_secs(60), || {''',1)
s=s.replace('            && r.response\n', '            && r.operation != "storage.loadConversation"\n            && r.response\n',1)
s=s.replace('        AGENT_CANCEL_RUN_METHOD,\n    ] {','        AGENT_CANCEL_RUN_METHOD,\n        "storage.loadConversation",\n    ] {',1)
s=s.replace('"fixed_wave_interval_ms":2_000,','"fixed_wave_interval_ms":interval_ms,\n        "mixed_workload":if matches!(case.work,Work::Mixed) {json!({"stream_chunk_bytes":16,"stream_interval_ms":20,"query_interval_ms":500,"cycle":["text","text","text","approval","stop"]})} else {Value::Null},',1)
# Existing smoke/full remain exactly the same schedule; extension is opt-in.
s=s.replace('    let selected = cases\n','''    if std::env::var("BACKEND_BASELINE_MODE").as_deref() == Ok("soak") {
        let units = std::env::var("BACKEND_BASELINE_UNITS").ok().map(|value| value.parse::<usize>().expect("integer units")).unwrap_or(20);
        assert!([1,5,10,20,50].contains(&units));
        let interval = std::env::var("BACKEND_BASELINE_INTERVAL_MS").ok().map(|value| value.parse::<u64>().expect("integer interval")).unwrap_or(2_000);
        let duration = std::env::var("BACKEND_BASELINE_DURATION_SECONDS").ok().map(|value| value.parse::<u64>().expect("integer duration")).unwrap_or(600);
        assert!((10..=1_800).contains(&duration));
        assert!((250..=10_000).contains(&interval));
        cases = vec![Case {units,roots:units,history_pairs:0,work:Work::Mixed,slow_seconds:0,waves:(duration * 1000 / interval + 1) as usize}];
    }
    let selected = cases\n''',1)
s=s.replace('        Work::SteadyText,\n','        Work::SteadyText,\n        Work::Mixed,\n',1)
p.write_text(s)
