use std::{
    cell::RefCell,
    collections::HashSet,
    sync::{atomic::AtomicBool, Arc},
};

use git_features::{parallel, progress::Progress};
use git_hash::ObjectId;

use crate::{data::output, find};

pub(in crate::data::output::count::objects_impl) mod reduce;
mod util;

mod types;
pub use types::{Error, ObjectExpansion, Options, Outcome};
mod tree;

/// The return type used by [`objects()`].
pub type Result<E1, E2> = std::result::Result<(Vec<output::Count>, Outcome), Error<E1, E2>>;

/// Generate [`Count`][output::Count]s from input `objects` with object expansion based on [`options`][Options]
/// to learn which objects would would constitute a pack. This step is required to know exactly how many objects would
/// be in a pack while keeping data around to avoid minimize object database access.
///
/// A [`Count`][output::Count] object maintains enough state to greatly accelerate future access of packed objects.
///
/// * `db` - the object store to use for accessing objects.
/// * `make_cache` - a function to create thread-local pack caches
/// * `objects_ids`
///   * A list of objects ids to add to the pack. Duplication checks are performed so no object is ever added to a pack twice.
///   * Objects may be expanded based on the provided [`options`][Options]
/// * `progress`
///   * a way to obtain progress information
/// * `should_interrupt`
///  * A flag that is set to true if the operation should stop
/// * `options`
///   * more configuration
pub fn objects<Find, Iter, IterErr, Oid, Cache>(
    db: Find,
    make_cache: impl Fn() -> Cache + Send + Sync,
    objects_ids: Iter,
    progress: impl Progress,
    should_interrupt: &AtomicBool,
    Options {
        thread_limit,
        input_object_expansion,
        chunk_size,
        #[cfg(feature = "object-cache-dynamic")]
        object_cache_size_in_bytes,
    }: Options,
) -> Result<find::existing::Error<Find::Error>, IterErr>
where
    Find: crate::Find + Send + Sync,
    <Find as crate::Find>::Error: Send,
    Iter: Iterator<Item = std::result::Result<Oid, IterErr>> + Send,
    Oid: Into<ObjectId> + Send,
    IterErr: std::error::Error + Send,
    Cache: crate::cache::DecodeEntry,
{
    let lower_bound = objects_ids.size_hint().0;
    let (chunk_size, thread_limit, _) = parallel::optimize_chunk_size_and_thread_limit(
        chunk_size,
        if lower_bound == 0 { None } else { Some(lower_bound) },
        thread_limit,
        None,
    );
    let chunks = util::Chunks {
        iter: objects_ids,
        size: chunk_size,
    };
    let seen_objs = dashmap::DashSet::<ObjectId>::new();
    let progress = Arc::new(parking_lot::Mutex::new(progress));

    parallel::in_parallel(
        chunks,
        thread_limit,
        {
            let progress = Arc::clone(&progress);
            move |n| {
                (
                    Vec::new(),   // object data buffer
                    Vec::new(),   // object data buffer 2 to hold two objects at a time
                    make_cache(), // cache to speed up pack operations
                    {
                        let mut p = progress.lock().add_child(format!("thread {}", n));
                        p.init(None, git_features::progress::count("objects"));
                        p
                    },
                )
            }
        },
        {
            move |oids: Vec<std::result::Result<Oid, IterErr>>, (buf1, buf2, cache, progress)| {
                expand::this(
                    &db,
                    input_object_expansion,
                    &seen_objs,
                    oids,
                    buf1,
                    buf2,
                    cache,
                    progress,
                    should_interrupt,
                    true,
                    #[cfg(feature = "object-cache-dynamic")]
                    object_cache_size_in_bytes,
                )
            }
        },
        reduce::Statistics::new(progress),
    )
}

/// Like [`objects()`] but using a single thread only to mostly save on the otherwise required overhead.
pub fn objects_unthreaded<Find, IterErr, Oid>(
    db: Find,
    pack_cache: &mut impl crate::cache::DecodeEntry,
    object_ids: impl Iterator<Item = std::result::Result<Oid, IterErr>>,
    mut progress: impl Progress,
    should_interrupt: &AtomicBool,
    input_object_expansion: ObjectExpansion,
    #[cfg(feature = "object-cache-dynamic")] object_cache_size_in_bytes: usize,
) -> Result<find::existing::Error<Find::Error>, IterErr>
where
    Find: crate::Find + Send + Sync,
    Oid: Into<ObjectId> + Send,
    IterErr: std::error::Error + Send,
{
    let seen_objs = RefCell::new(HashSet::<ObjectId>::new());

    let (mut buf1, mut buf2) = (Vec::new(), Vec::new());
    expand::this(
        &db,
        input_object_expansion,
        &seen_objs,
        object_ids,
        &mut buf1,
        &mut buf2,
        pack_cache,
        &mut progress,
        should_interrupt,
        false,
        #[cfg(feature = "object-cache-dynamic")]
        object_cache_size_in_bytes,
    )
}

mod expand {
    use std::sync::atomic::{AtomicBool, Ordering};

    use git_features::progress::Progress;
    use git_hash::{oid, ObjectId};
    use git_object::{CommitRefIter, TagRefIter};

    use super::{
        tree,
        types::{Error, ObjectExpansion, Outcome},
        util,
    };
    use crate::{
        cache::Object,
        data::{output, output::count::PackLocation},
        find, FindExt,
    };

    #[allow(clippy::too_many_arguments)]
    pub fn this<Find, IterErr, Oid>(
        db: &Find,
        input_object_expansion: ObjectExpansion,
        seen_objs: &impl util::InsertImmutable<ObjectId>,
        oids: impl IntoIterator<Item = std::result::Result<Oid, IterErr>>,
        buf1: &mut Vec<u8>,
        buf2: &mut Vec<u8>,
        cache: &mut impl crate::cache::DecodeEntry,
        progress: &mut impl Progress,
        should_interrupt: &AtomicBool,
        allow_pack_lookups: bool,
        #[cfg(feature = "object-cache-dynamic")] object_cache_size_in_bytes: usize,
    ) -> super::Result<find::existing::Error<Find::Error>, IterErr>
    where
        Find: crate::Find + Send + Sync,
        Oid: Into<ObjectId> + Send,
        IterErr: std::error::Error + Send,
    {
        use ObjectExpansion::*;

        let mut out = Vec::new();
        let mut tree_traversal_state = git_traverse::tree::breadthfirst::State::default();
        let mut tree_diff_state = git_diff::tree::State::default();
        let mut parent_commit_ids = Vec::new();
        let mut traverse_delegate = tree::traverse::AllUnseen::new(seen_objs);
        let mut changes_delegate = tree::changes::AllNew::new(seen_objs);
        let mut outcome = Outcome::default();
        #[cfg(feature = "object-cache-dynamic")]
        let mut obj_cache = crate::cache::object::MemoryCappedHashmap::new(object_cache_size_in_bytes);
        #[cfg(not(feature = "object-cache-dynamic"))]
        let mut obj_cache = crate::cache::object::Never;

        let stats = &mut outcome;
        for id in oids.into_iter() {
            if should_interrupt.load(Ordering::Relaxed) {
                return Err(Error::Interrupted);
            }

            let id = id.map(|oid| oid.into()).map_err(Error::InputIteration)?;
            let obj = db.find(id, buf1, cache)?;
            stats.input_objects += 1;
            match input_object_expansion {
                TreeAdditionsComparedToAncestor => {
                    use git_object::Kind::*;
                    let mut obj = obj;
                    let mut id = id.to_owned();

                    loop {
                        push_obj_count_unique(&mut out, seen_objs, &id, &obj, progress, stats, false);
                        match obj.kind {
                            Tree | Blob => break,
                            Tag => {
                                id = TagRefIter::from_bytes(obj.data)
                                    .target_id()
                                    .expect("every tag has a target");
                                obj = db.find(id, buf1, cache)?;
                                stats.expanded_objects += 1;
                                continue;
                            }
                            Commit => {
                                let current_tree_iter = {
                                    let mut commit_iter = CommitRefIter::from_bytes(obj.data);
                                    let tree_id = commit_iter.tree_id().expect("every commit has a tree");
                                    parent_commit_ids.clear();
                                    for token in commit_iter {
                                        match token {
                                            Ok(git_object::commit::ref_iter::Token::Parent { id }) => {
                                                parent_commit_ids.push(id)
                                            }
                                            Ok(_) => break,
                                            Err(err) => return Err(Error::CommitDecode(err)),
                                        }
                                    }
                                    let obj = db.find(tree_id, buf1, cache)?;
                                    push_obj_count_unique(&mut out, seen_objs, &tree_id, &obj, progress, stats, true);
                                    git_object::TreeRefIter::from_bytes(obj.data)
                                };

                                let objects = if parent_commit_ids.is_empty() {
                                    traverse_delegate.clear();
                                    git_traverse::tree::breadthfirst(
                                        current_tree_iter,
                                        &mut tree_traversal_state,
                                        |oid, buf| {
                                            stats.decoded_objects += 1;
                                            match db.find(oid, buf, cache).ok() {
                                                Some(obj) => {
                                                    progress.inc();
                                                    stats.expanded_objects += 1;
                                                    out.push(output::Count::from_data(oid, &obj));
                                                    obj.try_into_tree_iter()
                                                }
                                                None => None,
                                            }
                                        },
                                        &mut traverse_delegate,
                                    )
                                    .map_err(Error::TreeTraverse)?;
                                    &traverse_delegate.non_trees
                                } else {
                                    for commit_id in &parent_commit_ids {
                                        let parent_tree_id = {
                                            let parent_commit_obj = db.find(commit_id, buf2, cache)?;

                                            push_obj_count_unique(
                                                &mut out,
                                                seen_objs,
                                                commit_id,
                                                &parent_commit_obj,
                                                progress,
                                                stats,
                                                true,
                                            );
                                            CommitRefIter::from_bytes(parent_commit_obj.data)
                                                .tree_id()
                                                .expect("every commit has a tree")
                                        };
                                        let parent_tree = {
                                            let parent_tree_obj = db.find(parent_tree_id, buf2, cache)?;
                                            push_obj_count_unique(
                                                &mut out,
                                                seen_objs,
                                                &parent_tree_id,
                                                &parent_tree_obj,
                                                progress,
                                                stats,
                                                true,
                                            );
                                            git_object::TreeRefIter::from_bytes(parent_tree_obj.data)
                                        };

                                        changes_delegate.clear();
                                        git_diff::tree::Changes::from(Some(parent_tree))
                                            .needed_to_obtain(
                                                current_tree_iter.clone(),
                                                &mut tree_diff_state,
                                                |oid, buf| {
                                                    stats.decoded_objects += 1;
                                                    let id = oid.to_owned();
                                                    match obj_cache.get(&id, buf) {
                                                        Some(_kind) => git_object::TreeRefIter::from_bytes(buf).into(),
                                                        None => match db.find_tree_iter(oid, buf, cache).ok() {
                                                            Some(_) => {
                                                                obj_cache.put(id, git_object::Kind::Tree, buf);
                                                                git_object::TreeRefIter::from_bytes(buf).into()
                                                            }
                                                            None => None,
                                                        },
                                                    }
                                                },
                                                &mut changes_delegate,
                                            )
                                            .map_err(Error::TreeChanges)?;
                                    }
                                    &changes_delegate.objects
                                };
                                for id in objects.iter() {
                                    out.push(id_to_count(db, buf2, id, progress, stats, allow_pack_lookups));
                                }
                                break;
                            }
                        }
                    }
                }
                TreeContents => {
                    use git_object::Kind::*;
                    let mut id = id;
                    let mut obj = obj;
                    loop {
                        push_obj_count_unique(&mut out, seen_objs, &id, &obj, progress, stats, false);
                        match obj.kind {
                            Tree => {
                                traverse_delegate.clear();
                                git_traverse::tree::breadthfirst(
                                    git_object::TreeRefIter::from_bytes(obj.data),
                                    &mut tree_traversal_state,
                                    |oid, buf| {
                                        stats.decoded_objects += 1;
                                        match db.find(oid, buf, cache).ok() {
                                            Some(obj) => {
                                                progress.inc();
                                                stats.expanded_objects += 1;
                                                out.push(output::Count::from_data(oid, &obj));
                                                obj.try_into_tree_iter()
                                            }
                                            None => None,
                                        }
                                    },
                                    &mut traverse_delegate,
                                )
                                .map_err(Error::TreeTraverse)?;
                                for id in traverse_delegate.non_trees.iter() {
                                    out.push(id_to_count(db, buf1, id, progress, stats, allow_pack_lookups));
                                }
                                break;
                            }
                            Commit => {
                                id = CommitRefIter::from_bytes(obj.data)
                                    .tree_id()
                                    .expect("every commit has a tree");
                                stats.expanded_objects += 1;
                                obj = db.find(id, buf1, cache)?;
                                continue;
                            }
                            Blob => break,
                            Tag => {
                                id = TagRefIter::from_bytes(obj.data)
                                    .target_id()
                                    .expect("every tag has a target");
                                stats.expanded_objects += 1;
                                obj = db.find(id, buf1, cache)?;
                                continue;
                            }
                        }
                    }
                }
                AsIs => push_obj_count_unique(&mut out, seen_objs, &id, &obj, progress, stats, false),
            }
        }
        Ok((out, outcome))
    }

    #[inline]
    fn push_obj_count_unique(
        out: &mut Vec<output::Count>,
        all_seen: &impl util::InsertImmutable<ObjectId>,
        id: &oid,
        obj: &crate::data::Object<'_>,
        progress: &mut impl Progress,
        statistics: &mut Outcome,
        count_expanded: bool,
    ) {
        let inserted = all_seen.insert(id.to_owned());
        if inserted {
            progress.inc();
            statistics.decoded_objects += 1;
            if count_expanded {
                statistics.expanded_objects += 1;
            }
            out.push(output::Count::from_data(id, obj));
        }
    }

    #[inline]
    fn id_to_count<Find: crate::Find>(
        db: &Find,
        buf: &mut Vec<u8>,
        id: &oid,
        progress: &mut impl Progress,
        statistics: &mut Outcome,
        allow_pack_lookups: bool,
    ) -> output::Count {
        progress.inc();
        statistics.expanded_objects += 1;
        output::Count {
            id: id.to_owned(),
            entry_pack_location: if allow_pack_lookups {
                PackLocation::LookedUp(db.location_by_oid(id, buf))
            } else {
                PackLocation::NotLookedUp
            },
        }
    }
}