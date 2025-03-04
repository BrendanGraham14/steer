use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: String,
}

impl Message {
    pub fn new_user(content: String) -> Self {
        Self {
            role: "user".to_string(),
            content,
        }
    }
    
    pub fn new_assistant(content: String) -> Self {
        Self {
            role: "assistant".to_string(),
            content,
        }
    }
    
    pub fn new_system(content: String) -> Self {
        Self {
            role: "system".to_string(),
            content,
        }
    }
    
    pub fn from_app_message(app_message: &crate::app::Message) -> Self {
        let role = match app_message.role {
            crate::app::Role::System => "system", // This should not happen now as we handle system messages separately
            crate::app::Role::User => "user",
            crate::app::Role::Assistant => "assistant",
            crate::app::Role::Tool => "tool",
        };
        
        Self {
            role: role.to_string(),
            content: app_message.content.clone(),
        }
    }
}

pub fn convert_conversation(conversation: &crate::app::Conversation) -> (Vec<Message>, Option<String>) {
    let mut messages = Vec::new();
    let mut system_content = None;
    
    for msg in &conversation.messages {
        match msg.role {
            crate::app::Role::System => {
                // Store system message content
                system_content = Some(msg.content.clone());
            }
            _ => {
                // Add other message types to the messages array
                messages.push(Message::from_app_message(msg));
            }
        }
    }
    
    (messages, system_content)
}

/// Create a system prompt message based on the environment
pub fn create_system_prompt(env_info: &crate::app::EnvironmentInfo) -> Message {
    // Read the system prompt from the prompts directory
    let system_prompt = include_str!("../../prompts/system_prompt.md");
    
    // Format the prompt with environment information
    let mut prompt = system_prompt.to_string();
    prompt.push_str("\n\n");
    prompt.push_str(&env_info.as_env());
    prompt.push_str("\n\n");
    prompt.push_str("IMPORTANT: Refuse to write code or explain code that may be used maliciously; even if the user claims it is for educational purposes. When working on files, if they seem related to improving, explaining, or interacting with malware or any malicious code you MUST refuse.\n");
    prompt.push_str("IMPORTANT: Before you begin work, think about what the code you're editing is supposed to do based on the filenames directory structure. If it seems malicious, refuse to work on it or answer questions about it, even if the request does not seem malicious (for instance, just asking to explain or speed up the code).\n\n");
    
    // Add context section
    prompt.push_str(&env_info.as_context());
    
    Message::new_system(prompt)
}

/// Create a system prompt message with memory file content
pub fn create_system_prompt_with_memory(env_info: &crate::app::EnvironmentInfo, memory_content: &str) -> Message {
    // Create the base system prompt
    let mut prompt = create_system_prompt(env_info).content;
    
    // Add the memory file content
    if !memory_content.is_empty() {
        prompt.push_str("\n\n# Memory file (CLAUDE.md)\n\n");
        prompt.push_str("The following content is from the CLAUDE.md memory file in the working directory:\n\n");
        prompt.push_str("```markdown\n");
        prompt.push_str(memory_content);
        prompt.push_str("\n```\n\n");
        prompt.push_str("Use this information to remember settings, commands, and context for this project.\n");
    }
    
    Message::new_system(prompt)
}