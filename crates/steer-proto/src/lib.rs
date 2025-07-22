pub mod common {
    pub mod v1 {
        tonic::include_proto!("steer.common.v1");
    }
}

pub mod agent {
    pub mod v1 {
        tonic::include_proto!("steer.agent.v1");
    }
}

pub mod remote_workspace {
    pub mod v1 {
        tonic::include_proto!("steer.remote_workspace.v1");
    }
}
