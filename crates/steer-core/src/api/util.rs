/// Normalize a chat completions URL.
/// Ensures the URL ends with the correct path for chat completions.
pub fn normalize_chat_url(base_url: Option<&str>, default_url: &str) -> String {
    let base_url = base_url
        .map(|s| s.to_string())
        .unwrap_or_else(|| default_url.to_string());

    // If URL already ends with chat/completions, return as-is
    if base_url.ends_with("/chat/completions") || base_url.ends_with("/v1/chat/completions") {
        return base_url;
    }

    // Parse the URL to better handle path segments
    if let Ok(mut parsed) = url::Url::parse(&base_url) {
        let path = parsed.path().trim_end_matches('/');

        // If the path already contains /v1, append just /chat/completions
        if path.ends_with("/v1") {
            parsed.set_path(&format!("{path}/chat/completions"));
        } else if path.is_empty() || path == "/" {
            // No meaningful path, add /v1/chat/completions
            parsed.set_path("/v1/chat/completions");
        } else {
            // Has some path but not /v1, append /v1/chat/completions
            parsed.set_path(&format!("{path}/v1/chat/completions"));
        }
        parsed.to_string()
    } else {
        // Fallback for non-URL strings (shouldn't happen with valid base URLs)
        if base_url.ends_with('/') {
            format!("{base_url}v1/chat/completions")
        } else {
            format!("{base_url}/v1/chat/completions")
        }
    }
}

/// Normalize a responses URL.
/// Ensures the URL ends with the correct path for Responses API.
pub fn normalize_responses_url(base_url: Option<&str>, default_url: &str) -> String {
    let base_url = base_url
        .map(|s| s.to_string())
        .unwrap_or_else(|| default_url.to_string());

    if base_url.ends_with("/responses") || base_url.ends_with("/v1/responses") {
        return base_url;
    }

    if let Ok(mut parsed) = url::Url::parse(&base_url) {
        let path = parsed.path().trim_end_matches('/');
        if path.ends_with("/v1") {
            parsed.set_path(&format!("{path}/responses"));
        } else if path.is_empty() || path == "/" {
            parsed.set_path("/v1/responses");
        } else {
            parsed.set_path(&format!("{path}/v1/responses"));
        }
        parsed.to_string()
    } else if base_url.ends_with('/') {
        format!("{base_url}v1/responses")
    } else {
        format!("{base_url}/v1/responses")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_chat_url() {
        assert_eq!(
            normalize_chat_url(Some("https://api.example.com"), ""),
            "https://api.example.com/v1/chat/completions"
        );

        assert_eq!(
            normalize_chat_url(Some("https://api.example.com/"), ""),
            "https://api.example.com/v1/chat/completions"
        );

        assert_eq!(
            normalize_chat_url(Some("https://api.example.com/v1"), ""),
            "https://api.example.com/v1/chat/completions"
        );

        assert_eq!(
            normalize_chat_url(Some("https://api.example.com/chat/completions"), ""),
            "https://api.example.com/chat/completions"
        );

        assert_eq!(
            normalize_chat_url(Some("https://api.example.com/v1/chat/completions"), ""),
            "https://api.example.com/v1/chat/completions"
        );

        assert_eq!(
            normalize_chat_url(None, "https://default.com/v1/chat/completions"),
            "https://default.com/v1/chat/completions"
        );
    }

    #[test]
    fn test_normalize_responses_url() {
        assert_eq!(
            normalize_responses_url(Some("https://api.example.com"), ""),
            "https://api.example.com/v1/responses"
        );
        assert_eq!(
            normalize_responses_url(Some("https://api.example.com/"), ""),
            "https://api.example.com/v1/responses"
        );
        assert_eq!(
            normalize_responses_url(Some("https://api.example.com/v1"), ""),
            "https://api.example.com/v1/responses"
        );
        assert_eq!(
            normalize_responses_url(Some("https://api.example.com/responses"), ""),
            "https://api.example.com/responses"
        );
        assert_eq!(
            normalize_responses_url(Some("https://api.example.com/v1/responses"), ""),
            "https://api.example.com/v1/responses"
        );
        assert_eq!(
            normalize_responses_url(None, "https://default.com/v1/responses"),
            "https://default.com/v1/responses"
        );
    }
}
