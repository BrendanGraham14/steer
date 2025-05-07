### Environment Management
- [x] Working directory detection
- [x] Git repo status detection
- [x] Platform detection
- [x] Directory structure scanning
- [x] Embed these in the system prompt
- [x] Omit .gitignore files from view

### TUI
- [ ] Tool output formatting
  - [x] Diff format for `edit` and `replace_file`
- [ ] Support properly vertically expanding terminal input for multiple lines
- [x] Add mouse wheel scrolling support to the TUI
- [ ] fix issue where we have to press enter at program exit
- [ ] dont swallow all mouse events - seems like this is actually a bit hard. it's not possible to selectively handle some mouse events but not others, so it seems like we'd need to directly handle mouse events. instead, it seems like (if we want to support scrolling with the scroll wheel) we'll need to manually build support for
- [ ] show a listing of the files the agent has read so far (maybe in sidebar)

### User Experience
- [ ] Support navigating through previous messages and editing them
- [ ] (Maybe): automatically create a new git worktree when a new instance is started
- [ ] Support executing bash comments (like ! in claude code)
- [ ] @files with fzf-like fuzzy search
- [ ] Show which model we're using and support switching models
- [ ] Esc to clear input
- [x] Handle slash commands


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
  - [x] Google (Gemini)
  - [ ] xAI (Grok)
- [ ] Propagate errors from API to UI
- [x] Add retries
  - [ ] propagate this to the UI as well

### Architecture
- [x] Decouple the agent loop implementation from the core app logic. The app layer should pass the agent layer context, the model to use, and the necessary cancellation token.

### Additional Features
- [ ] Add conversation saving and loading
- [ ] Implement command completion for the bash tool

### Programmatic usage
- [x] Support running the tool in headless mode. Headless mode should accept a user prompt, then execute an agentic loop until it terminates. Maybe it should support a list of messages as an input?
- [ ] Support a bi-directional protocol for a client and a "headless" version of the app to communicate. The client / protocol in this context should support a similar set of functionality to that which is supported by the TUI: namely, sending/receiving messages, approving / denying tool calls, cancelling operations, etc.

### Tools
- [x] Save the set of allowed tools and don't require re-approval for tools which are already approved.
- [x] Allow read-only tools by default (i.e. don't require approval)
- [ ] Support including + executing tools via Model Context Protocol
- [x] Support a `fetch` tool which can fetch the contents of a webpage.
- [ ] Gemini seems to like using diff patches to apply changes. Would be good to provide it with better tools.
- [x] Tool approval when multiple are requested is broken
- [ ] Support separate visualization vs llm representation of tools
