pub mod common {
    pub mod v1 {
        tonic::include_proto!("conductor.common.v1");
    }
}

pub mod agent {
    pub mod v1 {
        tonic::include_proto!("conductor.agent.v1");
    }
}

pub mod remote_workspace {
    pub mod v1 {
        tonic::include_proto!("conductor.remote_workspace.v1");
    }
}
