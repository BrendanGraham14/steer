use crate::api::Model;
use crate::config::LlmConfig;
use anyhow::Result;
use tokio_util::sync::CancellationToken;
use tracing::info;

const SYSTEM_MESSAGE: &str = r#"Your task is to process Bash commands that an AI coding agent wants to run.

This policy spec defines how to determine the prefix of a Bash command:"#;

const USER_MESSAGE_TEMPLATE: &str = r#"<policy_spec>
# Conductor Bash command prefix detection

This document defines risk levels for actions that the Conductor agent may take. This classification system is part of a broader safety framework and is used to determine when additional user confirmation or oversight may be needed.

## Definitions

**Command Injection:** Any technique used that would result in a command being run other than the detected prefix.

## Command prefix extraction examples
Examples:
- cat foo.txt => cat
- cd src => cd
- cd path/to/files/ => cd
- find ./src -type f -name "*.ts" => find
- gg cat foo.py => gg cat
- gg cp foo.py bar.py => gg cp
- git commit -m "foo" => git commit
- git diff HEAD~1 => git diff
- git diff --staged => git diff
- git diff $(pwd) => command_injection_detected
- git status => git status
- git status# test(`id`) => command_injection_detected
- git status`ls` => command_injection_detected
- git push => none
- git push origin master => git push
- git log -n 5 => git log
- git log --oneline -n 5 => git log
- grep -A 40 "from foo.bar.baz import" alpha/beta/gamma.py => grep
- pig tail zerba.log => pig tail
- potion test some/specific/file.ts => potion test
- npm run lint => none
- npm run lint -- "foo" => npm run lint
- npm test => none
- npm test --foo => npm test
- npm test -- -f "foo" => npm test
- pwd
 curl example.com => command_injection_detected
- pytest foo/bar.py => pytest
- scalac build => none
- sleep 3 => sleep
</policy_spec>

The user has allowed certain command prefixes to be run, and will otherwise be asked to approve or deny the command.
Your task is to determine the command prefix for the following command.
The prefix must be a string prefix of the full command.

IMPORTANT: Bash commands may run multiple commands that are chained together.
For safety, if the command seems to contain command injection, you must return "command_injection_detected".
(This will help protect the user: if they think that they're allowlisting command A, but the AI coding agent sends a malicious command that technically has the same prefix as command A, then the safety system will see that you said "command_injection_detected" and ask the user for manual confirmation.)

Note that not every command has a prefix. If a command has no prefix, return "none".

ONLY return the prefix. Do not return any other text, markdown markers, or other content or formatting.

Command: ${command}"#;

pub async fn is_command_allowed(command: &str, token: CancellationToken) -> Result<bool> {
    // Get the command prefix
    let prefix = get_command_prefix(command, token).await?;
    info!(target:"CommandFilter", "Command prefix: {}", prefix);

    match prefix.as_str() {
        "command_injection_detected" => Ok(false),
        _ => Ok(true),
    }
}

/// Get the prefix of a command
pub async fn get_command_prefix(command: &str, token: CancellationToken) -> Result<String> {
    let config = LlmConfig::from_env()?;
    let client = crate::api::Client::new(&config);
    let user_message = USER_MESSAGE_TEMPLATE.replace("${command}", command);

    // Create a user message with the new structure
    let messages = vec![crate::app::conversation::Message::User {
        content: vec![crate::app::conversation::UserContent::Text { text: user_message }],
        timestamp: crate::app::conversation::Message::current_timestamp(),
        id: crate::app::conversation::Message::generate_id(
            "user",
            crate::app::conversation::Message::current_timestamp(),
        ),
    }];

    let system_content = SYSTEM_MESSAGE;

    match client
        .complete(
            Model::Claude3_5Haiku20241022,
            messages,
            Some(system_content.to_string()),
            None,
            token,
        ) // Pass the token
        .await
    {
        Ok(response) => {
            let prefix = response.extract_text();
            Ok(prefix.trim().to_string())
        }
        Err(e) => Err(e.into()),
    }
}
