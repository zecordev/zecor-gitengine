// SPDX-License-Identifier: Apache-2.0
//! In-process git for the checks an autonomous coding loop makes thousands of times a
//! night: resolve a rev, find a merge base, and ask "where does this branch stand
//! relative to its base" -- ahead/behind counts, is-merged, is-descendant -- as one
//! call, without a `git` fork+exec per question.
//!
//! Backed by `gitoxide` (`gix`): pure Rust, thread-safe, no C dependency. Merge-base
//! and ahead/behind are computed here from revision walks so the surface stays stable
//! across `gix` releases.

use anyhow::{Context, Result};
use gix::bstr::ByteSlice;
use serde::Serialize;
use std::collections::{BTreeMap, HashSet};
use std::path::Path;

#[derive(Debug, Clone, Serialize)]
pub struct Head {
    pub sha: String,
    pub reference: Option<String>,
    pub detached: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct BranchState {
    pub branch: String,
    pub base: String,
    pub branch_sha: String,
    pub base_sha: String,
    pub merge_base: Option<String>,
    /// commits on `branch` not reachable from `base`
    pub ahead: usize,
    /// commits on `base` not reachable from `branch`
    pub behind: usize,
    /// every commit of `branch` is reachable from `base`
    pub is_merged: bool,
    /// `branch` is strictly ahead of `base` with nothing to rebase over
    pub is_descendant: bool,
}

fn open(repo_dir: &Path) -> Result<gix::Repository> {
    gix::open(repo_dir).with_context(|| format!("open git repo at {}", repo_dir.display()))
}

fn resolve(repo: &gix::Repository, rev: &str) -> Result<gix::ObjectId> {
    Ok(repo
        .rev_parse_single(rev)
        .with_context(|| format!("rev-parse {rev:?}"))?
        .detach())
}

/// Every commit reachable from `tip`, oldest-last order not guaranteed.
fn ancestors(repo: &gix::Repository, tip: gix::ObjectId) -> Result<HashSet<gix::ObjectId>> {
    let mut set = HashSet::new();
    for step in repo.rev_walk([tip]).all()? {
        set.insert(step?.id);
    }
    Ok(set)
}

pub fn rev_parse(repo_dir: &Path, rev: &str) -> Result<String> {
    Ok(resolve(&open(repo_dir)?, rev)?.to_string())
}

pub fn head(repo_dir: &Path) -> Result<Head> {
    let repo = open(repo_dir)?;
    let head = repo.head().context("read HEAD")?;
    let detached = head.is_detached();
    let reference = head
        .referent_name()
        .map(|n| n.as_bstr().to_str_lossy().into_owned());
    let sha = repo
        .head_id()
        .context("HEAD has no commit")?
        .detach()
        .to_string();
    Ok(Head {
        sha,
        reference,
        detached,
    })
}

/// The best common ancestor of `a` and `b`: the ancestor of `a` closest to `a` (by
/// walk order) that is also an ancestor of `b`.
fn merge_base_id(
    repo: &gix::Repository,
    a: gix::ObjectId,
    b: gix::ObjectId,
) -> Result<Option<gix::ObjectId>> {
    let b_anc = ancestors(repo, b)?;
    for step in repo.rev_walk([a]).all()? {
        let id = step?.id;
        if b_anc.contains(&id) {
            return Ok(Some(id));
        }
    }
    Ok(None)
}

pub fn merge_base(repo_dir: &Path, a: &str, b: &str) -> Result<Option<String>> {
    let repo = open(repo_dir)?;
    let ida = resolve(&repo, a)?;
    let idb = resolve(&repo, b)?;
    Ok(merge_base_id(&repo, ida, idb)?.map(|id| id.to_string()))
}

fn count_ahead(
    repo: &gix::Repository,
    from: gix::ObjectId,
    exclude_anc: &HashSet<gix::ObjectId>,
) -> Result<usize> {
    let mut n = 0usize;
    for step in repo.rev_walk([from]).all()? {
        if !exclude_anc.contains(&step?.id) {
            n += 1;
        }
    }
    Ok(n)
}

pub fn branch_state(repo_dir: &Path, branch: &str, base: &str) -> Result<BranchState> {
    let repo = open(repo_dir)?;
    let branch_id = resolve(&repo, branch)?;
    let base_id = resolve(&repo, base)?;
    let branch_anc = ancestors(&repo, branch_id)?;
    let base_anc = ancestors(&repo, base_id)?;
    let mb = merge_base_id(&repo, branch_id, base_id)?;

    let ahead = count_ahead(&repo, branch_id, &base_anc)?;
    let behind = count_ahead(&repo, base_id, &branch_anc)?;

    Ok(BranchState {
        branch: branch.to_string(),
        base: base.to_string(),
        branch_sha: branch_id.to_string(),
        base_sha: base_id.to_string(),
        merge_base: mb.map(|m| m.to_string()),
        ahead,
        behind,
        is_merged: ahead == 0,
        is_descendant: mb == Some(base_id) && behind == 0 && ahead > 0,
    })
}

fn tree_blobs(repo: &gix::Repository, rev: &str) -> Result<BTreeMap<String, gix::ObjectId>> {
    let tree = repo
        .rev_parse_single(rev)?
        .object()?
        .peel_to_tree()
        .with_context(|| format!("{rev:?} is not a tree-ish"))?;
    let mut out = BTreeMap::new();
    let mut recorder = gix::traverse::tree::Recorder::default();
    tree.traverse().breadthfirst(&mut recorder)?;
    for entry in recorder.records {
        if entry.mode.is_blob() {
            out.insert(entry.filepath.to_str_lossy().into_owned(), entry.oid);
        }
    }
    Ok(out)
}

#[derive(Debug, Clone, Serialize)]
pub struct LogEntry {
    pub sha: String,
    pub summary: String,
    pub author: String,
    pub email: String,
    /// commit time, seconds since the epoch
    pub time: i64,
    pub parents: Vec<String>,
}

/// The most recent `limit` commits reachable from `rev`, newest first.
pub fn log(repo_dir: &Path, rev: &str, limit: usize) -> Result<Vec<LogEntry>> {
    let repo = open(repo_dir)?;
    let tip = resolve(&repo, rev)?;
    let mut out = Vec::new();
    for step in repo.rev_walk([tip]).all()? {
        if out.len() >= limit {
            break;
        }
        let id = step?.id;
        let commit = repo.find_object(id)?.try_into_commit()?;
        let msg = commit.message_raw_sloppy();
        let summary = msg
            .to_str_lossy()
            .lines()
            .next()
            .unwrap_or("")
            .trim()
            .to_string();
        let author = commit.author()?;
        out.push(LogEntry {
            sha: id.to_string(),
            summary,
            author: author.name.to_str_lossy().into_owned(),
            email: author.email.to_str_lossy().into_owned(),
            time: author.time.seconds,
            parents: commit.parent_ids().map(|p| p.to_string()).collect(),
        });
    }
    Ok(out)
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct Status {
    /// paths changed in the index relative to HEAD
    pub staged: Vec<StatusEntry>,
    /// paths changed in the working tree relative to the index
    pub unstaged: Vec<StatusEntry>,
    /// paths present on disk but not tracked and not ignored
    pub untracked: Vec<String>,
    pub is_clean: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct StatusEntry {
    pub path: String,
    pub change: String, // added | modified | deleted | type-change | renamed | conflict
}

/// Working-tree status: staged (HEAD vs index), unstaged (index vs worktree), untracked.
pub fn status(repo_dir: &Path) -> Result<Status> {
    let repo = open(repo_dir)?;

    // staged: compare the HEAD tree to the index by blob id
    let head_blobs = tree_blobs(&repo, "HEAD").unwrap_or_default();
    let index = repo.open_index()?;
    let mut idx_blobs: BTreeMap<String, gix::ObjectId> = BTreeMap::new();
    for entry in index.entries() {
        let path = entry.path(&index).to_str_lossy().into_owned();
        idx_blobs.insert(path, entry.id);
    }
    let mut staged = Vec::new();
    for (path, oid) in &idx_blobs {
        match head_blobs.get(path) {
            None => staged.push(StatusEntry {
                path: path.clone(),
                change: "added".into(),
            }),
            Some(h) if h != oid => staged.push(StatusEntry {
                path: path.clone(),
                change: "modified".into(),
            }),
            Some(_) => {}
        }
    }
    for path in head_blobs.keys() {
        if !idx_blobs.contains_key(path) {
            staged.push(StatusEntry {
                path: path.clone(),
                change: "deleted".into(),
            });
        }
    }
    staged.sort_by(|a, b| a.path.cmp(&b.path));

    // unstaged + untracked: the index-vs-worktree walk
    let mut unstaged = Vec::new();
    let mut untracked = Vec::new();
    let iter = repo
        .status(gix::progress::Discard)?
        .into_index_worktree_iter(Vec::new())?;
    for item in iter {
        let item = item?;
        let path = item.rela_path().to_str_lossy().into_owned();
        use gix::status::index_worktree::iter::Summary::*;
        match item.summary() {
            Some(Added) => untracked.push(path),
            Some(Removed) => unstaged.push(StatusEntry {
                path,
                change: "deleted".into(),
            }),
            Some(Modified | IntentToAdd) => unstaged.push(StatusEntry {
                path,
                change: "modified".into(),
            }),
            Some(TypeChange) => unstaged.push(StatusEntry {
                path,
                change: "type-change".into(),
            }),
            Some(Conflict) => unstaged.push(StatusEntry {
                path,
                change: "conflict".into(),
            }),
            Some(Renamed | Copied) => unstaged.push(StatusEntry {
                path,
                change: "renamed".into(),
            }),
            None => {}
        }
    }
    unstaged.sort_by(|a, b| a.path.cmp(&b.path));
    untracked.sort();

    let is_clean = staged.is_empty() && unstaged.is_empty() && untracked.is_empty();
    Ok(Status {
        staged,
        unstaged,
        untracked,
        is_clean,
    })
}

/// Files that differ between two tree-ish revs (added, modified, or deleted).
pub fn changed_files(repo_dir: &Path, a: &str, b: &str) -> Result<Vec<String>> {
    let repo = open(repo_dir)?;
    let ba = tree_blobs(&repo, a)?;
    let bb = tree_blobs(&repo, b)?;
    let mut out: Vec<String> = Vec::new();
    for (path, oid) in &bb {
        if ba.get(path) != Some(oid) {
            out.push(path.clone());
        }
    }
    for path in ba.keys() {
        if !bb.contains_key(path) {
            out.push(path.clone());
        }
    }
    out.sort();
    out.dedup();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use tempfile::TempDir;

    fn git(dir: &Path, args: &[&str]) {
        let ok = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?}");
    }

    fn repo() -> TempDir {
        let d = TempDir::new().unwrap();
        let p = d.path();
        git(p, &["init", "-q", "-b", "main"]);
        std::fs::write(p.join("a.txt"), "one\n").unwrap();
        git(p, &["add", "."]);
        git(p, &["commit", "-qm", "c1"]);
        git(p, &["checkout", "-q", "-b", "feature"]);
        std::fs::write(p.join("b.txt"), "two\n").unwrap();
        git(p, &["add", "."]);
        git(p, &["commit", "-qm", "c2"]);
        d
    }

    #[test]
    fn head_and_rev_parse() {
        let d = repo();
        let h = head(d.path()).unwrap();
        assert!(!h.detached && h.reference.as_deref() == Some("refs/heads/feature"));
        assert_eq!(rev_parse(d.path(), "HEAD").unwrap(), h.sha);
    }

    #[test]
    fn branch_state_reports_ahead_and_unmerged() {
        let d = repo();
        let s = branch_state(d.path(), "feature", "main").unwrap();
        assert_eq!((s.ahead, s.behind), (1, 0));
        assert!(!s.is_merged && s.is_descendant);
        assert_eq!(s.merge_base.as_deref(), Some(s.base_sha.as_str()));
    }

    #[test]
    fn merged_branch_is_zero_ahead() {
        let d = repo();
        git(d.path(), &["checkout", "-q", "main"]);
        git(d.path(), &["merge", "-q", "--no-ff", "feature", "-m", "m"]);
        let s = branch_state(d.path(), "feature", "main").unwrap();
        assert_eq!(s.ahead, 0);
        assert!(s.is_merged);
    }

    #[test]
    fn changed_files_between_revs() {
        let d = repo();
        assert_eq!(
            changed_files(d.path(), "main", "feature").unwrap(),
            vec!["b.txt".to_string()]
        );
    }

    #[test]
    fn log_is_newest_first_and_capped() {
        let d = repo();
        let l = log(d.path(), "HEAD", 10).unwrap();
        assert_eq!(l.len(), 2);
        assert_eq!(l[0].summary, "c2");
        assert_eq!(l[1].summary, "c1");
        assert_eq!(l[0].author, "t");
        assert_eq!(l[0].parents, vec![l[1].sha.clone()]);
        assert_eq!(log(d.path(), "HEAD", 1).unwrap().len(), 1);
    }

    #[test]
    fn status_sees_staged_unstaged_and_untracked() {
        let d = repo();
        let p = d.path();
        assert!(status(p).unwrap().is_clean);

        std::fs::write(p.join("a.txt"), "one\nmore\n").unwrap(); // tracked, modified, unstaged
        std::fs::write(p.join("new.txt"), "x\n").unwrap(); // untracked
        let s = status(p).unwrap();
        assert!(!s.is_clean);
        assert_eq!(s.untracked, vec!["new.txt".to_string()]);
        assert!(s
            .unstaged
            .iter()
            .any(|e| e.path == "a.txt" && e.change == "modified"));

        git(p, &["add", "new.txt"]);
        let s = status(p).unwrap();
        assert!(s
            .staged
            .iter()
            .any(|e| e.path == "new.txt" && e.change == "added"));
    }
}
