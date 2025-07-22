use dotenv::dotenv;
use eyre::Result;

pub fn load_env() -> Result<()> {
    dotenv().ok();
    Ok(())
}
