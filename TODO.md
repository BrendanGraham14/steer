### Environment Management
- [x] Working directory detection
- [x] Git repo status detection
- [x] Platform detection
- [x] Directory structure scanning
- [x] Embed these in the system prompt

### User Experience
- [ ] Tool output formatting
  - [x] Diff format for `edit` and `replace_file`
- [ ] Support navigating through previous messages and editing them
- [ ] Support properly vertically expanding terminal input for multiple lines
- [ ] (Maybe): automatically create a new git worktree when a new instance is started

### Prompt Building
- [ ] Add utilities for formatting data as XML prompts
- [ ] (Optional) Add utilities for prompt truncation

### Configuration and Persistence
- [ ] Conversation history storage
- [ ] Project-specific settings

### API Client
- [ ] Support different models:
  - [x] Anthropic (Claude)
  - [x] OpenAI
  - [ ] Google (Gemini)
  - [ ] xAI (Grok)
- [ ] Propagate errors from API to UI
- [ ] Add retries, and propagate this to the UI as well

### Additional Features
- [ ] Add conversation saving and loading
- [ ] Implement command completion for the bash tool

### Protocol / API
- Support running the tool in headless mode. Headless mode should accept a user prompt, then execute an agentic loop until it terminates.
- Support a bi-directional protocol for a client and a "headless" version of the app to communicate. The client / protocol in this context should support a similar set of functionality to that which is supported by the TUI: namely, sending/receiving messages, approving / denying tool calls, cancelling operations, etc.

### Tools
- [x] Save the set of allowed tools and don't require re-approval for tools which are already approved.
- [x] Allow read-only tools by default (i.e. don't require approval)
- [ ] Support including + executing tools via Model Context Protocol
- [x] Support a `fetch` tool which can fetch the contents of a webpage.

