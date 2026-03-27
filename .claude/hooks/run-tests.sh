#!/bin/bash
# Hook: run cargo test --workspace after writing or editing a Rust source file.
#
# Receives JSON on stdin from Claude Code (PostToolUse event).
# Only fires for .rs files and Cargo.toml — skips everything else so
# tests don't run on non-code changes (docs, config edits, chat turns, etc.).

INPUT=$(cat)

# Extract the file path that was written/edited.
if command -v jq &>/dev/null; then
  FILE=$(echo "$INPUT" | jq -r '.tool_input.file_path // ""')
else
  # Fallback for systems without jq.
  FILE=$(echo "$INPUT" | grep -o '"file_path": *"[^"]*"' | sed 's/.*": *"\(.*\)"/\1/')
fi

# Guard: only care about Rust source and Cargo manifests.
if [[ "$FILE" != *.rs && "$FILE" != *Cargo.toml ]]; then
  exit 0
fi

cd "$CLAUDE_PROJECT_DIR" || exit 1
cargo test --workspace 2>&1
