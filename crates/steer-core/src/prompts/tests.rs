#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Model, ModelFamily};

    #[test]
    fn test_default_prompt() {
        let prompt = DefaultPrompt;
        let content = prompt.system_prompt();
        assert!(content.contains("You are Steer"));
        assert!(content.contains("AI-powered agent"));
    }

    #[test]
    fn test_gemini_prompt() {
        let prompt = GeminiPrompt;
        let content = prompt.system_prompt();
        assert!(content.contains("You are Steer"));
        assert!(content.contains("slash commands"));
    }

    #[test]
    fn test_o3_prompt() {
        let prompt = O3Prompt;
        let content = prompt.system_prompt();
        assert!(content.contains("You are an AI coding assistant"));
        assert!(content.contains("working directory"));
    }

    #[test]
    fn test_prompt_selector_default() {
        let model = Model {
            family: ModelFamily::Claude,
            name: "claude-3-5-sonnet".to_string(),
            display_name: "Claude 3.5 Sonnet".to_string(),
        };
        let selector = PromptSelector::from_model(&model);
        assert!(matches!(selector, PromptSelector::Default));
    }

    #[test]
    fn test_prompt_selector_gemini() {
        let model = Model {
            family: ModelFamily::Gemini,
            name: "gemini-pro".to_string(),
            display_name: "Gemini Pro".to_string(),
        };
        let selector = PromptSelector::from_model(&model);
        assert!(matches!(selector, PromptSelector::Gemini));
    }

    #[test]
    fn test_prompt_selector_o3() {
        let model = Model {
            family: ModelFamily::OpenAI,
            name: "o3-2025-01-24".to_string(),
            display_name: "O3".to_string(),
        };
        let selector = PromptSelector::from_model(&model);
        assert!(matches!(selector, PromptSelector::O3));
    }

    #[test]
    fn test_selector_to_prompt() {
        let default_prompt = PromptSelector::Default.to_prompt();
        assert!(default_prompt.system_prompt().contains("You are Steer"));

        let gemini_prompt = PromptSelector::Gemini.to_prompt();
        assert!(gemini_prompt.system_prompt().contains("You are Steer"));

        let o3_prompt = PromptSelector::O3.to_prompt();
        assert!(o3_prompt.system_prompt().contains("You are an AI coding assistant"));
    }
}