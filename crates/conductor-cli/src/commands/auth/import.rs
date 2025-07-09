use clap::Args;
use conductor_core::{
    api::ProviderKind,
    auth::{AuthStorage, Credential, DefaultAuthStorage},
};
use rpassword::prompt_password;
use zeroize::Zeroizing;

// Wrapper to implement clap::ValueEnum for ProviderKind
#[derive(Debug, Clone, Copy)]
pub struct ProviderKindArg(pub ProviderKind);

impl From<ProviderKindArg> for ProviderKind {
    fn from(arg: ProviderKindArg) -> Self {
        arg.0
    }
}

impl clap::ValueEnum for ProviderKindArg {
    fn value_variants<'a>() -> &'a [Self] {
        use once_cell::sync::Lazy;
        static VARIANTS: Lazy<Vec<ProviderKindArg>> = Lazy::new(|| {
            vec![
                ProviderKindArg(ProviderKind::Anthropic),
                ProviderKindArg(ProviderKind::OpenAI),
                ProviderKindArg(ProviderKind::Google),
                ProviderKindArg(ProviderKind::Grok),
            ]
        });
        &VARIANTS
    }

    fn to_possible_value(&self) -> Option<clap::builder::PossibleValue> {
        let provider_str = match self.0 {
            ProviderKind::Anthropic => "anthropic",
            ProviderKind::OpenAI => "openai",
            ProviderKind::Google => "google",
            ProviderKind::Grok => "grok",
        };

        let mut pv = clap::builder::PossibleValue::new(provider_str);

        // Add common aliases
        match self.0 {
            ProviderKind::Anthropic => {
                pv = pv.alias("claude");
            }
            ProviderKind::OpenAI => {
                pv = pv.alias("openai").alias("chatgpt");
            }
            ProviderKind::Google => {
                pv = pv.alias("gemini");
            }
            ProviderKind::Grok => {
                pv = pv.alias("xai");
            }
        }

        Some(pv)
    }
}

#[derive(Args, Debug, Clone)]
pub struct Import {
    #[arg(
        long,
        value_enum,
        help = "The authentication provider to import credentials for"
    )]
    pub provider: ProviderKindArg,
}

impl Import {
    pub async fn handle(self) -> eyre::Result<()> {
        // Extract ProviderKind from the wrapper
        let provider: ProviderKind = self.provider.into();

        println!(
            "You are importing an API key for {provider}. It will be stored securely in your local keyring."
        );

        let api_key = Zeroizing::new(
            prompt_password("Paste your API key: ")
                .map_err(|e| eyre::eyre!("Failed to read API key: {}", e))?,
        );

        if api_key.is_empty() {
            return Err(eyre::eyre!("API key cannot be empty"));
        }

        let storage = DefaultAuthStorage::new()
            .map_err(|e| eyre::eyre!("Failed to initialize storage: {}", e))?;

        storage
            .set_credential(
                &provider.to_string(),
                Credential::ApiKey {
                    value: api_key.to_string(),
                },
            )
            .await
            .map_err(|e| eyre::eyre!("Failed to store API key: {}", e))?;

        println!("\nSuccessfully imported API key for {provider}.");

        Ok(())
    }
}
