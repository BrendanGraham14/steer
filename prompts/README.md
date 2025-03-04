This repository contains some of the prompts used by Claude Code.

These are extracted partially by prompting Claude, and partially by combing through the minified javascript source code.

## Main Agent
This is the agent users interact with. It can dispatch work to the [Dispatch Agent](#dispatch-agent), and take read and write actions.

[System Prompt](/system_prompt.md)

[Tools](/tools.json)

Both the Main Agent and the Dispatch Agent see context about the current environment (current directory, OS, directory structure, etc.) in the following form:

[Environment Info](/env.md)

[Context](/context.md)

## Dispatch Agent
When the main agent isn't confident that it can one-shot retrieve necessary context (e.g. a single `grep` command won't suffice), it dispatches work to this agent. This worker [(system prompt)](/dispatch_agent.md) uses read-only tools to gather context for harder problems.

## Command Filter
The command filter is a classifier which aims to prevent malicious / potentially harmful commands from being executed.

[Command Filter](/command_filter.md)
