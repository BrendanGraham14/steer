use anyhow::{Context, Result};
use serde_json::Value;
use regex::Regex;
use std::collections::HashMap;

/// Handle a tool call from Claude
pub async fn handle_tool_call(message: &str) -> Result<Vec<(String, String)>> {
    let mut results = Vec::new();
    
    // Extract function calls from the message
    let function_calls = extract_function_calls(message)?;
    
    // Execute each function call
    for (name, params) in function_calls {
        let result = crate::tools::execute_tool(&name, &params).await?;
        results.push((name, result));
    }
    
    Ok(results)
}

/// Extract function calls from a message
fn extract_function_calls(message: &str) -> Result<Vec<(String, Value)>> {
    let mut calls = Vec::new();
    
    // We'll look for patterns like <function_calls><invoke name="NAME"><parameter name="PARAM">VALUE</parameter></invoke></function_calls>
    
    // Look for blocks delimited by <function_calls> tags
    let function_blocks_re = Regex::new(r"<function_calls>(.*?)</function_calls>")?;
    
    for captures in function_blocks_re.captures_iter(message) {
        if let Some(block) = captures.get(1) {
            let function_block = block.as_str();
            
            // Extract individual function invokes
            let invoke_re = Regex::new(r#"<invoke name="([^"]+)">(.*?)</invoke>"#)?;
            
            for invoke_captures in invoke_re.captures_iter(function_block) {
                if let (Some(name_match), Some(params_match)) = (invoke_captures.get(1), invoke_captures.get(2)) {
                    let name = name_match.as_str().to_string();
                    let params_text = params_match.as_str();
                    
                    // Extract parameters
                    let param_re = Regex::new(r#"<parameter name="([^"]+)">(.*?)</parameter>"#)?;
                    let mut params = HashMap::new();
                    
                    for param_captures in param_re.captures_iter(params_text) {
                        if let (Some(param_name), Some(param_value)) = (param_captures.get(1), param_captures.get(2)) {
                            params.insert(param_name.as_str().to_string(), param_value.as_str().to_string());
                        }
                    }
                    
                    // Convert params to JSON
                    let params_json = serde_json::to_value(params)?;
                    calls.push((name, params_json));
                }
            }
        }
    }
    
    Ok(calls)
}
