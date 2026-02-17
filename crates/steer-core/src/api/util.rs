use crate::api::error::ApiError;

/// Normalize a chat completions URL.
/// Ensures the URL ends with the correct path for chat completions.
pub fn normalize_chat_url(base_url: Option<&str>, default_url: &str) -> String {
    let base_url = base_url.map_or_else(|| default_url.to_string(), |s| s.to_string());

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
    let base_url = base_url.map_or_else(|| default_url.to_string(), |s| s.to_string());

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

pub fn map_http_status_to_api_error(provider: &str, status_code: u16, details: String) -> ApiError {
    match status_code {
        401 | 403 => ApiError::AuthenticationFailed {
            provider: provider.to_string(),
            details,
        },
        408 => ApiError::Timeout {
            provider: provider.to_string(),
        },
        429 => ApiError::RateLimited {
            provider: provider.to_string(),
            details,
        },
        409 => ApiError::ServerError {
            provider: provider.to_string(),
            status_code,
            details,
        },
        400..=499 => ApiError::InvalidRequest {
            provider: provider.to_string(),
            details,
        },
        500..=599 => ApiError::ServerError {
            provider: provider.to_string(),
            status_code,
            details,
        },
        _ => ApiError::Unknown {
            provider: provider.to_string(),
            details,
        },
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
    fn test_map_http_status_to_api_error() {
        let provider = "test";

        assert!(matches!(
            map_http_status_to_api_error(provider, 401, "auth".to_string()),
            ApiError::AuthenticationFailed { .. }
        ));
        assert!(matches!(
            map_http_status_to_api_error(provider, 408, "timeout".to_string()),
            ApiError::Timeout { .. }
        ));
        assert!(matches!(
            map_http_status_to_api_error(provider, 409, "conflict".to_string()),
            ApiError::ServerError {
                status_code: 409,
                ..
            }
        ));
        assert!(matches!(
            map_http_status_to_api_error(provider, 429, "rate".to_string()),
            ApiError::RateLimited { .. }
        ));
        assert!(matches!(
            map_http_status_to_api_error(provider, 400, "bad request".to_string()),
            ApiError::InvalidRequest { .. }
        ));
        assert!(matches!(
            map_http_status_to_api_error(provider, 503, "server".to_string()),
            ApiError::ServerError {
                status_code: 503,
                ..
            }
        ));
        assert!(matches!(
            map_http_status_to_api_error(provider, 302, "redirect".to_string()),
            ApiError::Unknown { .. }
        ));
    }
}
