//! Pure data types representing a parsed gettext PO file.
//!
//! Nothing in this module performs file I/O — these are just structs and
//! impls. Parsing lives in [`crate::service::parser`] and serialization in
//! [`crate::service::serializer`].

pub mod entry;
pub mod file;

pub use entry::MessageEntry;
pub use file::GettextFile;
