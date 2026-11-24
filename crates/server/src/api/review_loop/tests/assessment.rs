use super::*;

#[tokio::test]
async fn assessment_settings_default_and_changes_never_schedule_work() {
    let w = world().await;
    let settings = w.get("/api/v1/settings/comment-assessment").await;
    assert_eq!(settings["config"]["primary_method"], "claudius_host");
    assert_eq!(settings["config"]["ab_prs"], json!([]));
    assert_eq!(settings["quota"]["remaining"], 80);
    let (status,updated)=w.call("PUT","/api/v1/settings/comment-assessment",Some(json!({"primary_method":"prompt_v4","ab_prs":["acme/node-client#482"],"ab_until":null}))).await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(updated["config"]["primary_method"], "prompt_v4");
    assert_eq!(updated["quota"]["remaining"], 80);
    assert!(
        w.state
            .db
            .review_loop()
            .live_passes()
            .await
            .unwrap()
            .is_empty()
    );
    let (status, _) = w
        .call(
            "PUT",
            "/api/v1/settings/comment-assessment",
            Some(json!({"ab_prs":["../../../other"]})),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn assessment_comparison_is_payer_scoped_and_reads_do_not_fetch_github() {
    let w = world().await;
    w.server.reset().await;
    let (status, _) = w
        .call("GET", &format!("{PR}/comments/assessment-comparison"), None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(w.server.received_requests().await.unwrap().is_empty());
}
