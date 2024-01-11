use crate::file::transaction::prepare_and_commit::{committer, create_at};
use crate::file::EmptyCommit;
use gix_lock::acquire::Fail;
use gix_ref::file::transaction::PackedRefs;
use gix_ref::store::WriteReflog;
use gix_ref::transaction::{Change, LogChange, PreviousValue, RefEdit};

mod access;
mod find;
mod iter;
mod reflog;

#[test]
fn precompose_unicode_journey() -> crate::Result {
    let tmp = gix_testtools::tempfile::TempDir::new()?;
    let precomposed_a = "ä";
    let decomposed_a = "a\u{308}";
    let root = tmp.path().join(decomposed_a);
    std::fs::create_dir(&root)?;

    let store_decomposed = gix_ref::file::Store::at(
        root,
        WriteReflog::Always,
        gix_hash::Kind::Sha1,
        false, /* precompose_unicode */
    );
    assert!(!store_decomposed.precompose_unicode);

    let decomposed_ref = format!("refs/heads/{decomposed_a}");
    store_decomposed
        .transaction()
        .prepare(Some(create_at(&decomposed_ref)), Fail::Immediately, Fail::Immediately)?
        .commit(committer().to_ref())?;

    let r = store_decomposed.iter()?.all()?.next().expect("created one ref")?;
    assert_eq!(r.name.as_bstr(), decomposed_ref, "no transformation happens by default");

    let fs_folds_decomposed = gix_fs::Capabilities::probe(tmp.path()).precompose_unicode;
    if !fs_folds_decomposed {
        // For file-access to work, we need a filesystem for which precomposed == decomposed.
        return Ok(());
    }

    let store_precomposed = gix_ref::file::Store::at(
        tmp.path().join(precomposed_a), // it's important that root paths are also precomposed then.
        WriteReflog::Always,
        gix_hash::Kind::Sha1,
        true, /* precompose_unicode */
    );

    let precomposed_ref = format!("refs/heads/{precomposed_a}");
    let r = store_precomposed.iter()?.all()?.next().expect("created one ref")?;
    assert_eq!(
        r.name.as_bstr(),
        precomposed_ref,
        "it transforms all refs it sees to precomposed format, important when sending them over the wire"
    );

    assert_eq!(
        store_precomposed.find(precomposed_a)?.name.as_bstr(),
        precomposed_ref,
        "can find as precomposed, even though on disk is decomposed it is decomposed"
    );
    assert_eq!(
        store_precomposed.find(decomposed_a)?.name.as_bstr(),
        decomposed_ref,
        "can find as decomposed, and it keeps it as is to not violate expectations of the returned name being equal to the input (when comparing as bytes)"
    );

    let decomposed_u = "u\u{308}";
    let decomposed_ref = format!("refs/heads/{decomposed_u}");
    let edits = store_precomposed
        .transaction()
        .prepare(Some(create_at(&decomposed_ref)), Fail::Immediately, Fail::Immediately)?
        .commit(committer().to_ref())?;
    assert_eq!(
        edits[0].name.as_bstr(),
        decomposed_ref,
        "it doesn't alter the composition style to allow input and output to remain unchanged"
    );

    assert_eq!(
        store_decomposed.iter()?.all()?.nth(1).expect("two refs")?.name.shorten(),
        decomposed_u,
        "the ref name isn't transformed in any way and left decomposed on disk as well, making sure internal loose/packed-ref interactions work reliably"
    );

    assert!(
        store_precomposed.cached_packed_buffer()?.is_none(),
        "no packed-refs yet"
    );
    let edits = store_precomposed
        .transaction()
        .packed_refs(PackedRefs::DeletionsAndNonSymbolicUpdatesRemoveLooseSourceReference(
            Box::new(EmptyCommit),
        ))
        .prepare(
            // Intentionally use the decomposed versions of their names
            store_decomposed
                .loose_iter()?
                .filter_map(|r| r.ok().filter(|r| r.kind() == gix_ref::Kind::Peeled))
                .map(|r| RefEdit {
                    change: Change::Update {
                        log: LogChange::default(),
                        expected: PreviousValue::MustExistAndMatch(r.target.clone()),
                        new: r.target,
                    },
                    name: r.name,
                    deref: false,
                }),
            Fail::Immediately,
            Fail::Immediately,
        )?
        .commit(committer().to_ref())?;
    assert!(
        store_precomposed.cached_packed_buffer()?.is_some(),
        "refs were written into the packed-refs file"
    );

    assert_eq!(store_precomposed.loose_iter()?.count(), 0, "all loose refs are gone");
    assert_eq!(edits.len(), 2);
    assert_eq!(
        edits[0].name.shorten(),
        decomposed_a,
        "composition stays the same for consistency"
    );
    assert_eq!(
        edits[1].name.shorten(),
        decomposed_u,
        "composition stays the same for consistency"
    );

    assert_eq!(
        store_decomposed.find(precomposed_a)?.name.shorten(),
        precomposed_a,
        "the decomposed store can only find what's in packed-refs verbatim"
    );
    assert!(
        store_decomposed.try_find(decomposed_a)?.is_none(),
        "decomposed inputs don't match in packed-refs"
    );
    assert_eq!(
        store_precomposed.find(precomposed_a)?.name.shorten(),
        precomposed_a,
        "we find what's in the packed-refs file, which is native to packed-refs"
    );
    assert_eq!(
        store_precomposed.find(decomposed_a)?.name.shorten(),
        decomposed_a,
        "despite the input being decomposed, we find the ref (in packed-refs) as precomposed, but return it just like we inserted it"
    );

    // TODO: symrefs
    // TODO: namespace

    Ok(())
}
