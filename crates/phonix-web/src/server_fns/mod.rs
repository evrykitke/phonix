//! Server functions, one file per scope.
//!
//! Each `#[server]` function compiles to two things: the real body on the
//! server, and an HTTP call on the client. The body is stripped from the wasm
//! build, so database and broker access inside one never reaches the browser.
//!
//! | Module           | Endpoints                                          |
//! | ---------------- | -------------------------------------------------- |
//! | [`tenant_fns`]   | Which workspace is this, and a smoke-test query     |
//! | [`onboarding_fns`] | Address availability, creating a workspace       |
//! | [`auth_fns`]     | Sign in, sign out, who am I                        |
//! | [`account_fns`]  | Your own profile, password and second factor       |
//! | [`settings_fns`] | This workspace's password and MFA policy           |
//! | [`admin_fns`]    | The people in this workspace, and what they may do |
//! | [`master_fns`]   | The parties it trades with, and the taxes it applies |
//! | [`currency_fns`] | What it deals in, and what a rate was on a day      |
//! | [`numbering_fns`] | What a document number looks like, and where it is |
//! | [`books_fns`]    | What it has invoiced, and what it is owed           |
//! | [`file_fns`]     | Where an upload got to, and what to do with it     |
//! | [`public_fns`]   | What a signed-out screen shows in its chrome        |
//! | [`reset_fns`]    | A forgotten password: ask for a code, spend it      |
//!
//! # These are thin on purpose
//!
//! A server function parses its input, calls **one** use case, and maps the
//! result. It does not open transactions, hash anything, or decide who may do
//! what: that is `phonix-services`, and a second implementation here would be
//! a second thing to get wrong.
//!
//! Rejections come back as `Ok(..)` carrying per-field messages -
//! `SignupResult::Rejected`, `LoginResult::Rejected`. An `Err` from one of
//! these means something broke, not that somebody mistyped their password.

pub mod account_fns;
pub mod admin_fns;
pub mod app_fns;
pub mod auth_fns;
pub mod books_fns;
pub mod currency_fns;
pub mod file_fns;
pub mod master_fns;
pub mod numbering_fns;
pub mod onboarding_fns;
pub mod public_fns;
pub mod reset_fns;
pub mod settings_fns;
pub mod tenant_fns;
