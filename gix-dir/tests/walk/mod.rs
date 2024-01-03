use gix_dir::{walk, Entry, EntryRef};
use gix_testtools::scripted_fixture_read_only;
use std::path::{Path, PathBuf};

use gix_dir::entry::Kind::*;
use gix_dir::entry::Mode::*;

mod baseline {
    use std::path::Path;

    /// Parse multiple walks out of a single `fixture`.
    pub fn extract_walks(_fixture: &Path) -> crate::Result {
        Ok(())
    }
}

#[test]
fn baseline() -> crate::Result {
    baseline::extract_walks(&scripted_fixture_read_only("walk_baseline.sh")?)?;
    Ok(())
}

#[test]
#[cfg(not(windows))]
fn root_may_not_lead_through_symlinks() -> crate::Result {
    for (name, intermediate, expected) in [
        ("immediate-breakout-symlink", "", 0),
        ("breakout-symlink", "hide", 1),
        ("breakout-symlink", "hide/../hide", 1),
    ] {
        let root = fixture(name);
        let err = try_collect(|keep, index| {
            walk(
                &root.join(intermediate).join("breakout"),
                &root,
                &index,
                options(),
                keep,
            )
        })
        .unwrap_err();
        assert!(
            matches!(err, walk::Error::SymlinkInRoot { component_index, .. } if component_index == expected),
            "{name} should have component {expected}"
        );
    }
    Ok(())
}

#[test]
fn root_must_be_in_worktree() -> crate::Result {
    let err = try_collect(|keep, index| {
        walk(
            "traversal".as_ref(),
            "unrelated-worktree".as_ref(),
            &index,
            options(),
            keep,
        )
    })
    .unwrap_err();
    assert!(matches!(err, walk::Error::RootNotInWorktree { .. }));
    Ok(())
}

#[test]
#[ignore = "WIP"]
fn worktree_root_can_be_symlink() -> crate::Result {
    let root = fixture("symlink-to-breakout-symlink");
    let (out, entries) = collect(|keep, index| walk(&root, &root, &index, options(), keep));
    assert_eq!(
        out,
        walk::Outcome {
            read_dir_calls: 1,
            returned_entries: entries.len(),
            seen_entries: 1,
        }
    );
    assert_eq!(entries.len(), 1);

    let entry = |a, b, c| entry(root.join(a), b, c);
    assert_eq!(&entries[0], &entry("hide", Untracked, Directory));
    Ok(())
}

#[test]
#[ignore = "assure we apply standard filters and checks even for roots"]
fn root_may_not_go_through_dot_git() {}

#[test]
#[ignore = "assure we apply standard filters and checks even for roots"]
fn root_has_pathspec_filter_applied() {}

#[test]
#[ignore = "assure we apply standard filters and checks even for roots"]
fn root_that_is_ignored_is_listed() {}

#[test]
#[ignore = "assure we apply standard filters and checks even for roots"]
fn root_that_is_untracked_is_listed() {}

#[test]
#[ignore = "to be implemented"]
fn precompose_unicode() {}

#[test]
#[ignore = "need case-insensitive testing as well"]
fn case_insensitive_usecases() {}

#[test]
#[ignore = "what about partial checkouts?"]
fn partial_checkouts() {}

#[test]
#[ignore = "what about submodules - are they treated specifically?"]
fn submodules() {}

fn fixture(name: &str) -> PathBuf {
    let root = scripted_fixture_read_only("many.sh").expect("script works");
    root.join(name)
}

/// Default options
fn options() -> walk::Options {
    walk::Options {
        precompose_unicode: false,
        ignore_case: false,
    }
}

fn entry(path: impl AsRef<Path>, kind: gix_dir::entry::Kind, mode: gix_dir::entry::Mode) -> Entry {
    Entry {
        path: path.as_ref().to_owned(),
        kind,
        mode,
    }
}

fn collect(
    cb: impl FnOnce(&dyn FnMut(EntryRef<'_>) -> walk::Action, gix_index::State) -> Result<walk::Outcome, walk::Error>,
) -> (walk::Outcome, Vec<Entry>) {
    try_collect(cb).unwrap()
}

fn try_collect(
    cb: impl FnOnce(&dyn FnMut(EntryRef<'_>) -> walk::Action, gix_index::State) -> Result<walk::Outcome, walk::Error>,
) -> Result<(walk::Outcome, Vec<Entry>), walk::Error> {
    let state = gix_index::State::new(gix_index::hash::Kind::Sha1);
    let mut out = Vec::new();
    let outcome = cb(
        &mut |entry| {
            out.push(entry.to_owned());
            walk::Action::Continue
        },
        state,
    )?;
    Ok((outcome, out))
}
