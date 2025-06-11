use crate::grpc::proto;
use crate::session::state::{
    BackendConfig, ContainerRuntime, RemoteAuth, SessionConfig, SessionToolConfig,
    ToolApprovalPolicy, ToolFilter, WorkspaceConfig,
};
use std::collections::HashSet;

/// Convert internal ToolApprovalPolicy to protobuf
pub fn tool_approval_policy_to_proto(policy: &ToolApprovalPolicy) -> proto::ToolApprovalPolicy {
    use proto::{
        AlwaysAskPolicy, ApprovalDecision, MixedPolicy, PreApprovedPolicy,
        tool_approval_policy::Policy,
    };

    let policy_variant = match policy {
        ToolApprovalPolicy::AlwaysAsk => Policy::AlwaysAsk(AlwaysAskPolicy {
            timeout_ms: None,
            default_decision: ApprovalDecision::Deny as i32,
        }),
        ToolApprovalPolicy::PreApproved { tools } => Policy::PreApproved(PreApprovedPolicy {
            tools: tools.iter().cloned().collect(),
        }),
        ToolApprovalPolicy::Mixed {
            pre_approved,
            ask_for_others,
        } => Policy::Mixed(MixedPolicy {
            pre_approved_tools: pre_approved.iter().cloned().collect(),
            ask_for_others: *ask_for_others,
            timeout_ms: None,
            default_decision: ApprovalDecision::Deny as i32,
        }),
    };

    proto::ToolApprovalPolicy {
        policy: Some(policy_variant),
    }
}

/// Convert internal WorkspaceConfig to protobuf
pub fn workspace_config_to_proto(config: &WorkspaceConfig) -> proto::WorkspaceConfig {
    use proto::workspace_config::Config;

    let config_variant = match config {
        WorkspaceConfig::Local => Config::Local(proto::LocalWorkspaceConfig {}),
        WorkspaceConfig::Remote {
            agent_address,
            auth,
        } => Config::Remote(proto::RemoteWorkspaceConfig {
            agent_address: agent_address.clone(),
            auth: auth.as_ref().map(remote_auth_to_proto),
        }),
        WorkspaceConfig::Container { image, runtime } => {
            Config::Container(proto::ContainerWorkspaceConfig {
                image: image.clone(),
                runtime: container_runtime_to_proto(runtime) as i32,
            })
        }
    };

    proto::WorkspaceConfig {
        config: Some(config_variant),
    }
}

/// Convert internal RemoteAuth to protobuf
pub fn remote_auth_to_proto(auth: &RemoteAuth) -> proto::RemoteAuth {
    use proto::remote_auth::Auth;

    let auth_variant = match auth {
        RemoteAuth::Bearer { token } => Auth::BearerToken(token.clone()),
        RemoteAuth::ApiKey { key } => Auth::ApiKey(key.clone()),
    };

    proto::RemoteAuth {
        auth: Some(auth_variant),
    }
}

/// Convert internal ContainerRuntime to protobuf
pub fn container_runtime_to_proto(runtime: &ContainerRuntime) -> proto::ContainerRuntime {
    match runtime {
        ContainerRuntime::Docker => proto::ContainerRuntime::Docker,
        ContainerRuntime::Podman => proto::ContainerRuntime::Podman,
    }
}

/// Convert internal ToolFilter to protobuf
pub fn tool_filter_to_proto(filter: &ToolFilter) -> proto::ToolFilter {
    use proto::tool_filter::Filter;

    let filter_variant = match filter {
        ToolFilter::All => Filter::All(true),
        ToolFilter::Include(tools) => Filter::Include(proto::IncludeFilter {
            tools: tools.clone(),
        }),
        ToolFilter::Exclude(tools) => Filter::Exclude(proto::ExcludeFilter {
            tools: tools.clone(),
        }),
    };

    proto::ToolFilter {
        filter: Some(filter_variant),
    }
}

/// Convert internal BackendConfig to protobuf
pub fn backend_config_to_proto(config: &BackendConfig) -> proto::BackendConfig {
    use proto::backend_config::Backend;

    let backend_variant = match config {
        BackendConfig::Local { tool_filter } => Backend::Local(proto::LocalBackendConfig {
            tool_filter: Some(tool_filter_to_proto(tool_filter)),
        }),
        BackendConfig::Remote {
            name,
            endpoint,
            auth,
            tool_filter,
        } => Backend::Remote(proto::RemoteBackendConfig {
            name: name.clone(),
            endpoint: endpoint.clone(),
            auth: auth.as_ref().map(remote_auth_to_proto),
            tool_filter: Some(tool_filter_to_proto(tool_filter)),
        }),
        BackendConfig::Container {
            image,
            runtime,
            tool_filter,
        } => Backend::Container(proto::ContainerBackendConfig {
            image: image.clone(),
            runtime: container_runtime_to_proto(runtime) as i32,
            tool_filter: Some(tool_filter_to_proto(tool_filter)),
        }),
        BackendConfig::Mcp {
            server_name,
            transport,
            command,
            args,
            tool_filter,
        } => Backend::Mcp(proto::McpBackendConfig {
            server_name: server_name.clone(),
            transport: transport.clone(),
            command: command.clone(),
            args: args.clone(),
            tool_filter: Some(tool_filter_to_proto(tool_filter)),
        }),
    };

    proto::BackendConfig {
        backend: Some(backend_variant),
    }
}

/// Convert internal SessionToolConfig to protobuf
pub fn session_tool_config_to_proto(config: &SessionToolConfig) -> proto::SessionToolConfig {
    proto::SessionToolConfig {
        backends: config
            .backends
            .iter()
            .map(backend_config_to_proto)
            .collect(),
        metadata: config.metadata.clone(),
    }
}

/// Convert internal SessionConfig to protobuf
pub fn session_config_to_proto(config: &SessionConfig) -> proto::SessionConfig {
    proto::SessionConfig {
        tool_policy: Some(tool_approval_policy_to_proto(
            &config.tool_config.approval_policy,
        )),
        tool_config: Some(session_tool_config_to_proto(&config.tool_config)),
        metadata: config.metadata.clone(),
        workspace_config: Some(workspace_config_to_proto(&config.workspace)),
        system_prompt: config.system_prompt.clone(),
    }
}
