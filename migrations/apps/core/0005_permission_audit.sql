-- ---------------------------------------------------------------------------
-- 0005: audit events for permission changes.
--
-- `role_changed` already records "this person was given that role". It cannot
-- record "that role now grants one more thing", nor "this person, alone, was
-- denied something their role gives" - and those are the two changes an audit
-- actually chases after a privilege escalation.
--
-- Two events rather than one. A role change reaches everybody holding it and is
-- read by whoever reviews the role; an individual override reaches one person
-- and is read by whoever reviews that account. Filtering them apart with a JSON
-- predicate would work and would be slower and less obvious.
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
        'role_permissions_changed'
    )
);
