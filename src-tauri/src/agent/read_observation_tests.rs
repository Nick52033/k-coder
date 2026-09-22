use super::*;

fn read_call(id: usize, start_line: usize, line_count: usize) -> ProviderEvent {
    ProviderEvent::ToolCall {
        call: ToolCall {
            id: format!("read-{id}"),
            name: "read_file".into(),
            arguments: json!({"path": "source.txt", "startLine": start_line, "lineCount": line_count}),
            metadata: json!({}),
        },
    }
}

#[tokio::test]
async fn hard_provider_call_budget_survives_continuation() {
    let (_directory, _repository, runtime, thread_id) = runtime_fixture().await;
    let scripts = (0..4)
        .map(|id| {
            vec![
                Ok(ProviderEvent::ToolCall {
                    call: ToolCall {
                        id: format!("budget-call-{id}"),
                        name: "list_directory".into(),
                        arguments: json!({"path": format!("missing-{id}")}),
                        metadata: json!({}),
                    },
                }),
                Ok(ProviderEvent::Completed),
            ]
        })
        .collect::<Vec<_>>();
    let provider = Arc::new(FakeProvider::script(scripts));
    let publisher = Arc::new(UserInputResolvingPublisher::new(
        runtime.user_input_manager(),
        TURN_CONTINUE,
    ));
    let outcome = runtime
        .with_provider_call_budget(3)
        .with_soft_turn_limits(SoftTurnLimits::new(1, u64::MAX, u64::MAX))
        .run_turn(
            provider.clone(),
            "fake".into(),
            RunTurnRequest {
                thread_id,
                input: "inspect until the task is complete".into(),
                agent_mode: None,
            },
            CancellationToken::new(),
            publisher.clone(),
        )
        .await
        .unwrap();

    assert_eq!(outcome.state, TurnState::Failed, "{outcome:?}");
    assert!(
        outcome
            .error
            .as_deref()
            .is_some_and(|error| error.contains("模型调用硬上限")),
        "{outcome:?}"
    );
    assert_eq!(provider.requests().len(), 3);
    let events = publisher
        .events
        .lock()
        .unwrap();
    let failure = events
        .iter()
        .find_map(|event| match &event.event {
            AgentEvent::TurnFailed { error: Some(error), .. } => Some(error),
            _ => None,
        })
        .expect("hard provider call limit should publish a structured error");
    assert_eq!(failure.code, "provider_call_limit_exceeded");
    assert_eq!(failure.details.as_ref().unwrap()["providerCalls"], 3);
    assert_eq!(failure.details.as_ref().unwrap()["maxProviderCalls"], 3);
    assert_eq!(failure.details.as_ref().unwrap()["recovery"], "new_turn");
}

#[tokio::test]
async fn successful_repeated_reads_keep_the_body_and_allow_completion() {
    let (directory, repository, runtime, thread_id) = runtime_fixture().await;
    let body = "first line\nimplementation detail\nlast line";
    std::fs::write(directory.path().join("source.txt"), body).unwrap();
    let mut script = (0..5)
        .map(|id| vec![Ok(read_call(id, 1, 3)), Ok(ProviderEvent::Completed)])
        .collect::<Vec<_>>();
    script.push(vec![
        Ok(ProviderEvent::TextDelta {
            delta: "analysis complete".into(),
        }),
        Ok(ProviderEvent::Completed),
    ]);
    let provider = Arc::new(FakeProvider::script(script));
    let outcome = runtime
        .run_turn(
            provider.clone(),
            "fake".into(),
            RunTurnRequest {
                thread_id: thread_id.clone(),
                input: "analyze source".into(),
                agent_mode: None,
            },
            CancellationToken::new(),
            Arc::new(RecordingPublisher::default()),
        )
        .await
        .unwrap();

    assert_eq!(outcome.state, TurnState::Completed, "{:?}", outcome.error);
    let events = repository.load(&thread_id).await.unwrap();
    let results = events
        .iter()
        .filter_map(|event| match &event.kind {
            StoredEventKind::ToolResult { name, result, .. } if name == "read_file" => Some(result),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(results.len(), 5);
    for result in results {
        assert!(result.success);
        assert!(result.output.contains(body));
        assert_ne!(result.metadata["contentSuppressed"], true);
    }
    let requests = provider.requests();
    assert_eq!(requests.len(), 6);
    assert!(requests.last().unwrap().messages.iter().any(|message| matches!(
        message, ProviderMessage::ToolResult { call_id, output, .. }
        if call_id == "read-4" && output.contains(body) && output.contains("already observed")
    )));
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event.kind, StoredEventKind::UserMessage { .. }))
            .count(),
        1
    );
}

#[tokio::test]
async fn repeated_reads_with_varied_ranges_still_reach_the_global_no_progress_guard() {
    let (directory, repository, runtime, thread_id) = runtime_fixture().await;
    let body = (1..=100).map(|n| format!("line {n}\n")).collect::<String>();
    std::fs::write(directory.path().join("source.txt"), body).unwrap();
    let script = (0..25)
        .map(|id| {
            vec![
                Ok(read_call(id, id + 1, 100 - id)),
                Ok(ProviderEvent::Completed),
            ]
        })
        .collect();
    let provider = Arc::new(FakeProvider::script(script));
    let outcome = runtime
        .run_turn(
            provider.clone(),
            "fake".into(),
            RunTurnRequest {
                thread_id: thread_id.clone(),
                input: "inspect".into(),
                agent_mode: None,
            },
            CancellationToken::new(),
            Arc::new(RecordingPublisher::default()),
        )
        .await
        .unwrap();
    assert_eq!(outcome.state, TurnState::Failed);
    assert!(
        outcome.error.as_deref().unwrap().contains("无实质进展"),
        "{:?}",
        outcome.error
    );
    assert_eq!(
        provider.requests().len(),
        PROGRESS_CHECK_WINDOW * (MAX_NO_PROGRESS_WINDOWS + 1)
    );
    assert!(
        repository
            .load(&thread_id)
            .await
            .unwrap()
            .iter()
            .filter_map(|event| match &event.kind {
                StoredEventKind::ToolResult { result, .. } => Some(result),
                _ => None,
            })
            .all(|result| result.success && result.output.contains("line 100"))
    );
}

#[test]
fn progress_snapshot_compares_merged_read_coverage() {
    let event = |start, end| {
        StoredEvent::new(
            "thread",
            Some("turn".into()),
            StoredEventKind::ToolResult {
                call_id: format!("read-{start}-{end}"),
                name: "read_file".into(),
                result: versioned_read_result("source.txt", "revision", start, end),
            },
        )
    };
    let original = vec![event(1, 100)];
    let mut repeated = original.clone();
    repeated.extend([event(2, 100), event(20, 80), event(1, 60)]);
    assert!(ProgressSnapshot::from_events(&original) == ProgressSnapshot::from_events(&repeated));
    repeated.push(event(99, 101));
    assert!(ProgressSnapshot::from_events(&original) != ProgressSnapshot::from_events(&repeated));
}

#[test]
fn mostly_overlapping_read_returns_the_new_lines() {
    let mut tracker = ReadObservationTracker::default();
    let first = versioned_read_result("source.txt", "revision", 1, 100);
    tracker.observe(&first).unwrap();
    let next = versioned_read_result("source.txt", "revision", 2, 101);
    let decision = tracker.observe(&next).unwrap();
    let outcome = read_observation_result(next, decision);
    assert!(outcome.success);
    assert_eq!(outcome.output, "file contents");
    assert_ne!(outcome.metadata["contentSuppressed"], true);
}

#[tokio::test]
async fn repeated_compaction_keeps_bodies_without_resetting_global_progress() {
    for (read_count, expected_state, expected_reads) in
        [(12, TurnState::Completed, 12), (25, TurnState::Failed, 20)]
    {
        let (directory, repository, runtime, thread_id) = runtime_fixture().await;
        let body = (1..=2_000)
            .map(|n| format!("line {n:04} context {}\n", "x".repeat(20)))
            .collect::<String>();
        std::fs::write(directory.path().join("source.txt"), body).unwrap();
        let mut script = (0..read_count)
            .map(|id| {
                vec![
                    Ok(read_call(id, id + 1, 2_000 - id)),
                    Ok(ProviderEvent::Completed),
                ]
            })
            .collect::<Vec<_>>();
        script.push(vec![
            Ok(ProviderEvent::TextDelta {
                delta: "done after compaction".into(),
            }),
            Ok(ProviderEvent::Completed),
        ]);
        let publisher = Arc::new(UserInputResolvingPublisher::new(
            runtime.user_input_manager(),
            TURN_COMPACT_AND_CONTINUE,
        ));
        let provider = Arc::new(FakeProvider::script(script));
        let outcome = runtime
            .with_context_limit(64_000)
            .with_soft_turn_limits(SoftTurnLimits::new(2, u64::MAX, u64::MAX))
            .run_turn(
                provider.clone(),
                "fake".into(),
                RunTurnRequest {
                    thread_id: thread_id.clone(),
                    input: "continue reading after compaction".into(),
                    agent_mode: None,
                },
                CancellationToken::new(),
                publisher,
            )
            .await
            .unwrap();
        assert_eq!(outcome.state, expected_state, "{:?}", outcome.error);
        if expected_state == TurnState::Failed {
            assert!(outcome.error.as_deref().unwrap().contains("无实质进展"));
            assert_eq!(provider.requests().len(), 20);
        }
        let events = repository.load(&thread_id).await.unwrap();
        assert!(
            events
                .iter()
                .filter(|event| matches!(event.kind, StoredEventKind::ContextCompacted { .. }))
                .count()
                >= 2
        );
        let reads = events
            .iter()
            .filter_map(|event| match &event.kind {
                StoredEventKind::ToolResult { name, result, .. } if name == "read_file" => {
                    Some(result)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(reads.len(), expected_reads);
        assert!(
            reads
                .iter()
                .all(|result| result.success && result.output.contains("line 2000 context"))
        );
    }
}

#[test]
fn progress_snapshot_tracks_byte_pages_within_the_same_line() {
    let event = |offset, bytes| {
        let mut result = versioned_read_result("source.txt", "revision", 1, 1);
        result.metadata["offset"] = json!(offset);
        result.metadata["bytesReturned"] = json!(bytes);
        StoredEvent::new(
            "thread",
            Some("turn".into()),
            StoredEventKind::ToolResult {
                call_id: format!("byte-{offset}"),
                name: "read_file".into(),
                result,
            },
        )
    };
    let first = vec![event(0, 100)];
    let mut next = first.clone();
    next.push(event(100, 100));
    assert!(ProgressSnapshot::from_events(&first) != ProgressSnapshot::from_events(&next));
    let mut repeated = next.clone();
    repeated.push(event(50, 100));
    assert!(ProgressSnapshot::from_events(&next) == ProgressSnapshot::from_events(&repeated));
}

#[test]
fn empty_byte_pages_at_different_offsets_do_not_change_progress() {
    let event = |offset| {
        let mut result = versioned_read_result("source.txt", "revision", 1, 1);
        result.output.clear();
        result.metadata["offset"] = json!(offset);
        result.metadata["bytesReturned"] = json!(0);
        StoredEvent::new(
            "thread",
            Some("turn".into()),
            StoredEventKind::ToolResult {
                call_id: format!("empty-{offset}"),
                name: "read_file".into(),
                result,
            },
        )
    };
    let first = vec![event(0)];
    let mut repeated = first.clone();
    repeated.extend([event(3), event(6), event(9)]);
    assert!(ProgressSnapshot::from_events(&[]) != ProgressSnapshot::from_events(&first));
    assert!(ProgressSnapshot::from_events(&first) == ProgressSnapshot::from_events(&repeated));
}

#[tokio::test]
async fn empty_byte_pages_still_reach_the_global_no_progress_guard() {
    let (directory, repository, runtime, thread_id) = runtime_fixture().await;
    std::fs::write(directory.path().join("source.txt"), "中".repeat(100)).unwrap();
    let script = (0..25)
        .map(|id| {
            vec![
                Ok(ProviderEvent::ToolCall {
                    call: ToolCall {
                        id: format!("empty-{id}"),
                        name: "read_file".into(),
                        arguments: json!({"path": "source.txt", "offset": id * 3, "limit": 1}),
                        metadata: json!({}),
                    },
                }),
                Ok(ProviderEvent::Completed),
            ]
        })
        .collect();
    let provider = Arc::new(FakeProvider::script(script));
    let outcome = runtime
        .run_turn(
            provider.clone(),
            "fake".into(),
            RunTurnRequest {
                thread_id: thread_id.clone(),
                input: "inspect".into(),
                agent_mode: None,
            },
            CancellationToken::new(),
            Arc::new(RecordingPublisher::default()),
        )
        .await
        .unwrap();
    assert_eq!(outcome.state, TurnState::Failed, "{:?}", outcome.error);
    assert!(
        outcome.error.as_deref().unwrap().contains("无实质进展"),
        "{:?}",
        outcome.error
    );
    assert_eq!(
        provider.requests().len(),
        PROGRESS_CHECK_WINDOW * (MAX_NO_PROGRESS_WINDOWS + 1)
    );
    let events = repository.load(&thread_id).await.unwrap();
    let reads = events
        .iter()
        .filter_map(|event| match &event.kind {
            StoredEventKind::ToolResult { name, result, .. } if name == "read_file" => Some(result),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(reads.len(), 20);
    assert!(reads.iter().all(|result| result.success
        && result.output.is_empty()
        && result.metadata["bytesReturned"] == 0));
}

#[tokio::test]
async fn repeated_reads_in_one_batch_keep_all_bodies_and_continue() {
    let (directory, repository, runtime, thread_id) = runtime_fixture().await;
    let body = "first line\nimplementation detail\nlast line";
    std::fs::write(directory.path().join("source.txt"), body).unwrap();
    let mut batch = (0..5).map(|id| Ok(read_call(id, 1, 3))).collect::<Vec<_>>();
    batch.push(Ok(ProviderEvent::Completed));
    let provider = Arc::new(FakeProvider::script(vec![
        batch,
        vec![
            Ok(ProviderEvent::TextDelta {
                delta: "batch complete".into(),
            }),
            Ok(ProviderEvent::Completed),
        ],
    ]));
    let outcome = runtime
        .run_turn(
            provider.clone(),
            "fake".into(),
            RunTurnRequest {
                thread_id: thread_id.clone(),
                input: "inspect".into(),
                agent_mode: None,
            },
            CancellationToken::new(),
            Arc::new(RecordingPublisher::default()),
        )
        .await
        .unwrap();
    assert_eq!(outcome.state, TurnState::Completed, "{:?}", outcome.error);
    let events = repository.load(&thread_id).await.unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(&event.kind,
                StoredEventKind::ToolResult { name, result, .. }
                if name == "read_file" && result.success && result.output == body
            ))
            .count(),
        5
    );
    let requests = provider.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[1]
            .messages
            .iter()
            .filter(|message| matches!(message,
                ProviderMessage::ToolResult { output, .. } if output.contains(body)
            ))
            .count(),
        5
    );
}

#[test]
fn truncated_read_does_not_claim_the_omitted_middle_as_observed() {
    let mut result = versioned_read_result("source.txt", "revision", 1, 1);
    result.output = "x".repeat(MAX_TOOL_OUTPUT_BYTES * 2);
    result.metadata["offset"] = json!(0);
    result.metadata["bytesReturned"] = json!(result.output.len());
    let bounded = bound_tool_result(result);
    let mut tracker = ReadObservationTracker::default();
    tracker.observe(&bounded).unwrap();
    let mut middle = versioned_read_result("source.txt", "revision", 1, 1);
    middle.metadata["offset"] = json!(MAX_TOOL_OUTPUT_BYTES);
    middle.metadata["bytesReturned"] = json!(100);
    assert!(matches!(
        tracker.observe(&middle),
        Some(ReadObservationDecision::NewCoverage)
    ));
}

#[test]
fn read_coverage_handles_revisions_empty_files_and_context_reset() {
    let mut tracker = ReadObservationTracker::default();
    let mut result = versioned_read_result("source.txt", "first", 1, 1);
    result.output.clear();
    result.metadata["offset"] = json!(0);
    result.metadata["bytesReturned"] = json!(0);
    assert_eq!(
        tracker.observe(&result),
        Some(ReadObservationDecision::NewCoverage)
    );
    assert_eq!(
        tracker.observe(&result),
        Some(ReadObservationDecision::AlreadyCovered)
    );
    result.metadata["fileRevision"] = json!("second");
    assert_eq!(
        tracker.observe(&result),
        Some(ReadObservationDecision::NewCoverage)
    );
    tracker.reset_context();
    assert_eq!(
        tracker.observe(&result),
        Some(ReadObservationDecision::NewCoverage)
    );
    result.success = false;
    assert!(tracker.observe(&result).is_none());
    result.success = true;
    result.metadata["contentSuppressed"] = json!(true);
    assert!(tracker.observe(&result).is_none());
}

#[test]
fn invalid_read_ranges_do_not_become_successful_observations() {
    for metadata in [
        json!({"startLine":0,"endLine":1}),
        json!({"startLine":2,"endLine":1}),
        json!({"offset":0}),
        json!({"offset":u64::MAX,"bytesReturned":1}),
        json!({"offset":100,"bytesReturned":200,"outputTruncated":true}),
        json!({"offset":100,"bytesReturned":200,"outputTruncated":true,"retainedOutputRanges":[[u64::MAX,u64::MAX]]}),
    ] {
        let mut result = versioned_read_result("source.txt", "revision", 1, 1);
        result.metadata = metadata;
        result.metadata["path"] = json!("source.txt");
        result.metadata["fileRevision"] = json!("revision");
        assert!(ReadObservationTracker::default().observe(&result).is_none());
    }
}
