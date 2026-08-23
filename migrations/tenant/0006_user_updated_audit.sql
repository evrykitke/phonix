-- ---------------------------------------------------------------------------
-- 0006: an audit event for editing an account.
--
-- `role_changed` records "this person was given that role" and nothing else.
-- An administrator editing the users screen can change a name, a status and a
-- set of roles in one save, and recording that as three events - or as a role
-- change that quietly also renamed somebody - loses the fact that it was one
-- decision by one person at one moment.
--
-- One event, recorded as `{from, to}` so the detail page renders it as a diff
-- rather than as a sentence; see `phonix_services::identity::audit_view`.
-- ---------------------------------------------------------------------------

ALTER TABLE identity_events DROP CONSTRAINT IF EXISTS identity_events_event_valid;

ALTER TABLE identity_events ADD CONSTRAINT identity_events_event_valid CHECK (
    event IN (
        'signup',
        'login',
        'logout',
        'password_change',
        'password_reset_requested',
        'password_reset_completed',
        'email_verification_sent',
        'email_verified',
        'mfa_enrolled',
        'mfa_challenge',
        'mfa_removed',
        'mfa_recovery_used',
        'mfa_recovery_generated',
        'account_locked',
        'account_unlocked',
        'role_changed',
        'session_revoked',
        'invitation_sent',
        'invitation_accepted',
        'password_policy_changed',
        'mfa_policy_changed',
        'user_permissions_changed',
        'role_permissions_changed',
        'user_updated'
    )
);
