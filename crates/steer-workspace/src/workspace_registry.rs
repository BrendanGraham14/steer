use crate::error::WorkspaceManagerError;
use crate::{EnvironmentId, WorkspaceId, WorkspaceInfo, WorkspaceManagerResult, VcsKind};
use sqlx::{Row, SqlitePool};
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub struct WorkspaceRegistry {
    pool: SqlitePool,
}

impl WorkspaceRegistry {
    pub async fn open(root: &Path) -> WorkspaceManagerResult<Self> {
        let registry_dir = root.join(".steer");
        std::fs::create_dir_all(&registry_dir)?;
        let db_path = registry_dir.join("workspaces.sqlite");

        let options = SqliteConnectOptions::new()
            .filename(&db_path)
            .create_if_missing(true);

        let pool = SqlitePool::connect_with(options)
            .await
            .map_err(|e| WorkspaceManagerError::Other(e.to_string()))?;

        let registry = Self { pool };
        registry.init_schema().await?;
        Ok(registry)
    }

    async fn init_schema(&self) -> WorkspaceManagerResult<()> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS workspaces (
                workspace_id TEXT PRIMARY KEY NOT NULL,
                environment_id TEXT NOT NULL,
                parent_workspace_id TEXT NULL,
                name TEXT NULL,
                path TEXT NOT NULL,
                vcs_kind TEXT NULL
            );
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| WorkspaceManagerError::Other(e.to_string()))?;

        sqlx::query(
            r#"
            CREATE INDEX IF NOT EXISTS idx_workspaces_environment
            ON workspaces(environment_id);
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| WorkspaceManagerError::Other(e.to_string()))?;

        Ok(())
    }

    pub async fn insert_workspace(&self, info: &WorkspaceInfo) -> WorkspaceManagerResult<()> {
        sqlx::query(
            r#"
            INSERT INTO workspaces (
                workspace_id,
                environment_id,
                parent_workspace_id,
                name,
                path,
                vcs_kind
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6);
            "#,
        )
        .bind(info.workspace_id.as_uuid().to_string())
        .bind(info.environment_id.as_uuid().to_string())
        .bind(info.parent_workspace_id.map(|id| id.as_uuid().to_string()))
        .bind(info.name.clone())
        .bind(info.path.to_string_lossy().to_string())
        .bind(info.vcs_kind.as_ref().map(VcsKind::as_str))
        .execute(&self.pool)
        .await
        .map_err(|e| WorkspaceManagerError::Other(e.to_string()))?;

        Ok(())
    }

    pub async fn delete_workspace(&self, workspace_id: WorkspaceId) -> WorkspaceManagerResult<()> {
        sqlx::query("DELETE FROM workspaces WHERE workspace_id = ?1")
            .bind(workspace_id.as_uuid().to_string())
            .execute(&self.pool)
            .await
            .map_err(|e| WorkspaceManagerError::Other(e.to_string()))?;
        Ok(())
    }

    pub async fn fetch_workspace(
        &self,
        workspace_id: WorkspaceId,
    ) -> WorkspaceManagerResult<Option<WorkspaceInfo>> {
        let row = sqlx::query(
            r#"
            SELECT workspace_id, environment_id, parent_workspace_id, name, path, vcs_kind
            FROM workspaces
            WHERE workspace_id = ?1;
            "#,
        )
        .bind(workspace_id.as_uuid().to_string())
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| WorkspaceManagerError::Other(e.to_string()))?;

        row.map(|row| self.row_to_workspace_info(row)).transpose()
    }

    pub async fn find_by_path(&self, path: &Path) -> WorkspaceManagerResult<Option<WorkspaceInfo>> {
        let row = sqlx::query(
            r#"
            SELECT workspace_id, environment_id, parent_workspace_id, name, path, vcs_kind
            FROM workspaces
            WHERE path = ?1;
            "#,
        )
        .bind(path.to_string_lossy().to_string())
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| WorkspaceManagerError::Other(e.to_string()))?;

        row.map(|row| self.row_to_workspace_info(row)).transpose()
    }

    pub async fn list_workspaces(
        &self,
        environment_id: EnvironmentId,
    ) -> WorkspaceManagerResult<Vec<WorkspaceInfo>> {
        let query = "SELECT workspace_id, environment_id, parent_workspace_id, name, path, vcs_kind FROM workspaces WHERE environment_id = ?1 ORDER BY name ASC";

        let rows = sqlx::query(query)
            .bind(environment_id.as_uuid().to_string())
            .fetch_all(&self.pool)
            .await
            .map_err(|e| WorkspaceManagerError::Other(e.to_string()))?;

        rows.into_iter()
            .map(|row| self.row_to_workspace_info(row))
            .collect()
    }

    fn row_to_workspace_info(&self, row: sqlx::sqlite::SqliteRow) -> WorkspaceManagerResult<WorkspaceInfo> {
        let workspace_id_str: String = row.get("workspace_id");
        let environment_id_str: String = row.get("environment_id");
        let parent_workspace_id_str: Option<String> = row.get("parent_workspace_id");
        let name: Option<String> = row.get("name");
        let path_str: String = row.get("path");
        let vcs_kind_str: Option<String> = row.get("vcs_kind");

        let workspace_id = WorkspaceId::from_uuid(
            uuid::Uuid::parse_str(&workspace_id_str)
                .map_err(|e| WorkspaceManagerError::Other(format!("Invalid workspace_id: {e}")))?,
        );
        let environment_id =
            EnvironmentId::from_uuid(uuid::Uuid::parse_str(&environment_id_str).map_err(|e| {
                WorkspaceManagerError::Other(format!("Invalid environment_id: {e}"))
            })?);
        let parent_workspace_id = match parent_workspace_id_str {
            Some(value) => Some(WorkspaceId::from_uuid(
                uuid::Uuid::parse_str(&value).map_err(|e| {
                    WorkspaceManagerError::Other(format!("Invalid parent_workspace_id: {e}"))
                })?,
            )),
            None => None,
        };
        let path = PathBuf::from(path_str);
        let vcs_kind = vcs_kind_str.and_then(|value| match value.as_str() {
            "git" => Some(VcsKind::Git),
            "jj" => Some(VcsKind::Jj),
            _ => None,
        });

        Ok(WorkspaceInfo {
            workspace_id,
            environment_id,
            parent_workspace_id,
            name,
            path,
            vcs_kind,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_registry_insert_list_and_delete() {
        let temp = TempDir::new().unwrap();
        let registry = WorkspaceRegistry::open(temp.path()).await.unwrap();

        let environment_id = EnvironmentId::local();
        let workspace_id = WorkspaceId::new();
        let info = WorkspaceInfo {
            workspace_id,
            environment_id,
            parent_workspace_id: None,
            name: Some("alpha".to_string()),
            path: temp.path().join("alpha"),
            vcs_kind: Some(VcsKind::Jj),
        };

        registry.insert_workspace(&info).await.unwrap();

        let fetched = registry.fetch_workspace(workspace_id).await.unwrap();
        let fetched = fetched.expect("expected workspace entry");
        assert_eq!(fetched.workspace_id, workspace_id);
        assert_eq!(fetched.environment_id, environment_id);
        assert_eq!(fetched.name.as_deref(), Some("alpha"));

        let list = registry
            .list_workspaces(environment_id)
            .await
            .unwrap();
        assert_eq!(list.len(), 1);

        registry.delete_workspace(workspace_id).await.unwrap();
        let fetched = registry.fetch_workspace(workspace_id).await.unwrap();
        assert!(fetched.is_none());
    }
}
