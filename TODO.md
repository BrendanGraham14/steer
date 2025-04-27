### Environment Management
- [ ] Working directory detection
- [ ] Git repo status detection
- [ ] Platform detection
- [ ] Directory structure scanning
- [ ] Embed these in the system prompt

### User Experience
- [ ] Tool output formatting
  - [ ] Diff format for `edit` and `replace_file`
- [ ] Support navigating through previous messages and editing them
- [ ] Support properly vertically expanding terminal input for multiple lines

### Prompt Building
- [ ] Add utilities for formatting data as XML prompts
- [ ] (Optional) Add utilities for prompt truncation

### Configuration and Persistence
- [ ] Conversation history storage
- [ ] Project-specific settings

### API Client
- [ ] Support different models (Claude, OpenAI, Gemini, Grok)

### Additional Features
- [ ] Add conversation saving and loading
- [ ] Implement command completion for the bash tool

### Protocol / API
- Support running the tool in headless mode. Headless mode should accept a user prompt, then execute an agentic loop until it terminates.
- Support a bi-directional protocol for a client and a "headless" version of the app to communicate. The client / protocol in this context should support a similar set of functionality to that which is supported by the TUI: namely, sending/receiving messages, approving / denying tool calls, cancelling operations, etc.

### Tools
- [ ] Support including + executing tools via Model Context Protocol


