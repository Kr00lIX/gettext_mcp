//! Domain services: parser, serializer, store, manager.
//!
//! The parser/serializer modules are pure (no I/O), while `store` and
//! `manager` route file access through the [`crate::io::FileStore`]
//! abstraction.

pub mod manager;
pub mod parser;
pub mod serializer;
pub mod store;

pub use manager::GettextStoreManager;
pub use store::GettextStore;
