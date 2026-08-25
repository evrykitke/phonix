//! Every sentence the application says, in whatever language it is being read.
//!
//! # The rule
//!
//! **No text that reaches a person is written at the place it is used.** A
//! validator, a button, a page heading and an email all name a key; the key is
//! turned into words once, as late as possible, against the reader's catalog.
//!
//! ```ignore
//! // in phonix-services / phonix-core: what to say
//! FieldError::new("password", msg!("validation.password.needs_digit"))
//!
//! // in phonix-web: how to say it, to this reader, now
//! <label>{l!("auth.signin.password")}</label>
//! ```
//!
//! # What is *not* translated
//!
//! Anything a person typed. A role called "Receptionist", a workspace name, an
//! uploaded filename, an address in a trail row - those are data. Only the
//! shell around them changes language. The distinction matters because the
//! alternative is an application that mangles somebody's name when they switch
//! to French.
//!
//! Audit rows are the sharp case: they record what *happened*, not what it
//! looked like in English on the day. So a trail stores a kind and an action
//! and is narrated on the way out, which is what it already did - see
//! [`crate::audit`].
//!
//! # The pieces
//!
//! | Type              | Question it answers                            |
//! | ----------------- | ---------------------------------------------- |
//! | [`Language`]      | Which language, and which way does it run?      |
//! | [`Message`]       | What is being said, before anyone picks words?  |
//! | [`Catalog`]       | What are the words, in this language?           |
//! | [`msg!`](crate::msg) / [`pmsg!`](crate::pmsg) | Build a message, checking the key at compile time |
//!
//! `phonix-web` adds `l!`, which is `msg!` resolved immediately against the
//! catalog in context - the form a view wants.
//!
//! # Where the words live
//!
//! ```text
//! crates/phonix-core/i18n/en.json   the source of truth; compiled in
//! locales/<code>.json               deployment overrides, read at boot
//! ```
//!
//! English is compiled into the binary and the wasm bundle so that nothing can
//! ever render blank, and so that `msg!` can reject an unknown key at compile
//! time. Everything else is a file an operator can add, fix or finish without a
//! rebuild.
//!
//! # Why this is not a translation crate
//!
//! `rust-i18n` keeps the active locale in a process-wide global, which is wrong
//! for a server rendering two requests in two languages at once. `fluent` is a
//! better *format* than this one and costs a parser in the wasm bundle plus a
//! second syntax for translators to learn. `icu` is correct and heavy.
//!
//! None of the three can do the thing that actually pays for itself here:
//! **fail the build over a mistyped key**. That needs the catalog present at
//! compile time in the same crate as the macro, which is what this module is.

pub mod catalog;
pub mod datetime;
pub mod language;
pub mod message;

pub use catalog::{Catalog, builtin_contains, builtin_keys};
pub use language::{Direction, Language};
pub use message::{Arg, Message};
