use conductor_core::api::Model;
use conductor_core::app::conversation::{AssistantContent, Message, MessageData, UserContent};

// This test requires real API keys and makes actual API calls
// Run with: cargo test --test headless_test -- --ignored
#[tokio::test]
#[ignore]
async fn test_headless_mode_integration() {
    dotenv::dotenv().ok();

    // Create a simple test message
    let timestamp = Message::current_timestamp();
    let messages = vec![Message {
        data: MessageData::User {
            content: vec![UserContent::Text {
                text: "What is 2+2?".to_string(),
            }],
        },
        timestamp,
        id: Message::generate_id("user", timestamp),
        parent_message_id: None,
    }];

    // Load config from environment is handled internally by run_once

    // Call run_once - note: new signature doesn't take config or timeout
    let result = conductor_cli::run_once(messages, Model::Claude3_7Sonnet20250219)
        .await
        .expect("run_once should succeed");

    // First assert that we got an Assistant message
    assert!(
        matches!(result.final_message.data, MessageData::Assistant { .. }),
        "Expected Assistant message in response"
    );

    // Extract content safely
    if let MessageData::Assistant { content, .. } = &result.final_message.data {
        // The response should contain the answer (4)
        let text_blocks: Vec<_> = content
            .iter()
            .filter_map(|c| {
                if let AssistantContent::Text { text } = c {
                    Some(text)
                } else {
                    None
                }
            })
            .collect();

        // Check that we got at least one text block
        assert!(!text_blocks.is_empty(), "No text blocks found in response");

        // Check that at least one text block contains "4"
        let contains_answer = text_blocks.iter().any(|text| text.contains("4"));
        assert!(contains_answer, "Response should contain the answer '4'");
    }
}
