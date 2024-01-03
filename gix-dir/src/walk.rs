#![allow(dead_code)]
use std::path::PathBuf;

/// Options for use in [`walk()`](function::walk()) function.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub struct Options {
    /// If true, the filesystem will store paths as decomposed unicode, i.e. `ä` becomes `"a\u{308}"`, which means that
    /// we have to turn these forms back from decomposed to precomposed unicode before storing it in the index or generally
    /// using it. This also applies to input received from the command-line, so callers may have to be aware of this and
    /// perform conversions accordingly.
    /// If false, no conversions will be performed.
    pub precompose_unicode: bool,
    /// If true, the filesystem ignores the case of input, which makes `A` the same file as `a`.
    /// This is also called case-folding.
    pub ignore_case: bool,
}

/// Additional information collected as outcome of [`walk()`](function::walk()).
#[derive(Debug, Clone, Copy, Ord, PartialOrd, Eq, PartialEq)]
pub struct Outcome {
    /// The amount of calls to read the directory contents.
    pub read_dir_calls: usize,
    /// The amount of returned entries.
    pub returned_entries: usize,
    /// The amount of entries, prior to pathspecs filtering them out or otherwise excluding them.
    pub seen_entries: usize,
}

/// The error returned by [`walk()`](function::walk()).
#[derive(Debug, thiserror::Error)]
#[allow(missing_docs)]
pub enum Error {
    #[error("Traversal root '{}' contains relative path components and could not be normalized", root.display())]
    NormalizeRoot { root: PathBuf },
    #[error("Traversal root '{}' must be literally contained in worktree root '{}'", root.display(), worktree_root.display())]
    RootNotInWorktree { root: PathBuf, worktree_root: PathBuf },
    #[error("A symlink was found at component {component_index} of traversal root '{}' as seen from worktree root '{}'", root.display(), worktree_root.display())]
    SymlinkInRoot {
        root: PathBuf,
        worktree_root: PathBuf,
        /// This index starts at 0, with 0 being the first component.
        component_index: usize,
    },
    #[error("Could not obtain symlink metadata on '{}'", path.display())]
    SymlinkMetadata { path: PathBuf, source: std::io::Error },
}

/// A type returned by the `for_each` function passed to [`walk()`](function::walk()).
#[derive(Debug, Copy, Clone)]
pub enum Action {
    Continue,
    Cancel,
}

pub(crate) mod function {
    use crate::walk::{Action, Error, Options, Outcome};
    use crate::EntryRef;
    use bstr::BStr;
    use std::borrow::Cow;
    use std::path::{Path, PathBuf};

    /// A function to perform a git-style directory walk.
    ///
    /// * `root` - the starting point of the walk and a readable directory.
    ///     - Note that if the path leading to this directory or `root` itself is excluded, it will be provided to `for_each`
    ///       without further traversal.
    ///     - If [`Options::precompose_unicode`] is enabled, this path must be precomposed.
    /// * `worktree_root` - the top-most root of the worktree, which must be a prefix to `root`.
    ///     - If [`Options::precompose_unicode`] is enabled, this path must be precomposed.
    /// * `index` - access to see which files or directories are tracked.
    /// * `for_each` - called for each observed entry in the directory.
    ///
    /// ### Performance Notes
    ///
    /// In theory, parallel directory traversal can be significantly faster, and what's possible for our current
    /// `gix_features::fs::WalkDir` implementation is to abstract a `filter_entry()` method so it works both for
    /// the iterator from the `walkdir` crate as well as from `jwalk`. However, doing so as initial version
    /// has the risk of not being significantly harder if not impossible to implement as flow-control is very
    /// limited.
    ///
    /// Thus the decision was made to start out with something akin to the Git implementation, get all tests and
    /// baseline comparison to pass, and see if an iterator with just `filter_entry` would be capable of dealing with
    /// it. Note that `filter_entry` is the only just-in-time traversal control that `walkdir` offers, even though
    /// one could consider switching to `jwalk` and just use its single-threaded implementation if a unified interface
    /// is necessary to make this work - `jwalk` has a more powerful API for this to work.
    ///
    /// If that was the case, we are talking about 0.5s for single-threaded traversal (without doing any extra work)
    /// or 0.25s for optimal multi-threaded performance, all in the WebKit directory with 388k items to traverse.
    /// Thus, the speedup could easily be 2x or more and thus worth investigating in due time.
    pub fn walk(
        root: &Path,
        worktree_root: &Path,
        _index: &gix_index::State,
        _options: Options,
        _for_each: &dyn FnMut(EntryRef<'_>) -> Action,
    ) -> Result<Outcome, Error> {
        let (current, _worktree_root_relative) = assure_no_symlink_in_root(worktree_root, root)?;
        debug_assert_eq!(
            current, worktree_root,
            "BUG: we initialize our buffer with the worktree root"
        );

        todo!()
    }

    /// What kind of path we are seeing which helps to decide what to do with it.
    enum PathKind {
        /// The path can safely be traversed into, as it is a directory and it is not special or ignored.
        Directory,
        /// The path is not available in the index, nor is it ignored.
        Untracked,
    }

    /// Figure out what to do with `rela_path`, provided as worktree-relative path, with `is_dir` being `true`
    /// for directories.
    /// `filename_start_idx` is the index at which the filename begins, i.e. `a/b` has `1` as index.
    /// Returns `None` if we shouldn't do anything with it as `rela_path` is not included in pathspecs, or is named `.git`.
    fn classify_path(
        rela_path: &BStr,
        _is_dir: bool,
        filename_start_idx: usize,
        ignore_case: bool,
    ) -> Option<PathKind> {
        if is_eq(&rela_path[filename_start_idx..], ".git", ignore_case) {
            return None;
        }
        todo!()
    }

    fn is_eq(lhs: &BStr, rhs: impl AsRef<BStr>, ignore_case: bool) -> bool {
        if ignore_case {
            lhs.eq_ignore_ascii_case(rhs.as_ref().as_ref())
        } else {
            lhs == rhs.as_ref()
        }
    }

    fn classify_root(_worktree_relative_root: &Path) -> Option<PathKind> {
        todo!()
    }

    fn assure_no_symlink_in_root<'root>(
        worktree_root: &Path,
        root: &'root Path,
    ) -> Result<(PathBuf, Cow<'root, Path>), Error> {
        let mut current = worktree_root.to_owned();
        let worktree_relative = root.strip_prefix(worktree_root).map_err(|_| Error::RootNotInWorktree {
            worktree_root: worktree_root.to_owned(),
            root: root.to_owned(),
        })?;
        let worktree_relative = gix_path::normalize(worktree_relative.into(), Path::new(""))
            .ok_or(Error::NormalizeRoot { root: root.to_owned() })?;

        for (idx, component) in worktree_relative.components().enumerate() {
            current.push(component);
            let meta = current.symlink_metadata().map_err(|err| Error::SymlinkMetadata {
                source: err,
                path: current.to_owned(),
            })?;
            if meta.is_symlink() {
                return Err(Error::SymlinkInRoot {
                    root: root.to_owned(),
                    worktree_root: worktree_root.to_owned(),
                    component_index: idx,
                });
            }
        }
        for _ in worktree_relative.components() {
            current.pop();
        }

        Ok((current, worktree_relative))
    }
}
