-- Slack names per user (docs/contracts/db-schema.md, plan docs/v1.1-dashboard-plan.md §F.6 G18): GitHub logins with
-- the text "Copy for Slack" writes instead of `@login`, a JSON array of `SlackName` objects. Additive: existing rows
-- gain an empty table.
ALTER TABLE dashboard_settings ADD COLUMN slack_names TEXT NOT NULL DEFAULT '[]'
    CHECK (json_valid(slack_names) AND json_type(slack_names) = 'array');
