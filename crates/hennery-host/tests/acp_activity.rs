use hennery_host::acp_activity::Activity;
use serde_json::json;

#[test]
fn foreground_idle_does_not_hide_background_or_clear_unknown_snapshots() {
    let mut activity = Activity::default();
    activity.observe(&json!({"update":{"sessionUpdate":"state_update","state":"idle","background":[{"type":"terminal","terminalId":"t"}]}}));
    assert!(!activity.foreground_busy());
    assert!(activity.blocks_parking());
    for background in [json!(null), json!("malformed")] {
        activity.observe(&json!({"update":{"sessionUpdate":"state_update","state":"idle","background":background}}));
        assert!(activity.blocks_parking());
    }
    activity
        .observe(&json!({"update":{"sessionUpdate":"terminal_update","terminalId":"t","exitStatus":{"exitCode":0}}}));
    assert!(activity.blocks_parking());
    activity.observe(&json!({"update":{"sessionUpdate":"state_update","state":"idle","background":[]}}));
    assert!(!activity.blocks_parking());
}

#[test]
fn unknown_foreground_and_unknown_background_work_are_busy() {
    let mut activity = Activity::default();
    for state in ["running", "requires_action", "future_state", "unknown"] {
        activity.observe(&json!({"update":{"sessionUpdate":"state_update","state":state}}));
        assert!(activity.foreground_busy());
        assert!(activity.blocks_parking());
    }
    activity
        .observe(&json!({"update":{"sessionUpdate":"state_update","state":"idle","background":[{"type":"future"}]}}));
    assert!(activity.blocks_parking());
    activity.observe(&json!({"update":{"sessionUpdate":"state_update","state":"idle","background":[]}}));
    assert!(
        !activity.blocks_parking(),
        "future work has no known final-status contract"
    );
}

#[test]
fn snapshot_disappearance_does_not_complete_known_work_without_final_evidence() {
    for (kind, field, final_update) in [
        (
            "terminal",
            "terminalId",
            json!({"sessionUpdate":"terminal_update","terminalId":"t","exitStatus":{}}),
        ),
        (
            "tool_call",
            "toolCallId",
            json!({"sessionUpdate":"tool_call_update","toolCallId":"t","status":"cancelled"}),
        ),
    ] {
        let mut activity = Activity::default();
        let mut item = json!({"type":kind});
        item[field] = json!("t");
        activity.observe(&json!({"update":{"sessionUpdate":"state_update","state":"idle","background":[item]}}));
        activity.observe(&json!({"update":{"sessionUpdate":"state_update","state":"idle","background":[]}}));
        assert!(activity.blocks_parking(), "missing final evidence for {kind}");
        activity.observe(&json!({"update":final_update}));
        assert!(
            !activity.blocks_parking(),
            "late final evidence releases removed {kind}"
        );
    }
}

#[test]
fn malformed_known_identities_cannot_clear_previous_membership() {
    let mut activity = Activity::default();
    activity.observe(&json!({"update":{"sessionUpdate":"state_update","state":"idle","background":[{"type":"terminal","terminalId":"t"}]}}));
    activity.observe(&json!({"update":{"sessionUpdate":"terminal_update","terminalId":"t","exitStatus":{}}}));
    for item in [
        json!({"type":"terminal"}),
        json!({"type":"terminal","terminalId":17}),
        json!({"type":"tool_call"}),
        json!({"type":"tool_call","toolCallId":""}),
    ] {
        activity.observe(&json!({"update":{"sessionUpdate":"state_update","state":"idle","background":[item]}}));
        assert!(activity.blocks_parking());
    }
    activity.observe(&json!({"update":{"sessionUpdate":"state_update","state":"idle","background":[]}}));
    assert!(!activity.blocks_parking());
}

#[test]
fn final_before_membership_is_remembered_but_listing_still_blocks_parking() {
    let mut activity = Activity::default();
    activity
        .observe(&json!({"update":{"sessionUpdate":"terminal_update","terminalId":"t","exitStatus":{"exitCode":0}}}));
    activity.observe(&json!({"update":{"sessionUpdate":"state_update","state":"idle","background":[{"type":"terminal","terminalId":"t"}]}}));
    assert!(activity.blocks_parking());
    activity.observe(&json!({"update":{"sessionUpdate":"state_update","state":"idle","background":[]}}));
    assert!(!activity.blocks_parking());
}

#[test]
fn evicted_early_final_evidence_leaves_old_work_conservatively_unresolved() {
    let mut activity = Activity::default();
    for index in 0..300 {
        activity.observe(&json!({"update":{"sessionUpdate":"terminal_update","terminalId":format!("terminal-{index}"),"exitStatus":{}}}));
    }
    activity.observe(&json!({"update":{"sessionUpdate":"state_update","state":"idle","background":[{"type":"terminal","terminalId":"terminal-0"}]}}));
    activity.observe(&json!({"update":{"sessionUpdate":"state_update","state":"idle","background":[]}}));
    assert!(
        activity.blocks_parking(),
        "cache eviction must not invent completion evidence"
    );
    activity.observe(&json!({"update":{"sessionUpdate":"terminal_update","terminalId":"terminal-0","exitStatus":{}}}));
    assert!(!activity.blocks_parking());
}
