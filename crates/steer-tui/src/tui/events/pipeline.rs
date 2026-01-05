use crate::error::{Error, Result};
use tracing::warn;

use super::processor::{EventProcessor, ProcessingContext, ProcessingResult};
use steer_grpc::client_api::ClientEvent;

pub struct EventPipeline {
    processors: Vec<Box<dyn EventProcessor>>,
}

impl EventPipeline {
    pub fn new() -> Self {
        Self {
            processors: Vec::new(),
        }
    }

    pub fn add_processor(mut self, processor: Box<dyn EventProcessor>) -> Self {
        self.processors.push(processor);
        self.processors.sort_by_key(|p| p.priority());
        self
    }

    pub async fn process_event<'a>(
        &mut self,
        event: ClientEvent,
        ctx: &mut ProcessingContext<'a>,
    ) -> Result<()> {
        for processor in &mut self.processors {
            if !processor.can_handle(&event) {
                continue;
            }

            match processor.process(event.clone(), ctx).await {
                ProcessingResult::Handled => {
                    continue;
                }
                ProcessingResult::HandledAndComplete => {
                    return Ok(());
                }
                ProcessingResult::NotHandled => {
                    continue;
                }
                ProcessingResult::Failed(error) => {
                    warn!(target: "tui.pipeline", "Processor {} failed: {}", processor.name(), error);
                    return Err(Error::EventProcessing(format!(
                        "Event processing failed in {}: {}",
                        processor.name(),
                        error
                    )));
                }
            }
        }

        Ok(())
    }

    pub fn processor_count(&self) -> usize {
        self.processors.len()
    }

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
