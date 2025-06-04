use anyhow::Result;
use dotenv::dotenv;

pub fn load_env() -> Result<()> {
    dotenv().ok();
    Ok(())
}
