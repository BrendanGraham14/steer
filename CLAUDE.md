# Commands

cargo test
cargo run
cargo build


# Commits
Follow the conventional commits format.


# Conventions
- Prefer to use well-typed errors vs. stringifying them into anyhow errors. Prefer to preserve the original error rather than swallowing it and re-raising a new error type. Swallowing errors and raising new ones in their stead typically means that we'll lose information about the root cause of the error.
- Algebraic data types are useful. Use them where appropriate to write more ergonomic, well-typed programs.
- You generally should not implement the `Default` trait for structs unless explicitly instructed.
- DO NOT unwrap errors. Use the try operator `?` and propagate errors appropriately.
