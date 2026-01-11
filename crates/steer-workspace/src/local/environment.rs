use async_trait::async_trait;
use std::path::PathBuf;

use crate::error::EnvironmentManagerResult;
use crate::manager::{
    CreateEnvironmentRequest, EnvironmentDeletePolicy, EnvironmentDescriptor, EnvironmentManager,
};
use crate::{EnvironmentId, EnvironmentInfo};

/// Local environment manager (single implicit environment).
#[derive(Debug, Clone)]
pub struct LocalEnvironmentManager {
    root: PathBuf,
    environment_id: EnvironmentId,
}

impl LocalEnvironmentManager {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            environment_id: EnvironmentId::local(),
        }
    }

    pub fn environment_id(&self) -> EnvironmentId {
        self.environment_id
    }
}

#[async_trait]
impl EnvironmentManager for LocalEnvironmentManager {
    async fn create_environment(
        &self,
        _request: CreateEnvironmentRequest,
    ) -> EnvironmentManagerResult<EnvironmentDescriptor> {
        Ok(EnvironmentDescriptor {
            environment_id: self.environment_id,
            root: self.root.clone(),
        })
    }

    async fn get_environment(
        &self,
        environment_id: EnvironmentId,
    ) -> EnvironmentManagerResult<EnvironmentDescriptor> {
        if environment_id != self.environment_id {
            return Err(crate::error::EnvironmentManagerError::NotFound(
                environment_id.as_uuid().to_string(),
            ));
        }

        Ok(EnvironmentDescriptor {
            environment_id: self.environment_id,
            root: self.root.clone(),
        })
    }

    async fn delete_environment(
        &self,
        _environment_id: EnvironmentId,
        _policy: EnvironmentDeletePolicy,
    ) -> EnvironmentManagerResult<()> {
        // Local environments are implicit and not deletable.
        Err(crate::error::EnvironmentManagerError::NotSupported(
            "local environment cannot be deleted".to_string(),
        ))
    }

    async fn environment_info(
        &self,
        environment_id: EnvironmentId,
    ) -> EnvironmentManagerResult<EnvironmentInfo> {
        if environment_id != self.environment_id {
            return Err(crate::error::EnvironmentManagerError::NotFound(
                environment_id.as_uuid().to_string(),
            ));
        }

        Ok(EnvironmentInfo::collect_for_path(&self.root)?)
    }
}
