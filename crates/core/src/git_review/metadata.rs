use super::*;

pub(super) struct ResolvedReviewTarget {
    pub(super) target: GitReviewTarget,
    pub(super) comparison: SnapshotComparison,
    pub(super) context: GitReviewContext,
}

#[derive(Debug)]
struct RefRecord {
    ref_name: String,
    symref: Option<String>,
}

pub(super) fn load_review_repository_context(
    repository: &RepositoryContext,
) -> Result<GitReviewRepositoryContext, String> {
    let current_branch = read_current_branch(repository)?;
    let head_sha = read_optional_head_oid(repository)?;
    let refs = read_branch_refs(repository)?;
    let default_base_ref = resolve_default_base_ref(repository, current_branch.as_deref(), &refs)?;
    let current_ref = current_branch
        .as_deref()
        .map(|branch| format!("refs/heads/{branch}"));
    let mut branches = refs
        .iter()
        .filter(|record| record.symref.is_none())
        .filter(|record| Some(record.ref_name.as_str()) != current_ref.as_deref())
        .filter_map(|record| branch_from_ref(record, default_base_ref.as_deref()))
        .collect::<Vec<_>>();
    branches.sort_by(|left, right| {
        right
            .is_default
            .cmp(&left.is_default)
            .then_with(|| left.kind_sort_key().cmp(&right.kind_sort_key()))
            .then_with(|| left.name.cmp(&right.name))
    });
    let truncated = branches.len() > MAX_REVIEW_BRANCHES;
    branches.truncate(MAX_REVIEW_BRANCHES);

    Ok(GitReviewRepositoryContext {
        repository_id: repository.repository_id.clone(),
        current_branch,
        head_sha,
        default_base_ref,
        branches,
        truncated,
    })
}

impl GitReviewBranch {
    fn kind_sort_key(&self) -> u8 {
        match self.kind {
            GitReviewBranchKind::Local => 0,
            GitReviewBranchKind::Remote => 1,
        }
    }
}

pub(super) fn load_review_commits(
    repository: &RepositoryContext,
) -> Result<GitReviewCommitList, String> {
    if read_optional_head_oid(repository)?.is_none() {
        return Ok(GitReviewCommitList {
            repository_id: repository.repository_id.clone(),
            commits: Vec::new(),
            truncated: false,
        });
    }

    let args = vec![
        "log".to_string(),
        "--no-show-signature".to_string(),
        "--no-renames".to_string(),
        format!("-n{}", MAX_REVIEW_COMMITS + 1),
        "--format=%x1e%H%x00%P%x00%cI%x00%s%x00%B%x00".to_string(),
        "--shortstat".to_string(),
        "HEAD".to_string(),
        "--".to_string(),
        repository.pathspec.clone(),
    ];
    let output = run_git(&repository.root, &args, MAX_METADATA_BYTES)?;
    if !output.status.success() {
        return Err(git_failure(
            "Unable to read the local Git commit history.",
            &output,
        ));
    }
    if output.stdout_truncated {
        return Err("The local Git commit history is too large to read safely.".to_string());
    }
    let mut commits = parse_commit_log(&output.stdout)?;
    let truncated = commits.len() > MAX_REVIEW_COMMITS;
    commits.truncate(MAX_REVIEW_COMMITS);
    Ok(GitReviewCommitList {
        repository_id: repository.repository_id.clone(),
        commits,
        truncated,
    })
}

pub(super) fn resolve_review_target(
    repository: &RepositoryContext,
    requested: GitReviewTarget,
) -> Result<ResolvedReviewTarget, String> {
    let current_branch = read_current_branch(repository)?;
    let head_sha = read_optional_head_oid(repository)?;
    let mut context = GitReviewContext {
        current_branch,
        head_sha: head_sha.clone(),
        ..GitReviewContext::default()
    };

    match requested {
        GitReviewTarget::LastTurn { .. } => {
            Err("Last-turn review requires an agent turn record.".to_string())
        }
        GitReviewTarget::Unstaged => Ok(ResolvedReviewTarget {
            target: GitReviewTarget::Unstaged,
            comparison: SnapshotComparison::IndexToWorktree,
            context,
        }),
        GitReviewTarget::Staged => {
            let before_oid = baseline_tree_oid(repository, head_sha.as_deref())?;
            Ok(ResolvedReviewTarget {
                target: GitReviewTarget::Staged,
                comparison: SnapshotComparison::TreeToIndex { before_oid },
                context,
            })
        }
        GitReviewTarget::Uncommitted => {
            let before_oid = baseline_tree_oid(repository, head_sha.as_deref())?;
            Ok(ResolvedReviewTarget {
                target: GitReviewTarget::Uncommitted,
                comparison: SnapshotComparison::TreeToWorktree { before_oid },
                context,
            })
        }
        GitReviewTarget::Commit { commit_sha } => {
            if !is_full_object_id(&commit_sha) {
                return Err("The selected commit ID is invalid.".to_string());
            }
            let commit_sha = resolve_commit_oid(repository, &commit_sha)?;
            let before_oid = first_parent_oid(repository, &commit_sha)?
                .unwrap_or(baseline_tree_oid(repository, None)?);
            context.base_sha = Some(before_oid.clone());
            context.commit = Some(read_commit_metadata(repository, &commit_sha)?);
            Ok(ResolvedReviewTarget {
                target: GitReviewTarget::Commit {
                    commit_sha: commit_sha.clone(),
                },
                comparison: SnapshotComparison::TreeToTree {
                    before_oid,
                    after_oid: commit_sha,
                },
                context,
            })
        }
        GitReviewTarget::Branch { base_ref } => {
            validate_branch_ref(repository, &base_ref)?;
            let head_oid = head_sha
                .as_deref()
                .ok_or_else(|| "Branch review requires at least one local commit.".to_string())?;
            let base_sha = resolve_commit_oid(repository, &base_ref)?;
            let merge_base_sha = merge_base_oid(repository, &base_sha, head_oid)?;
            context.base_ref = Some(base_ref.clone());
            context.base_sha = Some(base_sha);
            context.merge_base_sha = Some(merge_base_sha.clone());
            Ok(ResolvedReviewTarget {
                target: GitReviewTarget::Branch { base_ref },
                comparison: SnapshotComparison::TreeToWorktree {
                    before_oid: merge_base_sha,
                },
                context,
            })
        }
    }
}

fn read_current_branch(repository: &RepositoryContext) -> Result<Option<String>, String> {
    let output = run_git(
        &repository.root,
        &[
            "symbolic-ref".to_string(),
            "--quiet".to_string(),
            "--short".to_string(),
            "HEAD".to_string(),
        ],
        16 * 1024,
    )?;
    if !output.status.success() {
        return Ok(None);
    }
    let branch = std::str::from_utf8(trim_ascii(&output.stdout))
        .map_err(|_| "Git returned an invalid current branch name.".to_string())?;
    Ok((!branch.is_empty()).then(|| branch.to_string()))
}

fn read_optional_head_oid(repository: &RepositoryContext) -> Result<Option<String>, String> {
    let oid = read_head_oid(repository)?;
    Ok((!oid.is_empty()).then_some(oid))
}

fn baseline_tree_oid(
    repository: &RepositoryContext,
    head_oid: Option<&str>,
) -> Result<String, String> {
    match head_oid {
        Some(oid) => Ok(oid.to_string()),
        None => empty_tree_oid(repository),
    }
}

fn empty_tree_oid(repository: &RepositoryContext) -> Result<String, String> {
    let output = run_git_with_input(
        &repository.root,
        &[
            "hash-object".to_string(),
            "-t".to_string(),
            "tree".to_string(),
            "--stdin".to_string(),
        ],
        4096,
        Some(b""),
    )?;
    if !output.status.success() {
        return Err(git_failure(
            "Unable to resolve the empty Git tree.",
            &output,
        ));
    }
    parse_single_oid(&output.stdout, "Git returned an invalid empty tree ID.")
}

fn resolve_commit_oid(repository: &RepositoryContext, revision: &str) -> Result<String, String> {
    if revision.is_empty() || revision.len() > 1024 || revision.starts_with('-') {
        return Err("The selected Git revision is invalid.".to_string());
    }
    let output = run_git(
        &repository.root,
        &[
            "rev-parse".to_string(),
            "--verify".to_string(),
            "--end-of-options".to_string(),
            format!("{revision}^{{commit}}"),
        ],
        4096,
    )?;
    if !output.status.success() {
        return Err("The selected Git revision no longer exists.".to_string());
    }
    parse_single_oid(&output.stdout, "Git returned an invalid commit ID.")
}

fn first_parent_oid(
    repository: &RepositoryContext,
    commit_oid: &str,
) -> Result<Option<String>, String> {
    let output = run_git(
        &repository.root,
        &[
            "rev-list".to_string(),
            "--parents".to_string(),
            "-n".to_string(),
            "1".to_string(),
            commit_oid.to_string(),
        ],
        16 * 1024,
    )?;
    if !output.status.success() {
        return Err(git_failure("Unable to resolve the commit parent.", &output));
    }
    let text = std::str::from_utf8(trim_ascii(&output.stdout))
        .map_err(|_| "Git returned an invalid commit parent.".to_string())?;
    let mut oids = text.split_ascii_whitespace();
    let returned_commit = oids
        .next()
        .ok_or_else(|| "Git omitted the selected commit ID.".to_string())?;
    if returned_commit != commit_oid || !is_full_object_id(returned_commit) {
        return Err("Git returned an invalid selected commit ID.".to_string());
    }
    match oids.next() {
        Some(parent) if is_full_object_id(parent) => Ok(Some(parent.to_string())),
        Some(_) => Err("Git returned an invalid commit parent ID.".to_string()),
        None => Ok(None),
    }
}

fn merge_base_oid(
    repository: &RepositoryContext,
    base_oid: &str,
    head_oid: &str,
) -> Result<String, String> {
    let output = run_git(
        &repository.root,
        &[
            "merge-base".to_string(),
            base_oid.to_string(),
            head_oid.to_string(),
        ],
        4096,
    )?;
    if !output.status.success() {
        return Err("The selected branch does not share a merge base with HEAD.".to_string());
    }
    parse_single_oid(&output.stdout, "Git returned an invalid merge-base ID.")
}

fn parse_single_oid(output: &[u8], message: &str) -> Result<String, String> {
    let value = std::str::from_utf8(trim_ascii(output)).map_err(|_| message.to_string())?;
    if !is_full_object_id(value) {
        return Err(message.to_string());
    }
    Ok(value.to_string())
}

fn validate_branch_ref(repository: &RepositoryContext, base_ref: &str) -> Result<(), String> {
    if base_ref.len() > 1024
        || (!base_ref.starts_with("refs/heads/") && !base_ref.starts_with("refs/remotes/"))
        || base_ref.ends_with("/HEAD")
    {
        return Err("The selected branch reference is invalid.".to_string());
    }
    let output = run_git(
        &repository.root,
        &["check-ref-format".to_string(), base_ref.to_string()],
        4096,
    )?;
    if output.status.success() {
        Ok(())
    } else {
        Err("The selected branch reference is invalid.".to_string())
    }
}

fn read_branch_refs(repository: &RepositoryContext) -> Result<Vec<RefRecord>, String> {
    let output = run_git(
        &repository.root,
        &[
            "for-each-ref".to_string(),
            "--format=%(refname)%00%(symref)%00".to_string(),
            "refs/heads".to_string(),
            "refs/remotes".to_string(),
        ],
        MAX_METADATA_BYTES,
    )?;
    if !output.status.success() {
        return Err(git_failure("Unable to list local Git branches.", &output));
    }
    if output.stdout_truncated {
        return Err("The local Git branch list is too large to read safely.".to_string());
    }

    output
        .stdout
        .split(|byte| *byte == b'\n')
        .filter(|record| !trim_ascii(record).is_empty())
        .map(|record| {
            let fields = record.split(|byte| *byte == 0).collect::<Vec<_>>();
            if fields.len() < 2 {
                return Err("Git returned an invalid branch entry.".to_string());
            }
            let ref_name = std::str::from_utf8(fields[0])
                .map_err(|_| "Git returned a non-UTF-8 branch name.".to_string())?;
            let symref = std::str::from_utf8(fields[1])
                .map_err(|_| "Git returned a non-UTF-8 symbolic branch.".to_string())?;
            Ok(RefRecord {
                ref_name: ref_name.to_string(),
                symref: (!symref.is_empty()).then(|| symref.to_string()),
            })
        })
        .collect()
}

fn resolve_default_base_ref(
    repository: &RepositoryContext,
    current_branch: Option<&str>,
    refs: &[RefRecord],
) -> Result<Option<String>, String> {
    let configured_remote = match current_branch {
        Some(branch) => read_branch_remote(repository, branch)?,
        None => None,
    };
    let mut preferred_remote_heads = Vec::new();
    if let Some(remote) = configured_remote.filter(|remote| remote != ".") {
        preferred_remote_heads.push(format!("refs/remotes/{remote}/HEAD"));
    }
    preferred_remote_heads.push("refs/remotes/origin/HEAD".to_string());

    for preferred in preferred_remote_heads {
        if let Some(target) = refs
            .iter()
            .find(|record| record.ref_name == preferred)
            .and_then(|record| record.symref.as_deref())
        {
            return Ok(Some(target.to_string()));
        }
    }
    if let Some(target) = refs.iter().find_map(|record| {
        (record.ref_name.starts_with("refs/remotes/") && record.ref_name.ends_with("/HEAD"))
            .then_some(record.symref.as_deref())
            .flatten()
    }) {
        return Ok(Some(target.to_string()));
    }

    let current_ref = current_branch.map(|branch| format!("refs/heads/{branch}"));
    for candidate in ["refs/heads/main", "refs/heads/master", "refs/heads/trunk"] {
        if Some(candidate) == current_ref.as_deref() {
            continue;
        }
        if refs.iter().any(|record| record.ref_name == candidate) {
            return Ok(Some(candidate.to_string()));
        }
    }
    Ok(None)
}

fn read_branch_remote(
    repository: &RepositoryContext,
    current_branch: &str,
) -> Result<Option<String>, String> {
    let output = run_git(
        &repository.root,
        &[
            "config".to_string(),
            "--get".to_string(),
            format!("branch.{current_branch}.remote"),
        ],
        16 * 1024,
    )?;
    if !output.status.success() {
        return Ok(None);
    }
    let value = std::str::from_utf8(trim_ascii(&output.stdout))
        .map_err(|_| "Git returned an invalid branch remote.".to_string())?;
    Ok((!value.is_empty()).then(|| value.to_string()))
}

fn branch_from_ref(record: &RefRecord, default_base_ref: Option<&str>) -> Option<GitReviewBranch> {
    let (name, kind) = if let Some(name) = record.ref_name.strip_prefix("refs/heads/") {
        (name, GitReviewBranchKind::Local)
    } else if let Some(name) = record.ref_name.strip_prefix("refs/remotes/") {
        (name, GitReviewBranchKind::Remote)
    } else {
        return None;
    };
    Some(GitReviewBranch {
        name: name.to_string(),
        ref_name: record.ref_name.clone(),
        kind,
        is_default: Some(record.ref_name.as_str()) == default_base_ref,
    })
}

fn read_commit_metadata(
    repository: &RepositoryContext,
    commit_oid: &str,
) -> Result<GitReviewCommit, String> {
    let output = run_git(
        &repository.root,
        &[
            "show".to_string(),
            "--no-show-signature".to_string(),
            "--no-renames".to_string(),
            "--format=%x1e%H%x00%P%x00%cI%x00%s%x00%B%x00".to_string(),
            "--shortstat".to_string(),
            commit_oid.to_string(),
            "--".to_string(),
            repository.pathspec.clone(),
        ],
        MAX_METADATA_BYTES,
    )?;
    if !output.status.success() || output.stdout_truncated {
        return Err(git_failure(
            "Unable to read the selected commit metadata.",
            &output,
        ));
    }
    parse_commit_log(&output.stdout)?
        .into_iter()
        .next()
        .ok_or_else(|| "Git omitted the selected commit metadata.".to_string())
}

fn parse_commit_log(output: &[u8]) -> Result<Vec<GitReviewCommit>, String> {
    output
        .split(|byte| *byte == 0x1e)
        .filter(|record| !trim_ascii(record).is_empty())
        .map(parse_commit_record)
        .collect()
}

fn parse_commit_record(record: &[u8]) -> Result<GitReviewCommit, String> {
    let record = record.strip_prefix(b"\n").unwrap_or(record);
    let fields = record.splitn(6, |byte| *byte == 0).collect::<Vec<_>>();
    if fields.len() != 6 {
        return Err("Git returned invalid commit metadata.".to_string());
    }
    let text = |value: &[u8]| {
        std::str::from_utf8(value)
            .map(str::to_string)
            .map_err(|_| "Git returned non-UTF-8 commit metadata.".to_string())
    };
    let sha = text(fields[0])?;
    if !is_full_object_id(&sha) {
        return Err("Git returned an invalid commit ID.".to_string());
    }
    let parents = text(fields[1])?
        .split_ascii_whitespace()
        .map(|parent| {
            if is_full_object_id(parent) {
                Ok(parent.to_string())
            } else {
                Err("Git returned an invalid commit parent ID.".to_string())
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    let committed_at = text(fields[2])?;
    let subject = text(fields[3])?;
    let message = text(fields[4])?.trim_end_matches('\n').to_string();
    let stats_text = std::str::from_utf8(fields[5])
        .map_err(|_| "Git returned invalid commit statistics.".to_string())?;
    Ok(GitReviewCommit {
        sha,
        parents,
        subject,
        message,
        committed_at,
        stats: parse_shortstat(stats_text),
    })
}

fn parse_shortstat(value: &str) -> GitReviewStats {
    let file_count = count_before_any(value, &[" file changed", " files changed"]);
    let additions = count_before_any(value, &[" insertion(+)", " insertions(+)"]);
    let deletions = count_before_any(value, &[" deletion(-)", " deletions(-)"]);
    GitReviewStats {
        file_count: file_count.unwrap_or_default() as usize,
        additions: additions.unwrap_or_default(),
        deletions: deletions.unwrap_or_default(),
        line_counts_complete: file_count.is_some(),
    }
}

fn count_before_any(value: &str, suffixes: &[&str]) -> Option<u64> {
    suffixes.iter().find_map(|suffix| {
        let index = value.find(suffix)?;
        value[..index]
            .split(|character: char| !character.is_ascii_digit())
            .next_back()
            .filter(|digits| !digits.is_empty())?
            .parse()
            .ok()
    })
}
