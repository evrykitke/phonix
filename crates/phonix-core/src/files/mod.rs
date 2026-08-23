//! Files: what may be uploaded, what it turns out to be, and what became of it.
//!
//! Everything here is shared between the server and the browser, which is the
//! point. The `accept` attribute on a file input, the size a screen warns about
//! before uploading, and the rules the server applies to the bytes afterwards
//! all come from the same tables - so a screen cannot offer something the
//! server would refuse, and a new format is added in one place.
//!
//! | Module      | Question it answers                                  |
//! | ----------- | ---------------------------------------------------- |
//! | [`catalog`] | What is this file, judged from its bytes?            |
//! | [`bucket`]  | What is it for, and what does that permit?           |
//! | [`image`]   | How large is this picture, without decoding it?      |
//! | [`name`]    | What is safe to keep of the name it arrived with?    |
//! | [`upload`]  | Where has the job got to, and what came out of it?   |
//!
//! # The one rule the whole module exists to enforce
//!
//! **Nothing a caller says about a file decides anything.** Not the name, not
//! the extension, not the `Content-Type` the browser attached. Those are kept
//! so a rejection can quote them back and a download can offer the original
//! name, and they are consulted in exactly one place - breaking a tie between
//! formats that share a container, and only within that container.
//!
//! What decides is the bytes. See [`catalog::detect`].
//!
//! # And what follows from it
//!
//! Because the name decides nothing, nothing is ever *stored* under it either.
//! A stored file is renamed on the way in - see `phonix_storage::naming` - and
//! the path it lands at is built entirely from values this server generated. A
//! traversal attempt in a filename has nowhere to go, because no part of the
//! filename reaches the filesystem.

pub mod bucket;
pub mod catalog;
pub mod image;
pub mod name;
pub mod upload;

pub use bucket::{BUCKETS, BucketPolicy, bucket, largest_bucket_limit};
pub use catalog::{
    CATALOGUE, Container, FileCategory, FileType, Signature, by_extension, by_mime, detect,
    looks_like_text,
};
pub use image::Dimensions;
pub use name::{extension_of, human_size, sanitize_file_name};
pub use upload::{FileId, FileSummary, Rejection, UploadResult, UploadStatus};
