//! Signing in with somebody else's identity provider.
//!
//! One provider so far. The module exists as a directory rather than a file
//! because the second one - Microsoft, or a workspace's own SAML - shares the
//! shape and none of the endpoints, and the place that will differ is exactly
//! the file boundary.
//!
//! # The rule every provider here obeys
//!
//! **Federation signs people in. It never creates them.** A provider vouches
//! that whoever is at the browser controls an address; it does not and cannot
//! say that address belongs in this workspace. Membership is decided by an
//! invitation somebody sent, and this module's job ends at matching a verified
//! address to an account that already exists.
//!
//! Getting that backwards is how "Continue with Google" becomes a public
//! signup form for a private workspace.

pub mod google;
