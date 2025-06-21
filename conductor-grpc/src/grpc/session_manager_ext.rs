use conductor_core::session::{SessionManager, SessionManagerError, SessionConfig};
use conductor_core::app::conversation::Message;
use conductor_proto::agent as proto;
use crate::grpc::conversions::{
    message_to_proto, proto_to_tool_config, proto_to_workspace_config,
    session_config_to_proto,
};

/// Extension trait for SessionManager that adds gRPC-specific functionality
#[async_trait::async_trait]
pub trait SessionManagerExt {
    /// Get session state as protobuf SessionState
    async fn get_session_proto(
        &self,
        session_id: &str,
    ) -> Result<Option<proto::SessionState>, SessionManagerError>;

    /// Create session for gRPC
    async fn create_session_grpc(
        &self,
        config: proto::CreateSessionRequest,
        app_config: conductor_core::app::AppConfig,
    ) -> Result<(String, proto::SessionInfo), SessionManagerError>;
}

#[async_trait::async_trait]
impl SessionManagerExt for SessionManager {
    async fn get_session_proto(
        &self,
        session_id: &str,
    ) -> Result<Option<proto::SessionState>, SessionManagerError> {
        match self.store().get_session(session_id).await {
            Ok(Some(session)) => {
                // Use the conversion function to convert config
                let config = session_config_to_proto(&session.config);

                // Convert internal SessionState to protobuf SessionState
                let proto_state = proto::SessionState {
                    id: session_id.to_string(),
                    created_at: Some(prost_types::Timestamp::from(
                        std::time::SystemTime::from(session.created_at)
                    )),
                    updated_at: Some(prost_types::Timestamp::from(
                        std::time::SystemTime::from(session.updated_at)
                    )),
                    config: Some(config),
                    messages: session.state.messages.into_iter().map(|msg| {
                        message_to_proto(msg)
                    }).collect(),
                    tool_calls: std::collections::HashMap::new(), // TODO: Convert tool calls
                    approved_tools: session.state.approved_tools.into_iter().collect(),
                    last_event_sequence: session.state.last_event_sequence,
                    metadata: session.state.metadata,
                };
                Ok(Some(proto_state))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(SessionManagerError::Storage(e)),
        }
    }

    async fn create_session_grpc(
        &self,
        config: proto::CreateSessionRequest,
        app_config: conductor_core::app::AppConfig,
    ) -> Result<(String, proto::SessionInfo), SessionManagerError> {
        use conductor_core::session::ToolApprovalPolicy;
        
        // Convert protobuf config to internal SessionConfig
        let tool_policy = config
            .tool_policy
            .map(|policy| match policy.policy {
                Some(proto::tool_approval_policy::Policy::AlwaysAsk(_)) => {
                    ToolApprovalPolicy::AlwaysAsk
                }
                Some(proto::tool_approval_policy::Policy::PreApproved(
                    pre_approved,
                )) => ToolApprovalPolicy::PreApproved {
                    tools: pre_approved.tools.into_iter().collect(),
                },
                Some(proto::tool_approval_policy::Policy::Mixed(mixed)) => {
                    ToolApprovalPolicy::Mixed {
                        pre_approved: mixed.pre_approved_tools.into_iter().collect(),
                        ask_for_others: mixed.ask_for_others,
                    }
                }
                None => ToolApprovalPolicy::AlwaysAsk,
            })
            .unwrap_or(ToolApprovalPolicy::AlwaysAsk);

        let mut tool_config = config
            .tool_config
            .map(proto_to_tool_config)
            .unwrap_or_default();

        // Set the approval policy in the tool config
        tool_config.approval_policy = tool_policy;

        let workspace_config = config
            .workspace_config
            .map(proto_to_workspace_config)
            .unwrap_or_default();

        let session_config = SessionConfig {
            workspace: workspace_config,
            tool_config,
            system_prompt: config.system_prompt,
            metadata: config.metadata,
        };

        let (session_id, _command_tx) = self.create_session(session_config, app_config).await?;

        // Get session info for response
        let session_info = self.get_session(&session_id).await?.ok_or_else(|| {
            SessionManagerError::SessionNotActive {
                session_id: session_id.clone(),
            }
        })?;

        let proto_info = proto::SessionInfo {
            id: session_info.id,
            created_at: Some(prost_types::Timestamp::from(std::time::SystemTime::from(
                session_info.created_at,
            ))),
            updated_at: Some(prost_types::Timestamp::from(std::time::SystemTime::from(
                session_info.updated_at,
            ))),
            status: proto::SessionStatus::Active as i32,
            metadata: Some(proto::SessionMetadata {
                labels: session_info.metadata,
                annotations: std::collections::HashMap::new(),
            }),
        };

        Ok((session_id, proto_info))
    }
}