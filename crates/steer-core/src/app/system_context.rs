use crate::app::EnvironmentInfo;

#[derive(Debug, Clone)]
pub struct SystemContext {
    pub prompt: String,
    pub environment: Option<EnvironmentInfo>,
}

impl SystemContext {
    pub fn new(prompt: String) -> Self {
        Self {
            prompt,
            environment: None,
        }
    }

    pub fn with_environment(prompt: String, environment: Option<EnvironmentInfo>) -> Self {
        Self { prompt, environment }
    }

    pub fn render(&self) -> Option<String> {
        self.render_with_prompt(Some(self.prompt.clone()))
    }

    pub fn render_with_prompt(&self, prompt: Option<String>) -> Option<String> {
        let base = prompt.unwrap_or_default();
        let base_trimmed = base.trim();
        let base_value = if base_trimmed.is_empty() {
            None
        } else {
            Some(base.trim_end().to_string())
        };

        let env_value = self
            .environment
            .as_ref()
            .map(|env| env.as_context())
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());

        let mut combined = String::new();
        if let Some(base_value) = base_value {
            combined.push_str(&base_value);
        }
        if let Some(env_value) = env_value {
            if !combined.is_empty() {
                combined.push_str("\n\n");
            }
            combined.push_str(&env_value);
        }

        if combined.trim().is_empty() {
            None
        } else {
            Some(combined)
        }
    }
}
