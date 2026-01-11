use std::path::{Path, PathBuf};

use crate::{RepoId, WorkspaceId};

#[derive(Debug, Clone)]
pub(crate) struct WorkspaceLayout {
    root: PathBuf,
}

impl WorkspaceLayout {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub(crate) fn workspace_parent_dir(&self, repo_id: RepoId) -> PathBuf {
        self.root
            .join(".steer")
            .join("workspaces")
            .join(repo_id.as_uuid().to_string())
    }

    pub(crate) fn sanitize_name(&self, name: &str) -> String {
        let mut sanitized = String::new();
        for ch in name.chars() {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                sanitized.push(ch);
            } else if ch.is_ascii_whitespace() {
                sanitized.push('-');
            }
        }
        if sanitized.is_empty() {
            "workspace".to_string()
        } else {
            sanitized
        }
    }

    pub(crate) fn default_workspace_name(&self, workspace_id: WorkspaceId) -> String {
        let id = workspace_id.as_uuid().to_string();
        let short = id.split('-').next().unwrap_or("workspace");
        format!("ws-{short}")
    }

    pub(crate) fn ensure_unique_path(&self, base_dir: &Path, name: &str) -> PathBuf {
        let mut candidate = base_dir.join(name);
        if !candidate.exists() {
            return candidate;
        }
        let mut counter = 1;
        loop {
            let candidate_name = format!("{name}-{counter}");
            candidate = base_dir.join(candidate_name);
            if !candidate.exists() {
                return candidate;
            }
            counter += 1;
        }
    }
}
