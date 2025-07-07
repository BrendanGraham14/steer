use conductor_core::auth::{DefaultAuthStorage, anthropic::AnthropicOAuth};
use eyre::{Result, bail, eyre};
use std::sync::Arc;

pub async fn execute(provider: &str) -> Result<()> {
    match provider {
        "anthropic" | "claude" => login_anthropic().await,
        _ => bail!(
            "Unsupported provider: {}. Currently only 'anthropic' is supported.",
            provider
        ),
    }
}

async fn login_anthropic() -> Result<()> {
    println!("Starting OAuth login for Anthropic...");

    // Create OAuth client
    let oauth_client = AnthropicOAuth::new();

    // Generate PKCE challenge
    let pkce = AnthropicOAuth::generate_pkce();

    // Build authorization URL (state is the PKCE verifier)
    let auth_url = oauth_client.build_auth_url(&pkce);

    println!("\nOpening browser to authorize Conductor...");
    println!("If the browser doesn't open automatically, please visit:");
    println!("{auth_url}");
    println!();

    // Open browser
    if let Err(e) = open::that(&auth_url) {
        eprintln!("Failed to open browser: {e}. Please open the URL manually.");
    }

    println!("After authorizing, you'll be redirected to a page showing a code.");
    println!("Please copy the ENTIRE code (including the part after the #) and paste it here:");
    println!();
    print!("Code: ");
    use std::io::{self, Write};
    io::stdout().flush()?;

    let mut callback_code = String::new();
    io::stdin().read_line(&mut callback_code)?;
    let callback_code = callback_code.trim();

    // Parse the code and state
    let (code, returned_state) = AnthropicOAuth::parse_callback_code(callback_code)
        .map_err(|e| eyre!("Failed to parse callback code: {}", e))?;

    // Verify state matches (state is the PKCE verifier)
    if returned_state != pkce.verifier {
        bail!("State mismatch. The authorization may have been tampered with.");
    }

    println!("\nExchanging authorization code for tokens...");

    // Exchange code for tokens
    let tokens = oauth_client
        .exchange_code_for_tokens(&code, &returned_state, &pkce.verifier)
        .await
        .map_err(|e| eyre!("Failed to exchange code for tokens: {}", e))?;

    // Store tokens
    let storage = Arc::new(
        DefaultAuthStorage::new().map_err(|e| eyre!("Failed to create auth storage: {}", e))?,
    ) as Arc<dyn conductor_core::auth::AuthStorage>;

    storage
        .set_tokens("anthropic", tokens.clone())
        .await
        .map_err(|e| eyre!("Failed to store tokens: {}", e))?;

    println!("\n✅ Successfully logged in to Anthropic!");
    println!("You can now use Claude models without an API key.");

    // Note: Authentication test currently disabled
    // The OAuth tokens are valid but the test endpoint might require additional setup
    println!("\nNote: You can verify authentication by running:");
    println!("  conductor auth status");

    Ok(())
}
