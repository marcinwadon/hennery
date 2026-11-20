use super::*;

fn recorded_requests(bytes: &[u8]) -> Vec<Value> {
    // The child can be between writes, including inside a UTF-8 code point.
    // Only its terminating newline commits a JSONL record for observation.
    bytes
        .split_inclusive(|byte| *byte == b'\n')
        .filter(|line| line.ends_with(b"\n"))
        .map(|line| serde_json::from_slice(line).unwrap())
        .collect()
}

#[test]
fn steering_log_waits_for_the_writers_newline() {
    assert!(recorded_requests(b"{").is_empty());
    let first = json!({"method":"session/new"});
    let next = json!({"method":"_session/steering","text":"żółw 🐢"});
    let mut bytes = serde_json::to_vec(&first).unwrap();
    bytes.push(b'\n');
    let mut append = serde_json::to_vec(&next).unwrap();
    append.push(b'\n');
    for length in 0..append.len() {
        let mut snapshot = bytes.clone();
        snapshot.extend_from_slice(&append[..length]);
        assert_eq!(recorded_requests(&snapshot), vec![first.clone()]);
    }
    bytes.extend_from_slice(&append);
    assert_eq!(recorded_requests(&bytes), vec![first, next]);
}

#[test]
#[should_panic]
fn steering_log_still_rejects_a_malformed_complete_record() {
    recorded_requests(b"{}\n{broken}\n");
}

#[tokio::test]
async fn queued_steering_injects_once_and_late_ack_survives_disconnect() {
    exercise_steering(Scenario::Late).await;
}

#[tokio::test]
async fn default_steering_idle_race_restores_fifo() {
    exercise_steering(Scenario::Idle).await;
}

#[tokio::test]
async fn default_steering_queues_for_unsupported_adapter() {
    exercise_steering(Scenario::Unsupported).await;
}

#[tokio::test]
async fn default_steering_internal_errors_preserve_uncertainty() {
    exercise_steering(Scenario::InternalError).await;
}
#[tokio::test]
async fn default_steering_parameter_rejections_preserve_the_reason() {
    exercise_steering(Scenario::InvalidParams).await;
}
#[tokio::test]
async fn default_steering_falls_back_if_advertised_extension_is_missing() {
    exercise_steering(Scenario::MethodNotFound).await;
}

#[tokio::test]
async fn queued_steering_reports_rejection_if_advertised_extension_is_missing() {
    exercise_steering(Scenario::ManualMethodNotFound).await;
}

#[tokio::test]
async fn queued_steering_lost_adapter_connection_preserves_uncertainty() {
    exercise_steering(Scenario::Disconnect).await;
}

#[tokio::test]
async fn default_steering_admission_keeps_current_turn_and_permission() {
    exercise_steering(Scenario::Active).await;
}

#[tokio::test]
async fn queued_steering_dismissed_then_injected_preserves_content_until_session_deletion() {
    exercise_steering(Scenario::DismissedInjected).await;
}

#[tokio::test]
async fn queued_steering_dismissed_then_idle_never_enqueues_another_prompt() {
    exercise_steering(Scenario::DismissedIdle).await;
}

#[derive(Clone, Copy, PartialEq)]
enum Scenario {
    Active,
    Late,
    Idle,
    DismissedInjected,
    DismissedIdle,
    Unsupported,
    InternalError,
    InvalidParams,
    MethodNotFound,
    ManualMethodNotFound,
    Disconnect,
}

async fn exercise_steering(scenario: Scenario) {
    use Scenario::*;
    let outcome = match scenario {
        Active | Late | DismissedInjected => Some("injected"),
        Idle | DismissedIdle => Some("promptRequired"),
        Unsupported => None,
        InternalError => Some("internal_error"),
        InvalidParams => Some("invalid_params"),
        MethodNotFound | ManualMethodNotFound => Some("method_not_found"),
        Disconnect => Some("disconnect"),
    };
    hennery_testkit::first_exec(env!("CARGO_BIN_EXE_hennery-fake-acp"));
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("requests");
    let gate = dir.path().join("release-steering");
    let collector = Collector::start(&dir.path().join("hennery.db"), None).await;
    let script = FakeScript {
        steering_outcome: outcome.map(String::from),
        steering_gate: Some(gate.display().to_string()),
        session_log: Some(log.display().to_string()),
        log_prompts: true,
        ..asking(vec![hennery_testkit::FakeAsk::Permission])
    };
    let host = start_host(collector.addr, &dir.path().join("host"), &script);
    let c = client(&collector);
    wait_host_connected(&c, &collector).await;
    let session = start_session(&c, &collector).await;
    prompt(&c, &collector, &session).await;
    let pending = open_question(&c, &collector, &session).await;
    let base = collector.url(&format!("/api/sessions/{session}"));
    let queue = || collector.state.store.operator_prompt_queue(&session).unwrap();
    let log = || recorded_requests(&std::fs::read(&log).unwrap());
    let enqueue = |content: &Value| {
        let checked = hennery_sessions::content::check(content.as_array().unwrap().clone()).unwrap();
        collector
            .state
            .store
            .enqueue_operator_prompt(&session, &checked)
            .unwrap()
    };
    let first = enqueue(&text("first"));
    let mut selected =
        json!([{"type":"text","text":"selected"},{"type":"image","mimeType":"image/png","data":"iVBORw0KGgo="}]);
    if scenario == Active {
        selected
            .as_array_mut()
            .unwrap()
            .push(json!({"type":"file","name":"notes.txt","mimeType":"text/plain","data":"aGVsbG8="}));
    }
    let submit = || {
        post_json(
            &c,
            format!("{base}/prompt"),
            json!({"content":selected,"request_key":"follow-up"}),
        )
    };
    let automatic = matches!(
        scenario,
        Active | Idle | InternalError | Unsupported | MethodNotFound | InvalidParams
    );
    let second = if automatic {
        let (first_submit, retry) = tokio::join!(submit(), submit());
        assert_eq!(first_submit.0, 202);
        assert_eq!(retry, first_submit);
        first_submit.1
    } else {
        json!({"queue_id":enqueue(&selected)})
    };
    let url = format!("{base}/queue/{}", second["queue_id"].as_str().unwrap());
    let steer = || c.post(format!("{url}/steer")).send();
    if scenario == Unsupported {
        assert_eq!(steer().await.unwrap().status(), 409);
        assert_eq!(queue()[1].state, "queued");
        assert_eq!(log().iter().filter(|r| r["method"] == "_session/steering").count(), 0);
    } else {
        if automatic {
            assert_eq!(
                queue()[1].state,
                "delivering",
                "ordinary send should steer the live turn"
            );
        } else {
            let (a, b) = tokio::join!(steer(), steer());
            let mut statuses = [a.unwrap().status().as_u16(), b.unwrap().status().as_u16()];
            statuses.sort();
            assert_eq!(statuses, [200, 409]);
        }
        wait_for("adapter recorded steering", || async {
            log().into_iter().find(|r| r["method"] == "_session/steering")
        })
        .await;
        assert_eq!(queue()[1].state, "delivering");
        assert_eq!(steer().await.unwrap().status(), 409);
        assert_eq!(c.delete(&url).send().await.unwrap().status(), 409);
        assert_eq!(
            c.patch(&url)
                .json(&json!({"text":"oops"}))
                .send()
                .await
                .unwrap()
                .status(),
            409
        );
        let steered = log().into_iter().find(|r| r["method"] == "_session/steering").unwrap();
        if scenario == Active {
            let prompt = &steered["params"]["prompt"];
            assert_eq!(&prompt.as_array().unwrap()[..2], &selected.as_array().unwrap()[..2]);
            assert!(prompt[2]["text"].as_str().unwrap().contains("notes.txt"));
        } else {
            assert_eq!(steered["params"]["prompt"], selected);
        }
        assert_eq!(steered["params"]["_meta"]["steering"]["idleBehavior"], "promptRequired");
        assert_eq!(open_question(&c, &collector, &session).await, pending);

        if scenario == Active {
            std::fs::write(&gate, "").unwrap();
            wait_for("injected into open turn", || async {
                (queue().len() == 1).then_some(())
            })
            .await;
            assert_eq!(queue()[0].id, first);
            assert_eq!(open_question(&c, &collector, &session).await, pending);
            let facts = events(&c, &collector, &session).await;
            assert!(turn_ends(&facts).is_empty());
            let admitted: Vec<_> = facts.iter().filter(|e| e.kind == "user_steering").collect();
            assert_eq!(admitted.len(), 1);
            assert_eq!(admitted[0].body["content"][2]["name"], "notes.txt");
            assert_eq!(log().iter().filter(|r| r["method"] == "session/prompt").count(), 1);
            assert_eq!(facts.iter().filter(|e| e.kind == "user_turn").count(), 1);
        } else if matches!(scenario, DismissedInjected | DismissedIdle) {
            collector.state.hub.disconnect("host-1");
            wait_for("uncertain recovery", || async {
                (queue()[1].state == "uncertain").then_some(())
            })
            .await;
            let original_content = serde_json::to_value(&queue()[1].content).unwrap();
            let sha = original_content[1]["sha256"].as_str().unwrap();
            let image_url = collector.url(&format!("/api/attachments/{sha}"));
            assert_eq!(c.delete(&url).send().await.unwrap().status(), 200);
            assert_eq!(queue().len(), 1);
            collector
                .state
                .store
                .sweep_attachments(std::time::SystemTime::now(), &Default::default())
                .unwrap();
            assert_eq!(
                c.get(&image_url).send().await.unwrap().status(),
                200,
                "dispatched image survives dismissal"
            );
            std::fs::write(&gate, "").unwrap();
            let ack = wait_for("outboxed result after dismissal", || async {
                events(&c, &collector, &session)
                    .await
                    .into_iter()
                    .find(|e| e.kind == "steering_result")
            })
            .await;
            let facts = events(&c, &collector, &session).await;
            let attempt = facts
                .iter()
                .find(|e| e.kind == "steering_attempt")
                .expect("dispatched evidence");
            assert_eq!(attempt.body["request_id"], ack.body["request_id"]);
            assert_eq!(attempt.body["content"], original_content);
            assert_eq!(
                attempt.body["turn_id"],
                facts.iter().find(|e| e.kind == "user_turn").unwrap().body["turn_id"]
            );
            // A replay on a new host sequence must also be idempotent.
            let seq = facts.iter().filter_map(|e| e.host_seq).max().unwrap() + 1;
            let body = serde_json::from_value(ack.body.clone()).unwrap();
            collector.state.store.ingest(&session, seq, &body).unwrap();
            let facts = events(&c, &collector, &session).await;
            let admitted: Vec<_> = facts.iter().filter(|e| e.kind == "user_steering").collect();
            if scenario == DismissedInjected {
                assert_eq!(admitted.len(), 1);
                assert_eq!(admitted[0].body["turn_id"], attempt.body["turn_id"]);
                assert_eq!(admitted[0].body["request_id"], ack.body["request_id"]);
                assert_eq!(admitted[0].body["content"], original_content);
            } else {
                assert!(admitted.is_empty());
            }
            assert_eq!(queue().len(), 1);
            assert_eq!(queue()[0].id, first);
            assert_eq!(log().iter().filter(|r| r["method"] == "session/prompt").count(), 1);
            // Session deletion removes the attempt, the admission and their shared bytes.
            let judged = hennery_sessions::store::Unattached {
                lifecycle: "active".into(),
                presumed_parked: false,
            };
            assert!(matches!(
                collector.state.store.delete_session(&session, Some(&judged)).unwrap(),
                hennery_sessions::store::Deletion::Done { .. }
            ));
            assert_eq!(c.get(&image_url).send().await.unwrap().status(), 404);
            assert!(
                collector
                    .state
                    .store
                    .events(&session, 0, 100)
                    .unwrap()
                    .iter()
                    .all(|e| !matches!(e.kind.as_str(), "steering_attempt" | "user_steering"))
            );
        } else if scenario == Late {
            collector.state.hub.disconnect("host-1");
            wait_for("uncertain delivery", || async {
                (queue()[1].state == "uncertain").then_some(())
            })
            .await;
            assert_eq!(steer().await.unwrap().status(), 409);
            wait_for("reconciled host", || async {
                collector.state.hub.is_ready("host-1").then_some(())
            })
            .await;
            assert_eq!(
                answer(&c, &collector, &session, &pending, json!({"option_id":"allow"}))
                    .await
                    .0,
                202
            );
            wait_for("following ordinary turn starts", || async {
                (events(&c, &collector, &session)
                    .await
                    .iter()
                    .filter(|e| e.kind == "user_turn")
                    .count()
                    == 2)
                    .then_some(())
            })
            .await;
            let page_url = collector.url(&format!("/api/view/sessions/{session}"));
            let before: Value = c.get(&page_url).send().await.unwrap().json().await.unwrap();
            let anchor = format!("{}:{}", before["epoch"].as_str().unwrap(), before["revision"]);
            let stream = c
                .get(collector.url(&format!("/api/stream/view/sessions/{session}")))
                .query(&[("after", &anchor)])
                .send()
                .await
                .unwrap();
            std::fs::write(&gate, "").unwrap();
            wait_for("late admission acknowledgement", || async {
                queue().is_empty().then_some(())
            })
            .await;
            let facts = events(&c, &collector, &session).await;
            let later = facts.iter().filter(|e| e.kind == "user_turn").nth(1).unwrap();
            let older: Value = c
                .get(&page_url)
                .query(&[("before_turn", later.body["turn_id"].as_str().unwrap()), ("limit", "1")])
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            assert!(
                older["items"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|i| i["content"][0]["text"] == "selected"),
                "confirmed steering belongs on original turn's page"
            );
            let newest: Value = c
                .get(&page_url)
                .query(&[("limit", "1")])
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            assert!(
                newest["items"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|i| i["content"][0]["text"] != "selected")
            );
            assert!(
                tokio::time::timeout(Duration::from_secs(4), stream.text())
                    .await
                    .unwrap()
                    .unwrap()
                    .contains("resync_required")
            );
            let replay = c
                .get(collector.url(&format!("/api/stream/view/sessions/{session}")))
                .query(&[("after", &anchor)])
                .send()
                .await
                .unwrap();
            assert!(
                tokio::time::timeout(Duration::from_secs(4), replay.text())
                    .await
                    .unwrap()
                    .unwrap()
                    .contains("resync_required")
            );
            let original = facts.iter().find(|e| e.kind == "user_turn").unwrap();
            let steered = facts
                .iter()
                .find(|e| e.kind == "user_steering")
                .expect("confirmed steering retained in transcript");
            assert_eq!(facts.iter().filter(|e| e.kind == "user_steering").count(), 1);
            let after_b: Vec<EventDto> = c
                .get(format!("{base}/events"))
                .query(&[("after", later.event_id)])
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            let incremental: Vec<_> = after_b.iter().filter(|e| e.kind == "user_steering").collect();
            assert_eq!(
                incremental,
                vec![steered],
                "raw pagination replays the canonical admission"
            );
            let replayed = raw_replay_through(
                &c,
                &collector,
                &session,
                later.event_id,
                after_b.last().unwrap().event_id,
            )
            .await;
            assert_eq!(
                replayed
                    .iter()
                    .filter(|e| e.kind == "user_steering")
                    .collect::<Vec<_>>(),
                vec![steered]
            );
            assert_eq!(steered.body["turn_id"], original.body["turn_id"]);
            assert_eq!(steered.body["content"][0], selected[0]);
            collector
                .state
                .store
                .sweep_attachments(std::time::SystemTime::now(), &Default::default())
                .unwrap();
            let sha = steered.body["content"][1]["sha256"].as_str().unwrap();
            assert_eq!(
                c.get(collector.url(&format!("/api/attachments/{sha}")))
                    .send()
                    .await
                    .unwrap()
                    .status(),
                200
            );
        } else if matches!(scenario, MethodNotFound | ManualMethodNotFound) {
            std::fs::write(&gate, "").unwrap();
            let item = wait_for("method not found reply", || async {
                queue()
                    .into_iter()
                    .find(|item| item.id == second["queue_id"] && item.state != "delivering")
            })
            .await;
            if automatic {
                assert_eq!(item.state, "queued");
                assert_eq!(submit().await.1, second);
            } else {
                assert_eq!(item.state, "failed");
            }
        } else if matches!(outcome, Some("internal_error" | "invalid_params" | "disconnect")) {
            std::fs::write(&gate, "").unwrap();
            let wanted = if outcome == Some("invalid_params") {
                "failed"
            } else {
                "uncertain"
            };
            let item = wait_for("steering disposition", || async {
                queue()
                    .into_iter()
                    .find(|i| i.id == second["queue_id"] && i.state == wanted)
            })
            .await;
            if scenario == InternalError {
                assert_eq!(submit().await.1, second);
                assert_eq!(queue()[1].state, "uncertain");
            }
            if wanted == "failed" {
                assert!(item.reason.unwrap().contains("Rejected input"));
            }
        } else {
            assert_eq!(
                answer(&c, &collector, &session, &pending, json!({"option_id":"allow"}))
                    .await
                    .0,
                202
            );
            wait_for("original turn ended", || async {
                (!turn_ends(&events(&c, &collector, &session).await).is_empty()).then_some(())
            })
            .await;
            std::fs::write(&gate, "").unwrap();
            let question = open_question(&c, &collector, &session).await;
            wait_for("steering restored to the ordinary queue", || async {
                queue()
                    .into_iter()
                    .find(|item| item.id == second["queue_id"] && item.state == "queued")
            })
            .await;
            assert_eq!(submit().await.1, second);
            assert_eq!(queue()[0].state, "queued");
            assert_eq!(
                answer(&c, &collector, &session, &question, json!({"option_id":"allow"}))
                    .await
                    .0,
                202
            );
            wait_for("fallback prompt", || async {
                (log().iter().filter(|r| r["method"] == "session/prompt").count() == 3).then_some(())
            })
            .await;
            let prompts: Vec<_> = log().into_iter().filter(|r| r["method"] == "session/prompt").collect();
            assert_eq!(prompts[1]["params"]["prompt"], text("first"));
            assert_eq!(prompts[2]["params"]["prompt"], selected);
        }
        assert_eq!(log().iter().filter(|r| r["method"] == "_session/steering").count(), 1);
    }
    host.abort();
    let _ = host.await;
    collector.stop().await;
}

#[tokio::test]
async fn queued_steering_refuses_a_reserved_turn_before_actor_admission() {
    hennery_testkit::first_exec(env!("CARGO_BIN_EXE_hennery-fake-acp"));
    let dir = tempfile::tempdir().unwrap();
    let collector = Collector::start(&dir.path().join("db"), None).await;
    let host = start_host(
        collector.addr,
        &dir.path().join("host"),
        &FakeScript {
            steering_outcome: Some("injected".into()),
            ..Default::default()
        },
    );
    let c = client(&collector);
    wait_host_connected(&c, &collector).await;
    let session = start_session(&c, &collector).await;
    let store = &collector.state.store;
    let original = hennery_sessions::content::check(vec![json!({"type":"text","text":"reserved"})]).unwrap();
    assert!(store.open_prompt(&session, "reserved", &original).unwrap());
    let id = store.enqueue_operator_prompt(&session, &original).unwrap();
    let response = c
        .post(collector.url(&format!("/api/sessions/{session}/queue/{id}/steer")))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 409);
    assert_eq!(store.operator_prompt_queue(&session).unwrap()[0].state, "queued");
    host.abort();
    let _ = host.await;
    collector.stop().await;
}

async fn raw_replay_through(
    c: &reqwest::Client,
    collector: &Collector,
    session: &str,
    after: i64,
    through: i64,
) -> Vec<EventDto> {
    use futures::StreamExt;
    let mut stream = c
        .get(collector.url(&format!("/api/stream/sessions/{session}")))
        .header("last-event-id", after.to_string())
        .send()
        .await
        .unwrap()
        .bytes_stream();
    let mut received = Vec::new();
    let mut buffer = Vec::new();
    let until = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let bytes = tokio::time::timeout_at(until, stream.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        buffer.extend_from_slice(&bytes);
        while let Some(end) = buffer.windows(2).position(|pair| pair == b"\n\n") {
            let frame = String::from_utf8(buffer.drain(..end + 2).collect()).unwrap();
            if frame.lines().any(|line| line == "event: event") {
                let data = frame.lines().find_map(|line| line.strip_prefix("data: ")).unwrap();
                let event: EventDto = serde_json::from_str(data).unwrap();
                let done = event.event_id >= through;
                received.push(event);
                if done {
                    return received;
                }
            }
        }
    }
}
