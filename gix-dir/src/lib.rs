//! A crate for handling a git-style directory walk.
#![deny(rust_2018_idioms)]
#![forbid(unsafe_code)]
use std::path::{Path, PathBuf};

/// A directory entry, typically obtained using [`walk()`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub struct EntryRef<'a> {
    /// The path at which the file or directory could be found, always with `root` as prefix,
    /// the first parameter of [`walk()`].
    pub path: &'a Path,
    /// The kind of entry.
    pub kind: entry::Kind,
    /// Further specify the what the entry is, similar to a file mode.
    pub mode: entry::Mode,
}

/// Just like [`EntryRef`], but with all fields owned (and thus without a lifetime to consider).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub struct Entry {
    /// The path at which the file or directory could be found, always with `root` as prefix,
    /// the first parameter of [`walk()`].
    pub path: PathBuf,
    /// The kind of entry.
    pub kind: entry::Kind,
    /// Further specify the what the entry is, similar to a file mode.
    pub mode: entry::Mode,
}

///
pub mod entry;

///
pub mod walk;
pub use walk::function::walk;
