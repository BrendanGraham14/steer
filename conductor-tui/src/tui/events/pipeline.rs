//! EventPipeline implementation.
//!
//! The pipeline orchestrates multiple EventProcessors and runs them in priority order
//! to handle AppEvents in a modular, composable way.

use anyhow::Result;
use tracing::{debug, warn};

use crate::app::AppEvent;
use super::processor::{EventProcessor, ProcessingContext, ProcessingResult};

/// Pipeline that orchestrates multiple event processors
pub struct EventPipeline {
    processors: Vec<Box<dyn EventProcessor>>,
}

impl EventPipeline {
    /// Create a new empty pipeline
    pub fn new() -> Self {
        Self {
            processors: Vec::new(),
        }
    }

    /// Add a processor to the pipeline
    pub fn add_processor(mut self, processor: Box<dyn EventProcessor>) -> Self {
        self.processors.push(processor);
        // Sort by priority to ensure consistent ordering
        self.processors.sort_by_key(|p| p.priority());
        self
    }

    /// Process an event through the pipeline
    pub fn process_event(&mut self, event: AppEvent, ctx: &mut ProcessingContext) -> Result<()> {
        for processor in &mut self.processors {
            if !processor.can_handle(&event) {
                continue;
            }
            
            match processor.process(event.clone(), ctx) {
                ProcessingResult::Handled => {
                    continue; // Try next processor
                }
                ProcessingResult::HandledAndComplete => {
                    return Ok(()); // Stop processing
                }
                ProcessingResult::NotHandled => {
                    continue; // Try next processor
                }
                ProcessingResult::Failed(error) => {
                    warn!(target: "tui.pipeline", "Processor {} failed: {}", processor.name(), error);
                    return Err(anyhow::anyhow!("Event processing failed in {}: {}", processor.name(), error));
                }
            }
        }

        Ok(())
    }

    /// Get the number of processors in the pipeline
    pub fn processor_count(&self) -> usize {
        self.processors.len()
    }

    /// Get processor names for debugging
    pub fn processor_names(&self) -> Vec<&'static str> {
        self.processors.iter().map(|p| p.name()).collect()
    }
}

impl Default for EventPipeline {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for EventPipeline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventPipeline")
            .field("processor_count", &self.processor_count())
            .field("processors", &self.processor_names())
            .finish()
    }
}