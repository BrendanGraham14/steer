use coder::api::{
    Model,
    messages::{Message, MessageContent, MessageRole},
};
use coder::config::LlmConfig;
use std::time::Duration;

// This test requires real API keys and makes actual API calls
// Run with: cargo test --test headless_test -- --ignored
#[tokio::test]
#[ignore]
async fn test_headless_mode_integration() {
    dotenv::dotenv().ok();

    // Create a simple test message
    let messages = vec![Message {
        role: MessageRole::User,
        content: MessageContent::Text {
            content: "What is 2+2?".to_string(),
        },
        id: None,
    }];

    // Load config from environment
    let config = LlmConfig::from_env().expect("Failed to load config from environment");

    // Set a reasonable timeout
    let timeout = Some(Duration::from_secs(30));

    // Call run_once
    let result = coder::run_once(messages, Model::Claude3_7Sonnet20250219, &config, timeout)
        .await
        .expect("run_once should succeed");

    // Verify we got a response with the correct structure
    assert_eq!(result.final_msg.role, MessageRole::Assistant);

    // The response should contain the answer (4)
    if let MessageContent::StructuredContent { content } = &result.final_msg.content {
        let text_blocks: Vec<_> = content
            .0
            .iter()
            .filter_map(|block| {
                if let coder::api::messages::ContentBlock::Text { text } = block {
                    Some(text)
                } else {
                    None
                }
            })
            .collect();

        // Check that we got at least one text block
        assert!(!text_blocks.is_empty());

        // Check that at least one text block contains "4"
        let contains_answer = text_blocks.iter().any(|text| text.contains("4"));
        assert!(contains_answer, "Response should contain the answer '4'");
    } else {
        panic!("Expected StructuredContent in response");
    }
}
