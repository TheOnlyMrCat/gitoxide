use crate::{Entry, EntryRef};

/// The git-style filesystem mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub enum Mode {
    /// The entry is a blob, executable or not.
    Blob,
    /// The entry is a symlink.
    Symlink,
    /// The entry is an ordinary directory, which is either untracked or ignored along with all its contents.
    Directory,
    /// The entry is a directory which contains a `.git` folder.
    ///
    /// Note that we don't know if it's a submodule as we don't have `.gitmodules` information.
    Repository,
}

/// The kind of entry as obtained from a directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub enum Kind {
    /// The entry is not tracked by git yet, it was not found in the [index](gix_index::State).
    Untracked,
}

impl EntryRef<'_> {
    /// Strip the lifetime to obtain a fully owned copy.
    pub fn to_owned(&self) -> Entry {
        Entry {
            path: self.path.to_owned(),
            kind: self.kind,
            mode: self.mode,
        }
    }
}

impl Entry {
    /// Obtain an [`EntryRef`] from this instance.
    pub fn to_ref(&self) -> EntryRef<'_> {
        EntryRef {
            path: &self.path,
            kind: self.kind,
            mode: self.mode,
        }
    }
}
