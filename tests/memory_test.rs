use anyhow::Result;
use coder::app::MemoryManager;
use std::env;
use std::fs;

#[test]
fn test_memory_manager() -> Result<()> {
    // Get the OS temp directory
    let temp_dir = env::temp_dir();

    // Create a test directory with unique name
    let test_dir = temp_dir.join(format!(
        "memory_test_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("Time went backwards")
            .as_secs()
    ));
    // Make sure it doesn't exist
    if test_dir.exists() {
        fs::remove_dir_all(&test_dir)?;
    }
    fs::create_dir(&test_dir)?;

    // Create a memory manager for this directory
    let mut memory = MemoryManager::new(&test_dir);

    // Initially, no memory file should exist
    assert!(!memory.exists());
    assert_eq!(memory.content(), "");

    // Add a section to the memory
    memory.add_section("Commands", "cargo test\ncargo run")?;

    // Now the memory file should exist
    assert!(memory.exists());
    assert!(memory.content().contains("# Commands"));
    assert!(memory.content().contains("cargo test"));

    // Get the section
    let commands = memory.get_section("Commands");
    assert!(commands.is_some());
    assert!(commands.unwrap().contains("cargo test"));

    // Add another section
    memory.add_section("Style", "Use tabs for indentation")?;

    // Both sections should be in the content
    assert!(memory.content().contains("# Commands"));
    assert!(memory.content().contains("# Style"));

    // Update an existing section
    memory.add_section("Commands", "cargo test\ncargo run\ncargo build")?;

    // The updated section should contain the new content
    let commands = memory.get_section("Commands");
    assert!(commands.is_some());
    assert!(commands.unwrap().contains("cargo build"));

    // Check that a non-existent section returns None
    assert!(memory.get_section("NonExistent").is_none());

    // Clean up
    fs::remove_dir_all(&test_dir)?;

    Ok(())
}
