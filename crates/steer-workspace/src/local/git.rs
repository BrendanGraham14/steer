use std::path::{Component, Path, PathBuf};
use std::sync::atomic::AtomicBool;

use crate::error::{WorkspaceManagerError, WorkspaceManagerResult};
use crate::{RepoId, WorkspaceId};
use uuid::Uuid;

use super::layout::WorkspaceLayout;

#[derive(Debug, Clone)]
pub(crate) struct GitWorktreeNames {
    pub(crate) branch_name: String,
    pub(crate) worktree_name: String,
}

pub(crate) fn worktree_names(
    workspace_id: WorkspaceId,
    sanitized_name: &str,
    workspace_path: &Path,
) -> GitWorktreeNames {
    let workspace_slug = workspace_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(sanitized_name)
        .to_string();
    let short = workspace_id
        .as_uuid()
        .to_string()
        .split('-')
        .next()
        .unwrap_or("workspace")
        .to_string();
    let branch_name = format!("steer/{workspace_slug}-{short}");
    let worktree_name = WorkspaceLayout::sanitize_name(&format!("steer-{workspace_slug}-{short}"));
    GitWorktreeNames {
        branch_name,
        worktree_name,
    }
}

pub(crate) fn repo_id_for_path(path: &Path) -> WorkspaceManagerResult<RepoId> {
    let repo = gix::discover(path)
        .map_err(|e| WorkspaceManagerError::Other(format!("Failed to open git repo: {e}")))?;
    let canonical = std::fs::canonicalize(repo.common_dir())
        .unwrap_or_else(|_| repo.common_dir().to_path_buf());
    let uuid = Uuid::new_v5(&Uuid::NAMESPACE_URL, canonical.to_string_lossy().as_bytes());
    Ok(RepoId::from_uuid(uuid))
}

pub(crate) fn create_worktree(
    repo_root: &Path,
    worktree_path: &Path,
    worktree_name: &str,
    branch_name: &str,
) -> WorkspaceManagerResult<()> {
    let mut repo = gix::discover(repo_root)
        .map_err(|e| WorkspaceManagerError::Other(format!("Failed to open git repo: {e}")))?;

    let mut head = repo
        .head()
        .map_err(|e| WorkspaceManagerError::Other(format!("Failed to read git HEAD: {e}")))?;
    let head_id = head
        .try_peel_to_id_in_place()
        .map_err(|e| WorkspaceManagerError::Other(format!("Failed to resolve git HEAD: {e}")))?;
    let head_id = head_id.ok_or_else(|| {
        WorkspaceManagerError::NotSupported(
            "Git worktree creation requires at least one commit".to_string(),
        )
    })?;
    let head_oid = head_id.detach();
    let root_tree = repo
        .find_object(head_oid)
        .map_err(|e| WorkspaceManagerError::Other(format!("Failed to read HEAD object: {e}")))?
        .peel_to_tree()
        .map_err(|e| WorkspaceManagerError::Other(format!("Failed to peel HEAD to tree: {e}")))?
        .id;
    drop(head);

    let committer = repo.committer_or_set_generic_fallback().map_err(|e| {
        WorkspaceManagerError::Other(format!("Failed to configure git committer identity: {e}"))
    })?;
    let head_log_signature = gix::actor::Signature::from(committer);

    let branch_ref = format!("refs/heads/{branch_name}");
    let mut guard =
        WorktreeCreateGuard::new(&repo, branch_ref.clone(), worktree_path.to_path_buf());

    repo.reference(
        branch_ref.as_str(),
        head_oid,
        gix::refs::transaction::PreviousValue::MustNotExist,
        "steer worktree",
    )
    .map_err(|e| {
        WorkspaceManagerError::Other(format!("Failed to create git branch {branch_ref}: {e}"))
    })?;
    guard.mark_branch_created();

    std::fs::create_dir_all(worktree_path)?;
    guard.mark_worktree_created();
    let canonical_worktree_path =
        std::fs::canonicalize(worktree_path).unwrap_or_else(|_| worktree_path.to_path_buf());

    let common_dir = repo.common_dir().to_path_buf();
    let worktrees_root = common_dir.join("worktrees");
    std::fs::create_dir_all(&worktrees_root)?;
    let worktree_git_dir = WorkspaceLayout::ensure_unique_path(&worktrees_root, worktree_name);
    std::fs::create_dir_all(&worktree_git_dir)?;
    std::fs::create_dir_all(worktree_git_dir.join("refs"))?;
    guard.mark_git_dir_created(worktree_git_dir.clone());

    let gitfile_path = canonical_worktree_path.join(".git");
    std::fs::write(
        &gitfile_path,
        format!("gitdir: {}\n", worktree_git_dir.display()),
    )?;
    std::fs::write(
        worktree_git_dir.join("commondir"),
        format!("{}\n", common_dir.display()),
    )?;
    std::fs::write(
        worktree_git_dir.join("gitdir"),
        format!("{}\n", gitfile_path.display()),
    )?;
    std::fs::write(
        worktree_git_dir.join("HEAD"),
        format!("ref: {branch_ref}\n"),
    )?;
    std::fs::write(worktree_git_dir.join("ORIG_HEAD"), format!("{head_oid}\n"))?;
    let logs_dir = worktree_git_dir.join("logs");
    std::fs::create_dir_all(&logs_dir)?;
    let mut head_log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(logs_dir.join("HEAD"))?;
    let head_log_line = gix::refs::log::Line {
        previous_oid: gix::ObjectId::null(repo.object_hash()),
        new_oid: head_oid,
        signature: head_log_signature,
        message: format!("checkout: moving from (initial) to {branch_name}").into(),
    };
    head_log_line.write_to(&mut head_log)?;

    let worktree_repo = gix::open_opts(&canonical_worktree_path, repo.open_options().clone())
        .map_err(|e| {
            WorkspaceManagerError::Other(format!("Failed to open new git worktree: {e}"))
        })?;
    let mut index = worktree_repo.index_from_tree(&root_tree).map_err(|e| {
        WorkspaceManagerError::Other(format!("Failed to build index for git worktree: {e}"))
    })?;
    let mut opts = worktree_repo
        .checkout_options(gix::worktree::stack::state::attributes::Source::IdMapping)
        .map_err(|e| {
            WorkspaceManagerError::Other(format!("Failed to configure git checkout options: {e}"))
        })?;
    opts.destination_is_initially_empty = true;

    let files = gix::features::progress::Discard;
    let bytes = gix::features::progress::Discard;
    let should_interrupt = AtomicBool::new(false);

    let outcome = gix_worktree_state::checkout(
        &mut index,
        &canonical_worktree_path,
        worktree_repo.objects.clone().into_arc().map_err(|e| {
            WorkspaceManagerError::Other(format!("Failed to access git object database: {e}"))
        })?,
        &files,
        &bytes,
        &should_interrupt,
        opts,
    )
    .map_err(|e| WorkspaceManagerError::Other(format!("Failed to checkout git worktree: {e}")))?;

    if !outcome.collisions.is_empty()
        || !outcome.errors.is_empty()
        || !outcome.delayed_paths_unknown.is_empty()
        || !outcome.delayed_paths_unprocessed.is_empty()
    {
        return Err(WorkspaceManagerError::Other(format!(
            "Git worktree checkout incomplete (collisions: {}, errors: {}, delayed_unknown: {}, delayed_unprocessed: {})",
            outcome.collisions.len(),
            outcome.errors.len(),
            outcome.delayed_paths_unknown.len(),
            outcome.delayed_paths_unprocessed.len()
        )));
    }

    index
        .write(Default::default())
        .map_err(|e| WorkspaceManagerError::Other(format!("Failed to write git index: {e}")))?;

    guard.success();
    Ok(())
}

pub(crate) fn remove_worktree(
    repo_root: &Path,
    worktree_path: &Path,
) -> WorkspaceManagerResult<()> {
    let repo = gix::discover(repo_root)
        .map_err(|e| WorkspaceManagerError::Other(format!("Failed to open git repo: {e}")))?;

    let common_dir = repo.common_dir().to_path_buf();
    let worktrees_root = common_dir.join("worktrees");
    let target = normalize_path_no_symlinks(worktree_path);
    if let Some(main_workdir) = repo.workdir()
        && normalize_path_no_symlinks(main_workdir) == target
    {
        return Err(WorkspaceManagerError::InvalidRequest(
            "Refusing to delete the main git worktree".to_string(),
        ));
    }

    let mut private_git_dir = None;
    let gitfile_path = worktree_path.join(".git");
    if gitfile_path.is_file()
        && let Ok(path) = gix::discover::path::from_gitdir_file(&gitfile_path)
    {
        private_git_dir = Some(path);
    }
    if private_git_dir.is_none() {
        private_git_dir = find_worktree_git_dir(&worktrees_root, &target, &gitfile_path);
    }

    if private_git_dir.is_none() && !worktree_path.exists() {
        return Ok(());
    }
    let private_git_dir = private_git_dir.ok_or_else(|| {
        WorkspaceManagerError::NotFound(
            "Git worktree metadata not found for requested path".to_string(),
        )
    })?;

    let worktrees_root = std::fs::canonicalize(&worktrees_root)
        .unwrap_or_else(|_| normalize_path_no_symlinks(&worktrees_root));
    let private_git_dir = std::fs::canonicalize(&private_git_dir)
        .unwrap_or_else(|_| normalize_path_no_symlinks(&private_git_dir));
    if !private_git_dir.starts_with(&worktrees_root) {
        return Err(WorkspaceManagerError::InvalidRequest(
            "Git worktree metadata is outside of the repository worktrees directory".to_string(),
        ));
    }

    if worktree_path.exists() {
        std::fs::remove_dir_all(worktree_path)?;
    }
    if private_git_dir.exists() {
        std::fs::remove_dir_all(private_git_dir)?;
    }

    Ok(())
}

fn find_worktree_git_dir(
    worktrees_root: &Path,
    target_base: &Path,
    target_gitfile: &Path,
) -> Option<PathBuf> {
    let entries = std::fs::read_dir(worktrees_root).ok()?;
    for entry in entries.flatten() {
        let candidate = entry.path();
        let gitdir_file = candidate.join("gitdir");
        if !gitdir_file.is_file() {
            continue;
        }
        let path = match gix::discover::path::from_plain_file(&gitdir_file).transpose() {
            Ok(Some(path)) => path,
            _ => continue,
        };
        let resolved = if path.is_relative() {
            candidate.join(path)
        } else {
            path
        };
        if paths_match(&resolved, target_gitfile) {
            return Some(candidate);
        }
        let base = gix::discover::path::without_dot_git_dir(resolved);
        if paths_match(&base, target_base) {
            return Some(candidate);
        }
    }
    None
}

fn paths_match(left: &Path, right: &Path) -> bool {
    let left_candidates = normalized_candidates(left);
    let right_candidates = normalized_candidates(right);
    left_candidates
        .iter()
        .any(|l| right_candidates.iter().any(|r| l == r))
}

fn normalized_candidates(path: &Path) -> Vec<PathBuf> {
    let mut candidates = vec![normalize_path_no_symlinks(path)];
    if let Ok(canonical) = std::fs::canonicalize(path) {
        let canonical_norm = normalize_path_no_symlinks(&canonical);
        if !candidates.contains(&canonical_norm) {
            candidates.push(canonical_norm);
        }
    }
    candidates
}

fn normalize_path_no_symlinks(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(Path::new(std::path::MAIN_SEPARATOR_STR)),
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(part) => normalized.push(part),
        }
    }
    normalized
}

struct WorktreeCreateGuard<'repo> {
    repo: &'repo gix::Repository,
    branch_ref: String,
    worktree_path: PathBuf,
    created_branch: bool,
    created_worktree: bool,
    created_git_dir: Option<PathBuf>,
    committed: bool,
}

impl<'repo> WorktreeCreateGuard<'repo> {
    fn new(repo: &'repo gix::Repository, branch_ref: String, worktree_path: PathBuf) -> Self {
        Self {
            repo,
            branch_ref,
            worktree_path,
            created_branch: false,
            created_worktree: false,
            created_git_dir: None,
            committed: false,
        }
    }

    fn mark_branch_created(&mut self) {
        self.created_branch = true;
    }

    fn mark_worktree_created(&mut self) {
        self.created_worktree = true;
    }

    fn mark_git_dir_created(&mut self, path: PathBuf) {
        self.created_git_dir = Some(path);
    }

    fn success(&mut self) {
        self.committed = true;
    }
}

impl Drop for WorktreeCreateGuard<'_> {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        if let Some(worktree_git_dir) = self.created_git_dir.take() {
            let _ = std::fs::remove_dir_all(worktree_git_dir);
        }
        if self.created_worktree && self.worktree_path.exists() {
            let _ = std::fs::remove_dir_all(&self.worktree_path);
        }
        if self.created_branch
            && let Ok(reference) = self.repo.find_reference(&self.branch_ref)
        {
            let _ = reference.delete();
        }
    }
}
