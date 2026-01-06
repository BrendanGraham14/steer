//! Tool capability system for static tools.
//!
//! Capabilities define what runtime services a tool needs. The runtime
//! advertises its available capabilities, and only tools whose requirements
//! are satisfied get exposed to the model.

use bitflags::bitflags;
use serde::{Deserialize, Serialize};

bitflags! {
    /// Capabilities that the runtime can provide to tools.
    ///
    /// Tools declare which capabilities they require, and the runtime
    /// filters tool availability based on what it can provide.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
    pub struct Capabilities: u64 {
        /// Access to the workspace (filesystem, environment, working directory).
        /// Almost all tools require this.
        const WORKSPACE = 1 << 0;

        /// Ability to spawn sub-agents that run tool loops.
        /// Implies access to EventStore, ApiClient, ToolExecutor.
        const AGENT_SPAWNER = 1 << 1;

        /// Ability to call LLM models directly (outside of agent loop).
        /// Used by tools like `fetch` that need summarization.
        const MODEL_CALLER = 1 << 2;

        /// Network access for external HTTP requests.
        const NETWORK = 1 << 3;

        /// Convenience: basic workspace tools (grep, view, glob, ls)
        const BASIC = Self::WORKSPACE.bits();

        /// Convenience: tools that need to spawn agents
        const AGENT = Self::WORKSPACE.bits() | Self::AGENT_SPAWNER.bits();

        /// Convenience: tools that need model access
        const MODEL = Self::WORKSPACE.bits() | Self::MODEL_CALLER.bits();
    }
}

impl Capabilities {
    /// Check if these capabilities satisfy a tool's requirements.
    pub fn satisfies(&self, required: Capabilities) -> bool {
        self.contains(required)
    }

    /// Return human-readable names of the capabilities in this set.
    pub fn names(&self) -> Vec<&'static str> {
        let mut names = Vec::new();
        if self.contains(Capabilities::WORKSPACE) {
            names.push("workspace");
        }
        if self.contains(Capabilities::AGENT_SPAWNER) {
            names.push("agent_spawner");
        }
        if self.contains(Capabilities::MODEL_CALLER) {
            names.push("model_caller");
        }
        if self.contains(Capabilities::NETWORK) {
            names.push("network");
        }
        names
    }

    /// Return the capabilities that are missing to satisfy requirements.
    pub fn missing(&self, required: Capabilities) -> Capabilities {
        required - *self
    }
}

impl Default for Capabilities {
    fn default() -> Self {
        Capabilities::empty()
    }
}

impl std::fmt::Display for Capabilities {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let names = self.names();
        if names.is_empty() {
            write!(f, "(none)")
        } else {
            write!(f, "{}", names.join(", "))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_satisfies_exact() {
        let available = Capabilities::WORKSPACE;
        assert!(available.satisfies(Capabilities::WORKSPACE));
    }

    #[test]
    fn test_satisfies_superset() {
        let available = Capabilities::WORKSPACE | Capabilities::AGENT_SPAWNER;
        assert!(available.satisfies(Capabilities::WORKSPACE));
        assert!(available.satisfies(Capabilities::AGENT_SPAWNER));
        assert!(available.satisfies(Capabilities::WORKSPACE | Capabilities::AGENT_SPAWNER));
    }

    #[test]
    fn test_satisfies_missing() {
        let available = Capabilities::WORKSPACE;
        assert!(!available.satisfies(Capabilities::AGENT_SPAWNER));
        assert!(!available.satisfies(Capabilities::WORKSPACE | Capabilities::AGENT_SPAWNER));
    }

    #[test]
    fn test_missing_capabilities() {
        let available = Capabilities::WORKSPACE;
        let required = Capabilities::WORKSPACE | Capabilities::AGENT_SPAWNER;
        let missing = available.missing(required);
        assert_eq!(missing, Capabilities::AGENT_SPAWNER);
    }

    #[test]
    fn test_convenience_flags() {
        assert!(Capabilities::BASIC.contains(Capabilities::WORKSPACE));
        assert!(Capabilities::AGENT.contains(Capabilities::WORKSPACE));
        assert!(Capabilities::AGENT.contains(Capabilities::AGENT_SPAWNER));
    }

    #[test]
    fn test_display() {
        let caps = Capabilities::WORKSPACE | Capabilities::NETWORK;
        let s = caps.to_string();
        assert!(s.contains("workspace"));
        assert!(s.contains("network"));
    }
}
