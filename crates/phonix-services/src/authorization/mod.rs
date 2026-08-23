//! Authorization use cases: reading and changing who may do what.
//!
//! A folder beside [`identity`](crate::identity) rather than a crate, and
//! beside it rather than inside it, for the reason stated in
//! [`phonix_core::authorization`]: "who you are" and "what you may do" are
//! different questions, and the code that answers them should be as separable
//! as the questions are.
//!
//! | Module     | Use cases                                                  |
//! | ---------- | ---------------------------------------------------------- |
//! | [`grants`] | Read and edit a user's permissions, and a role's            |
//! | [`roles`]  | List, define, rename and remove the workspace's roles        |
//!
//! Every function here states its permission on the first line. The two that
//! change something name `*.ChangePermissions` rather than `*.Edit`, because
//! being allowed to rename a role is not the same as being allowed to hand
//! yourself the whole tree through it.

pub mod grants;
pub mod roles;
