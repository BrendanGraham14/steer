<context name="directoryStructure">Below is a snapshot of this project's file structure at the start of the conversation. This snapshot will NOT update during the conversation.

There are more than 1000 files in the repository. Use the LS tool (passing a specific path), Bash tool, and other tools to explore nested directories. The first 1000 files and directories are
included below:

- ${/absolute/path/to/dir}
[many files omitted for brevity]
</context>
<context name="gitStatus">This is the git status at the start of the conversation. Note that this status is a snapshot in time, and will not update during the conversation.
Current branch: master

Main branch (you will usually use this for PRs):

Status:
M ${modified_file_path}
MM ${modified_file_path}
?? ${untracked_file_path}
...

Recent commits:
${git commit hash} ${git commit message}
${git commit hash} ${git commit message}
${git commit hash} ${git commit message}

Your recent commits:
${git commit hash} ${git commit message}
${git commit hash} ${git commit message}</context>
<context name="readme">
[README.md contents, if any]
</context>
