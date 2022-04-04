use std::{
    borrow::Cow,
    fmt::{Display, Formatter},
};

use git_object::bstr::BStr;

#[derive(Debug, Clone)]
pub struct Outcome<'name> {
    pub name: Option<Cow<'name, BStr>>,
    pub id: git_hash::ObjectId,
    pub depth: u32,
    pub name_by_oid: std::collections::HashMap<git_hash::ObjectId, Cow<'name, BStr>>,
}

impl<'a> Outcome<'a> {
    pub fn into_format(self, hex_len: usize) -> Format<'a> {
        Format {
            name: self.name,
            id: self.id,
            hex_len,
            depth: self.depth,
            long: false,
            dirty_suffix: None,
        }
    }
}

#[derive(PartialEq, Eq, Debug, Hash, Ord, PartialOrd, Clone)]
pub struct Format<'a> {
    pub name: Option<Cow<'a, BStr>>,
    pub id: git_hash::ObjectId,
    pub hex_len: usize,
    pub depth: u32,
    pub long: bool,
    pub dirty_suffix: Option<String>,
}

impl<'a> Format<'a> {
    pub fn is_exact_match(&self) -> bool {
        self.depth == 0
    }
    pub fn long(&mut self) -> &mut Self {
        self.long = true;
        self
    }
    pub fn short(&mut self) -> &mut Self {
        self.long = false;
        self
    }
}

impl<'a> Display for Format<'a> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        if let Some(name) = self.name.as_deref() {
            if !self.long && self.is_exact_match() {
                name.fmt(f)?;
            } else {
                write!(f, "{}-{}-g{}", name, self.depth, self.id.to_hex_with_len(self.hex_len))?;
            }
        } else {
            self.id.to_hex_with_len(self.hex_len).fmt(f)?;
        }

        if let Some(suffix) = &self.dirty_suffix {
            write!(f, "-{}", suffix)?;
        }
        Ok(())
    }
}

type Flags = u32;
const MAX_CANDIDATES: usize = std::mem::size_of::<Flags>() * 8;

#[derive(Clone, Debug)]
pub struct Options<'name> {
    pub name_by_oid: std::collections::HashMap<git_hash::ObjectId, Cow<'name, BStr>>,
    /// The amount of names we will keep track of. Defaults to the maximum of 32.
    ///
    /// If the number is exceeded, it will be capped at 32.
    pub max_candidates: usize,
    /// If no candidate for naming, always show the abbreviated hash. Default: false.
    pub fallback_to_oid: bool,
}

impl<'name> Default for Options<'name> {
    fn default() -> Self {
        Options {
            max_candidates: MAX_CANDIDATES,
            name_by_oid: Default::default(),
            fallback_to_oid: false,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error<E>
where
    E: std::error::Error + Send + Sync + 'static,
{
    #[error("Commit {} could not be found during graph traversal", .oid.to_hex())]
    Find {
        #[source]
        err: E,
        oid: git_hash::ObjectId,
    },
    #[error("A commit could not be decoded during traversal")]
    Decode(#[from] git_object::decode::Error),
}

pub(crate) mod function {
    use super::Error;
    use hash_hasher::HashBuildHasher;
    use std::collections::HashMap;
    use std::{
        borrow::Cow,
        collections::{hash_map, VecDeque},
        iter::FromIterator,
    };

    use crate::describe::{Flags, Options, MAX_CANDIDATES};
    use git_hash::oid;
    use git_object::{bstr::BStr, CommitRefIter};

    use super::Outcome;

    pub fn describe<'name, Find, E>(
        commit: &oid,
        mut find: Find,
        Options {
            name_by_oid,
            mut max_candidates,
            fallback_to_oid,
        }: Options<'name>,
    ) -> Result<Option<Outcome<'name>>, Error<E>>
    where
        Find: for<'b> FnMut(&oid, &'b mut Vec<u8>) -> Result<CommitRefIter<'b>, E>,
        E: std::error::Error + Send + Sync + 'static,
    {
        if let Some(name) = name_by_oid.get(commit) {
            return Ok(Some(Outcome {
                name: name.clone().into(),
                id: commit.to_owned(),
                depth: 0,
                name_by_oid,
            }));
        }
        max_candidates = max_candidates.min(MAX_CANDIDATES);

        let mut buf = Vec::new();
        let mut parent_buf = Vec::new();
        let mut parents = Vec::new();

        let mut queue = VecDeque::from_iter(Some(commit.to_owned()));
        let mut candidates = Vec::new();
        let mut seen_commits = 0;
        let mut gave_up_on_commit = None;
        let mut seen = hash_hasher::HashedMap::default();
        seen.insert(commit.to_owned(), 0u32);

        while let Some(commit) = queue.pop_front() {
            seen_commits += 1;
            if let Some(name) = name_by_oid.get(&commit) {
                if candidates.len() < max_candidates {
                    let identity_bit = 1 << candidates.len();
                    candidates.push(Candidate {
                        name: name.clone(),
                        commits_in_its_future: seen_commits - 1,
                        identity_bit,
                        order: candidates.len(),
                    });
                    *seen.get_mut(&commit).expect("inserted") |= identity_bit;
                } else {
                    gave_up_on_commit = Some(commit);
                    break;
                }
            }

            let flags = seen[&commit];
            for candidate in candidates
                .iter_mut()
                .filter(|c| (flags & c.identity_bit) != c.identity_bit)
            {
                candidate.commits_in_its_future += 1;
            }

            parents_by_date_onto_queue_and_track_names(
                &mut find,
                &mut buf,
                &mut parent_buf,
                &mut parents,
                &mut queue,
                &mut seen,
                &commit,
                flags,
            )?;
        }

        if candidates.is_empty() {
            return if fallback_to_oid {
                Ok(Some(Outcome {
                    id: commit.to_owned(),
                    name: None,
                    name_by_oid,
                    depth: 0,
                }))
            } else {
                Ok(None)
            };
        }

        candidates.sort_by(|a, b| {
            a.commits_in_its_future
                .cmp(&b.commits_in_its_future)
                .then_with(|| a.order.cmp(&b.order))
        });

        if let Some(commit_id) = gave_up_on_commit {
            queue.push_front(commit_id);
        }

        finish_depth_computation(
            queue,
            find,
            candidates.first_mut().expect("at least one candidate"),
            seen,
            buf,
            parent_buf,
            parents,
        )?;

        Ok(candidates.into_iter().next().map(|c| Outcome {
            name: c.name.into(),
            id: commit.to_owned(),
            depth: c.commits_in_its_future,
            name_by_oid,
        }))
    }

    #[allow(clippy::too_many_arguments)]
    fn parents_by_date_onto_queue_and_track_names<Find, E>(
        find: &mut Find,
        buf: &mut Vec<u8>,
        parent_buf: &mut Vec<u8>,
        parents: &mut Vec<(git_hash::ObjectId, Flags)>,
        queue: &mut VecDeque<git_hash::ObjectId>,
        seen: &mut HashMap<git_hash::ObjectId, Flags, HashBuildHasher>,
        commit: &git_hash::oid,
        commit_flags: Flags,
    ) -> Result<(), Error<E>>
    where
        Find: for<'b> FnMut(&oid, &'b mut Vec<u8>) -> Result<CommitRefIter<'b>, E>,
        E: std::error::Error + Send + Sync + 'static,
    {
        let commit_iter = find(commit, buf).map_err(|err| Error::Find {
            err,
            oid: commit.to_owned(),
        })?;
        parents.clear();
        for token in commit_iter {
            match token {
                Ok(git_object::commit::ref_iter::Token::Tree { .. }) => continue,
                Ok(git_object::commit::ref_iter::Token::Parent { id: parent_id }) => match seen.entry(parent_id) {
                    hash_map::Entry::Vacant(entry) => {
                        let parent = find(&parent_id, parent_buf).map_err(|err| Error::Find {
                            err,
                            oid: commit.to_owned(),
                        })?;

                        let parent_commit_date = parent
                            .committer()
                            .map(|committer| committer.time.seconds_since_unix_epoch)
                            .unwrap_or_default();

                        entry.insert(commit_flags);
                        parents.push((parent_id, parent_commit_date));
                    }
                    hash_map::Entry::Occupied(mut entry) => {
                        *entry.get_mut() |= commit_flags;
                    }
                },
                Ok(_unused_token) => break,
                Err(err) => return Err(err.into()),
            }
        }

        parents.sort_by(|a, b| a.1.cmp(&b.1).reverse());
        queue.extend(parents.iter().map(|e| e.0));

        Ok(())
    }

    fn finish_depth_computation<'name, Find, E>(
        mut queue: VecDeque<git_hash::ObjectId>,
        mut find: Find,
        best_candidate: &mut Candidate<'name>,
        mut seen: hash_hasher::HashedMap<git_hash::ObjectId, Flags>,
        mut buf: Vec<u8>,
        mut parent_buf: Vec<u8>,
        mut parents: Vec<(git_hash::ObjectId, Flags)>,
    ) -> Result<(), Error<E>>
    where
        Find: for<'b> FnMut(&oid, &'b mut Vec<u8>) -> Result<CommitRefIter<'b>, E>,
        E: std::error::Error + Send + Sync + 'static,
    {
        while let Some(commit) = queue.pop_front() {
            let flags = seen[&commit];
            if (flags & best_candidate.identity_bit) == best_candidate.identity_bit {
                if queue
                    .iter()
                    .all(|id| (seen[id] & best_candidate.identity_bit) == best_candidate.identity_bit)
                {
                    break;
                }
            } else {
                best_candidate.commits_in_its_future += 1;
            }

            parents_by_date_onto_queue_and_track_names(
                &mut find,
                &mut buf,
                &mut parent_buf,
                &mut parents,
                &mut queue,
                &mut seen,
                &commit,
                flags,
            )?;
        }
        Ok(())
    }

    #[derive(Debug)]
    struct Candidate<'a> {
        name: Cow<'a, BStr>,
        commits_in_its_future: Flags,
        /// A single bit identifying this candidate uniquely in a bitset
        identity_bit: Flags,
        /// The order at which we found the candidate, first one has order = 0
        order: usize,
    }
}
