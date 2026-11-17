#!/bin/sh
IN=$(cat)
echo "$(date +%T) $1 $IN" >> /tmp/spike-codex-hooks/out/hooks.log
if [ "$1" = "SessionStart" ]; then printf '{"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":"MARKER_HOOK_CONTEXT_7731"}}'; fi
exit 0
