-- The sign-in allowlist comes from SR_ALLOWED_LOGINS, applied at every server start (ADR-007 addendum). Drops the
-- logins 0001 seeded while nobody has signed in through them; a bound entry stays for that start to keep or remove.
DELETE FROM tenant_allowed_logins
WHERE tenant_id = 'default' AND login IN ('lklimek', 'Claudius-Maginificent') AND github_id IS NULL;
