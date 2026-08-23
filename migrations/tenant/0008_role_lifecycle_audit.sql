-- ---------------------------------------------------------------------------
-- Three events for the three different things "a role changed" can mean.
--
-- `role_changed` is already taken, and it means "this person was given that
-- role" - it is written against an account. `role_permissions_changed` means
-- "this role now grants something different". Neither can carry "this role
-- exists now", "it is called something else now" or "it is gone, and everybody
-- who held it lost what it granted", which is what the roles screen can now do.
--
-- Three rather than one because the audit trail is read by asking one question
-- at a time, and a single `role_lifecycle` event would have to be opened to
-- find out which question it answers.
--
-- CHECK constraints cannot be extended in place, so the whole list is restated.
-- That is the same shape as 0005, 0006 and 0007, and it is on purpose: the
-- constraint as written here is the complete set of events this release can
-- store, which is worth being able to read in one place.
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
        'user_updated',
        'mail_settings_changed',
        'role_created',
        'role_updated',
        'role_deleted'
    )
);
