use steer::RuntimeBuilder;
use steer_core::app::MessageData;
use steer_core::app::conversation::AssistantContent;
use steer_core::test_utils::read_only_session_config;

#[tokio::test]
#[ignore = "requires external API credentials"]
async fn test_headless_mode_integration() {
    dotenvy::dotenv().ok();

    let message = "What is 2+2?".to_string();

    let (runtime, model) = RuntimeBuilder::new("claude-3-5-sonnet-latest".to_string())
        .build()
        .await
        .expect("Failed to create runtime");

    let config = read_only_session_config(model.clone());

    let result = steer::run_once_new_session(&runtime.handle(), config, message, model)
        .await
        .expect("run_once should succeed");

    assert!(
        matches!(result.final_message.data, MessageData::Assistant { .. }),
        "Expected Assistant message in response"
    );

    if let MessageData::Assistant { content, .. } = &result.final_message.data {
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

        assert!(!text_blocks.is_empty(), "No text blocks found in response");

        let contains_answer = text_blocks.iter().any(|text| text.contains('4'));
        assert!(contains_answer, "Response should contain the answer '4'");
    }

    runtime.shutdown().await;
}
