use std::collections::HashMap;

use steer_grpc::client_api::{ContextWindowUsage, ModelId, OpId, TokenUsage, UsageUpdateKind};

#[derive(Debug, Clone, PartialEq)]
pub struct LlmUsageSnapshot {
    pub op_id: OpId,
    pub model: ModelId,
    pub usage: TokenUsage,
    pub kind: UsageUpdateKind,
    pub context_window: Option<ContextWindowUsage>,
    pub max_context_tokens: Option<u32>,
    pub remaining_tokens: Option<u32>,
    pub utilization_ratio: Option<f64>,
    pub context_estimated: bool,
}

impl LlmUsageSnapshot {
    pub fn new(
        op_id: OpId,
        model: ModelId,
        usage: TokenUsage,
        context_window: Option<ContextWindowUsage>,
        kind: UsageUpdateKind,
    ) -> Self {
        let (max_context_tokens, remaining_tokens, utilization_ratio, context_estimated) =
            context_window
                .as_ref()
                .map_or((None, None, None, false), |context| {
                    (
                        context.max_context_tokens,
                        context.remaining_tokens,
                        context.utilization_ratio,
                        context.estimated,
                    )
                });

        Self {
            op_id,
            model,
            usage,
            kind,
            context_window,
            max_context_tokens,
            remaining_tokens,
            utilization_ratio,
            context_estimated,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct LlmUsageState {
    latest: Option<LlmUsageSnapshot>,
    by_op: HashMap<OpId, LlmUsageSnapshot>,
}

impl LlmUsageState {
    pub fn update(
        &mut self,
        op_id: OpId,
        model: ModelId,
        usage: TokenUsage,
        context_window: Option<ContextWindowUsage>,
        kind: UsageUpdateKind,
    ) {
        let snapshot = LlmUsageSnapshot::new(op_id, model, usage, context_window, kind);
        self.by_op.insert(snapshot.op_id, snapshot.clone());
        self.latest = Some(snapshot);
    }

    pub fn latest(&self) -> Option<&LlmUsageSnapshot> {
        self.latest.as_ref()
    }

    pub fn for_op(&self, op_id: &OpId) -> Option<&LlmUsageSnapshot> {
        self.by_op.get(op_id)
    }

    pub fn clear(&mut self) {
        self.latest = None;
        self.by_op.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use steer_grpc::client_api::builtin;

    #[test]
    fn update_tracks_latest_snapshot_and_context_utilization_fields() {
        let mut usage_state = LlmUsageState::default();
        let op_id = OpId::new();
        let model = builtin::claude_sonnet_4_5();
        let usage = TokenUsage::from_input_output(120, 30);

        usage_state.update(
            op_id,
            model.clone(),
            usage,
            Some(ContextWindowUsage {
                max_context_tokens: Some(200_000),
                remaining_tokens: Some(199_850),
                utilization_ratio: Some(0.00075),
                estimated: false,
            }),
            UsageUpdateKind::Final,
        );

        let latest = usage_state.latest().expect("latest usage should be set");
        assert_eq!(latest.op_id, op_id);
        assert_eq!(latest.model, model);
        assert_eq!(latest.usage, usage);
        assert_eq!(latest.kind, UsageUpdateKind::Final);
        assert_eq!(latest.max_context_tokens, Some(200_000));
        assert_eq!(latest.remaining_tokens, Some(199_850));
        assert_eq!(latest.utilization_ratio, Some(0.00075));
        assert!(!latest.context_estimated);

        let per_op = usage_state
            .for_op(&op_id)
            .expect("per-op usage should be tracked");
        assert_eq!(per_op, latest);
    }

    #[test]
    fn update_without_context_window_clears_utilization_fields() {
        let mut usage_state = LlmUsageState::default();
        let op_id = OpId::new();

        usage_state.update(
            op_id,
            builtin::claude_sonnet_4_5(),
            TokenUsage::from_input_output(10, 2),
            None,
            UsageUpdateKind::Partial,
        );

        let latest = usage_state.latest().expect("latest usage should be set");
        assert_eq!(latest.max_context_tokens, None);
        assert_eq!(latest.remaining_tokens, None);
        assert_eq!(latest.utilization_ratio, None);
        assert!(!latest.context_estimated);
    }
}
