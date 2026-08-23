//! Screens that run before there is a session.
//!
//! `sign_in` is `/`, `sign_up` is `/signup`. Both are reachable without a
//! tenant, which is what separates them from everything else in `pages/`.
//!
//! `accept_invitation` is `/invitations/accept`, and belongs here for the same
//! reason: the person following an invitation link has no session yet, which is
//! the entire point of an invitation.
//!
//! `challenge` is the odd one: a session exists by the time it renders, but it
//! has not finished authenticating, so nothing in `pages/admin` would let it
//! through either. It belongs here because it is part of signing in.

pub mod accept_invitation;
pub mod challenge;
pub mod sign_in;
pub mod sign_up;

pub use accept_invitation::AcceptInvitationPage;
pub use challenge::ChallengePage;
pub use sign_in::SignInPage;
pub use sign_up::SignUpPage;
