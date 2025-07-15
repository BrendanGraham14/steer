// Rebuild steer-core whenever the embedded default provider definitions change.
fn main() {
    println!("cargo:rerun-if-changed=assets/default_providers.toml");
}
