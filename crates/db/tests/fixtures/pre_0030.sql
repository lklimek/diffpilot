-- Synthetic credentials only. Every application table through 0029 has rows.
INSERT INTO users (id, github_id, login, name, language, created_at, updated_at)
VALUES ('u1', 1, 'alice', 'Owner ż', 'pl', '2026-11-23T00:00:00Z', '2026-11-23T00:00:00Z');
INSERT INTO tenant_allowed_logins VALUES ('default', 'alice', 1, '2026-11-23T00:00:00Z');
INSERT INTO memberships (tenant_id, user_id, created_at) VALUES ('default', 'u1', '2026-11-23T00:00:00Z');
INSERT INTO oauth_logins VALUES ('fake-oauth-state', 'default', x'000102ff', '2026-11-23T00:00:00Z', '2099-01-01T00:00:00Z');
INSERT INTO sessions VALUES (x'87059729cf67c4023780bd43670ba829d0d5ba1be7c94264784edfe3899c6642', 'default', 'u1', 'fake-csrf', '2026-11-23T00:00:00Z', '2026-11-23T00:00:00Z', '2099-01-01T00:00:00Z');
-- Fake GitHub tokens: AES-256-GCM, test_config key bytes 00..1f, nonce || ciphertext || tag.
INSERT INTO user_tokens VALUES ('u1', 'default', x'000102030405060708090a0b2163bd7ee882ab6fe534f5a6d08a1b08f0a5aa409f103a1217e0d5e6358b2be24e5c16bc603dbebb', '2099-01-01T00:00:00Z', x'0c0d0e0f1011121314151617fe9f02bd521195275f5f2af8e3ee4095307085c3821250cff570e912afa52c251a4cd7e9635187e48a', '2099-01-01T00:00:00Z', 7, '2026-11-23T00:00:00Z');
INSERT INTO llm_selection VALUES (1, 'gpt-6.2-sol', 'low', 'alice', '2026-11-23T00:00:00Z', 'high', 'high');
INSERT INTO controlled_owners VALUES ('default', 'acme');
INSERT INTO controlled_owners_changed VALUES ('default', 'alice', '2026-11-23T00:00:00Z');
INSERT INTO repos (id, tenant_id, owner, name, github_id, installation_id, installation_checked_at)
VALUES (1, 'default', 'acme', 'project', 11, 22, '2026-11-23T00:00:00Z');
INSERT INTO pull_requests (id, tenant_id, repo_id, number, title, author, html_url, base_sha, head_sha, fetched_at,
                          agent_thread_id, analytic_generation, session_state)
VALUES (1, 'default', 1, 42, 'PR ż', 'alice', 'https://github.example/acme/project/pull/42', 'base', 'head',
        '2026-11-23T00:00:00Z', 'old-analytic-thread', 3, 'active');
INSERT INTO analyses (id, tenant_id, pull_request_id, base_sha, head_sha, language, state, created_by, created_at)
VALUES ('a1', 'default', 1, 'base', 'head', 'pl', 'running', 'u1', '2026-11-23T00:00:00Z');
INSERT INTO documents (analysis_id, tenant_id, kind, key, body, created_at)
VALUES ('a1', 'default', 'flow', 'f1', '{"old":"analysis"}', '2026-11-23T00:00:00Z');
INSERT INTO review_states (user_id, analysis_id, tenant_id, ticked_steps, updated_at)
VALUES ('u1', 'a1', 'default', '["f1.1"]', '2026-11-23T00:00:00Z');
INSERT INTO feedback (id, tenant_id, analysis_id, user_id, target_kind, target_key, value, created_at, updated_at)
VALUES ('fb1', 'default', 'a1', 'u1', 'step', 'f1.1', 'wrong', '2026-11-23T00:00:00Z', '2026-11-23T00:00:00Z');
INSERT INTO comment_requests (user_id, idempotency_key, tenant_id, analysis_id, request_hash, state, created_at)
VALUES ('u1', 'request1', 'default', 'a1', x'0100ff', 'done', '2026-11-23T00:00:00Z');
INSERT INTO flow_work (tenant_id, analysis_id, flow_id, requested_at, auto_retries)
VALUES ('default', 'a1', 'f1', '2026-11-23T00:00:00Z', 2);
INSERT INTO working_sessions VALUES ('default', 1, 'u1', 'old-ask-thread', 3);
INSERT INTO llm_credentials (id, tenant_id, kind) VALUES ('old-codex', 'default', 'codex_chatgpt');
INSERT INTO dashboard_settings (user_id, tenant_id, linked_logins, enumerate_owners, repositories, lookback_days,
    ready_label, skip_authors, include_todos, memcan_extra_projects, schedule_enabled, schedule_time, time_zone,
    known_bots, slack_names, updated_at)
VALUES ('u1', 'default', '["alice","alice-bot"]', '["acme"]',
    '[{"repo":{"owner":"acme","name":"project"},"alias":"PROJECT","watch_issues":true,"ownership_policy":null,"branch_patterns":["release/*"],"ignored_checks":["optional"]}]',
    28, 'review-me', '["skip-bot"]', 0, '["extra-project"]', 1, '08:15', 'Europe/Warsaw', '["alice-bot"]',
    '[{"login":"alice","slack":"@owner"}]', '2026-11-23T00:00:00Z');
INSERT INTO dashboard_hidden_mentions VALUES ('default', 'u1', 'mention1', '2026-11-23T00:00:00Z');
INSERT INTO dashboard_snoozes (tenant_id, user_id, item_key, snoozed_at, until)
VALUES ('default', 'u1', 'acme~project~pr~42', '2026-11-23T00:00:00Z', '2099-01-01T00:00:00Z');
INSERT INTO dashboard_snoozes (tenant_id, user_id, item_key, snoozed_at, until, ended_at, end_reason)
VALUES ('default', 'u1', 'acme~project~pr~41', '2026-11-22T00:00:00Z', '2026-11-23T00:00:00Z', '2026-11-23T00:00:00Z', 'date');
-- `automatic` is admitted by 0029; keep all three job targets populated.
INSERT INTO dashboard_refreshes (id, tenant_id, user_id, trigger, state, started_at, snapshot, ranking_state, ranking_attempt, sent)
VALUES ('r1', 'default', 'u1', 'automatic', 'ranking', '2026-11-23T00:00:00Z', '{"items":[]}', 'running', 2, 3);
INSERT INTO dashboard_judgements (tenant_id, user_id, item_key, input_fingerprint, input_version, prompt_version,
    body, refresh_id, job_id, model, judged_at, last_seen_at)
VALUES ('default', 'u1', 'acme~project~pr~42', 'fingerprint', 2, 4, '{}', 'r1', 'j-dashboard', 'sol',
        '2026-11-23T00:00:00Z', '2026-11-23T00:00:00Z');
INSERT INTO access_tokens (id, tenant_id, user_id, name, token_hash, scopes, created_at, expires_at)
VALUES ('fakePAT1', 'default', 'u1', 'Agent', zeroblob(32), '["read","review_loop_execute"]', '2026-11-23T00:00:00Z', '2099-01-01T00:00:00Z');
INSERT INTO agent_subscriptions (token_id, tenant_id, capabilities, last_seen_at, push_kind, push_url, push_topic, push_secret)
VALUES ('fakePAT1', 'default', '["fix"]', '2026-11-23T00:00:00Z', 'webhook', 'https://push.example/hook', 'topic', x'001122ff');
INSERT INTO notification_channels (id, tenant_id, user_id, kind, url, topic, secret, events, enabled)
VALUES ('channel1', 'default', 'u1', 'ntfy', 'https://push.example', 'owner-topic', x'ff112200', '["questions_ready"]', 1);
INSERT INTO pr_watches (user_id, pull_request_id, tenant_id, since, state)
VALUES ('u1', 1, 'default', '2026-11-23T00:00:00Z', 'active');
INSERT INTO comment_prs (pull_request_id, tenant_id, read_at, github_snapshot, round_count, next_item_no, purpose, created_at)
VALUES (1, 'default', '2026-11-23T00:00:00Z', '{}', 2, 3, 'Old purpose', '2026-11-23T00:00:00Z');
INSERT INTO comment_items (id, tenant_id, pull_request_id, item_no, kind, stable_key, author, author_kind, title,
    body, body_hash, first_seen_head, github, state, created_at, updated_at)
VALUES ('item1', 'default', 1, 1, 'thread', 'thread1', 'reviewer', 'human', 'Finding', 'Old finding', 'hash', 'head',
        '{}', 'decided', '2026-11-23T00:00:00Z', '2026-11-23T00:00:00Z');
INSERT INTO questions (id, tenant_id, analysis_id, pull_request_id, user_id, text, state, kind, item_id, job_id, created_at)
VALUES ('q1', 'default', NULL, 1, 'u1', 'Why?', 'pending', 'comment_item', 'item1', 'j-comments', '2026-11-23T00:00:00Z');
INSERT INTO questions (id, tenant_id, analysis_id, pull_request_id, step_id, user_id, text, state, kind, parent_id, created_at)
VALUES ('q2', 'default', 'a1', 1, 'f1.1', 'u1', 'Follow up?', 'pending', 'step', 'q1', '2026-11-23T00:00:00Z');
INSERT INTO comment_passes (id, tenant_id, pull_request_id, user_id, cause, head, items, state, job_id, question_id, created_at)
VALUES ('pass1', 'default', 1, 'u1', 'question', 'head', 1, 'running', 'j-comments', 'q1', '2026-11-23T00:00:00Z');
INSERT INTO jobs (id, tenant_id, analysis_id, kind, priority, state, spec, credential_id, created_at, stage)
VALUES ('j-analysis', 'default', 'a1', 'analyse_flow', 1, 'running', '{}', 'old-codex', '2026-11-23T00:00:00Z', 'flow:f1');
INSERT INTO jobs (id, tenant_id, refresh_id, kind, priority, state, spec, credential_id, created_at, stage)
VALUES ('j-dashboard', 'default', 'r1', 'judge_work', 1, 'queued', '{}', 'old-codex', '2026-11-23T00:00:00Z', 'judge:batch1');
INSERT INTO jobs (id, tenant_id, pass_id, kind, priority, state, spec, credential_id, created_at, stage)
VALUES ('j-comments', 'default', 'pass1', 'verify_comments', 0, 'queued', '{}', 'old-codex', '2026-11-23T00:00:00Z', 'verify:pass1');
INSERT INTO installation_tokens VALUES ('j-analysis', 'default', 1, x'010203ff', '2099-01-01T00:00:00Z');
INSERT INTO job_events VALUES ('j-analysis', 'default', 1, 1, '2026-11-23T00:00:00Z', '{}');
INSERT INTO comment_assessments (tenant_id, item_id, job_id, head, fingerprint, body, prompt_version, assessed_at)
VALUES ('default', 'item1', 'j-comments', 'head', 'fp', '{"validity":"valid"}', 4, '2026-11-23T00:00:00Z');
INSERT INTO comment_decisions (id, tenant_id, item_id, user_id, kind, resolve_ticked, via, head, body_hash, decided_at)
VALUES ('d1', 'default', 'item1', 'u1', 'fix_here', 0, 'table', 'head', 'hash', '2026-11-23T00:00:00Z');
INSERT INTO comment_effects (id, tenant_id, decision_id, seq, kind, executor, params, state, claim_token_id, lease_until, updated_at)
VALUES ('e1', 'default', 'd1', 1, 'fix', 'agent', '{}', 'claimed', 'fakePAT1', '2099-01-01T00:00:00Z', '2026-11-23T00:00:00Z');
INSERT INTO comment_effect_reports (tenant_id, effect_id, token_id, outcome, idempotency_key, at)
VALUES ('default', 'e1', 'fakePAT1', 'released', 'report1', '2026-11-23T00:00:00Z');
INSERT INTO comment_carry_events VALUES ('carry1', 'default', 1, 'item1', 'd1', 'asked_again', 'code_changed', '{}', '2026-11-23T00:00:00Z');
INSERT INTO comment_decision_undos VALUES ('d1', 'default', 'u1', '2026-11-23T00:00:00Z');
INSERT INTO comment_item_skips VALUES ('default', 'u1', 'item1', 1, '2026-11-23T00:00:00Z');
INSERT INTO user_events (id, tenant_id, user_id, kind, pull_request_id, count, text, at, round_through)
VALUES ('event1', 'default', 'u1', 'round_executed', 1, 1, 'Round complete', '2026-11-23T00:00:00Z', '2026-11-23T00:00:00Z');
INSERT INTO notification_deliveries (tenant_id, user_event_id, channel_id, state, next_attempt_at, created_at, updated_at)
VALUES ('default', 'event1', 'channel1', 'pending', '2026-11-23T00:00:00Z', '2026-11-23T00:00:00Z', '2026-11-23T00:00:00Z');
INSERT INTO notification_deliveries (tenant_id, decision_id, token_id, state, next_attempt_at, created_at, updated_at)
VALUES ('default', 'd1', 'fakePAT1', 'pending', '2026-11-23T00:00:00Z', '2026-11-23T00:00:00Z', '2026-11-23T00:00:00Z');
INSERT INTO notification_sends (tenant_id, target, at) VALUES ('default', 'channel:u1/ntfy', '2026-11-23T00:00:00Z');
