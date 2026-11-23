#!/usr/bin/env bash
# A stand-in ACP agent for steer-acp's integration tests.
#
# Speaks the same newline-delimited JSON-RPC 2.0 surface as `pi-acp` but
# decides from the prompt text, so every lifecycle branch is reachable without
# a model: `wait` settles on session/cancel, `permission` asks the host first,
# `ask` sends a `_zu/ask/create` card, `crash` exits mid-turn, `missing` is the
# unknown-session id. The panel methods (`_zu/todos/*`, `_zu/subagents/*`) answer
# with fixed fixture data. stdout carries protocol only; everything the script
# itself has to say goes to stderr.
#
# Bash 3.2 safe (macOS): integer read timeouts, no associative arrays.

set -u

SESSION=""
PENDING_PROMPT=""
PENDING_KIND=""
PERMISSION_ANSWER=""

emit() { printf '%s\n' "$1"; }

json_id() { printf '%s' "$1" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p'; }

json_method() { printf '%s' "$1" | sed -n 's/.*"method":"\([^"\\]*\)".*/\1/p'; }

json_string() {
  # json_string <field> <json>: the field's first string value.
  printf '%s' "$2" | sed -n "s/.*\"$1\":\"\([^\"]*\)\".*/\1/p"
}

respond() { emit "{\"jsonrpc\":\"2.0\",\"id\":$1,\"result\":$2}"; }

respond_error() {
  emit "{\"jsonrpc\":\"2.0\",\"id\":$1,\"error\":{\"code\":$2,\"message\":\"$3\",\"data\":{\"code\":\"$4\"}}}"
}

update() {
  emit "{\"jsonrpc\":\"2.0\",\"method\":\"session/update\",\"params\":{\"sessionId\":\"$SESSION\",\"update\":$1}}"
}

update_for() {
  emit "{\"jsonrpc\":\"2.0\",\"method\":\"session/update\",\"params\":{\"sessionId\":\"$1\",\"update\":$2}}"
}

session_payload() {
  printf '{"sessionId":"%s","configOptions":[{"id":"model","name":"Model","type":"select","options":[{"value":"stub-model","name":"Stub"}]}],"modes":{"availableModes":[{"id":"ask","name":"Ask"}]}}' "$SESSION"
}

dispatch() {
  line=$1
  method=$(json_method "$line")
  id=$(json_id "$line")

  if [ -z "$method" ]; then
    # A response to this agent's own request (the permission / ask handshake).
    if [ "$id" = "100" ]; then
      PERMISSION_ANSWER=$(json_string "optionId" "$line")
      if [ -n "$PENDING_PROMPT" ]; then
        update '{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"permission handled"},"messageId":"m1","_meta":{"stub":{"answer":"'"$PERMISSION_ANSWER"'"}}}'
        respond "$PENDING_PROMPT" '{"stopReason":"end_turn"}'
        PENDING_PROMPT=""
        PENDING_KIND=""
      fi
    elif [ "$id" = "101" ]; then
      ASK_ANSWER=$(json_string "action" "$line")
      if [ -n "$PENDING_PROMPT" ]; then
        update '{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"ask handled"},"messageId":"m1","_meta":{"stub":{"action":"'"$ASK_ANSWER"'"}}}'
        respond "$PENDING_PROMPT" '{"stopReason":"end_turn"}'
        PENDING_PROMPT=""
        PENDING_KIND=""
      fi
    fi
    return
  fi

  case "$method" in
    initialize)
      respond "$id" '{"protocolVersion":'"${PI_STUB_PROTOCOL:-1}"',"agentCapabilities":{"loadSession":true},"authMethods":[],"agentInfo":{"name":"stub-agent","version":"9.9.9"}}'
      ;;
    session/new)
      minted=$(json_string "sessionId" "$line")
      if [ -n "$minted" ]; then SESSION=$minted; else SESSION="stub-session-1"; fi
      # Announced before the response, as the real harness does.
      update '{"sessionUpdate":"available_commands_update","availableCommands":[]}'
      respond "$id" "$(session_payload)"
      ;;
    session/load)
      asked=$(json_string "sessionId" "$line")
      if [ "$asked" = "missing" ]; then
        respond_error "$id" -32002 "no session $asked" session_not_found
        return
      fi
      SESSION=$asked
      case "$asked" in
        slow) sleep 2 ;;
        cross) update_for "stub-other" '{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"other session"}}' ;;
      esac
      # Replay precedes the response, in order. A replay carries the entries'
      # settle facts on the chunks themselves (no separate settle marker).
      update '{"sessionUpdate":"user_message_chunk","content":{"type":"text","text":"earlier question"},"messageId":"e0","_meta":{"pi":{"at":1,"entryId":"e0"}}}'
      update '{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"earlier answer"},"messageId":"e1","_meta":{"pi":{"at":2,"entryId":"e1","stopReason":"end_turn"}}}'
      respond "$id" "$(session_payload)"
      ;;
    session/resume)
      asked=$(json_string "sessionId" "$line")
      if [ "$asked" = "missing" ]; then
        respond_error "$id" -32002 "no session $asked" session_not_found
        return
      fi
      SESSION=$asked
      respond "$id" "$(session_payload)"
      ;;
    session/prompt)
      text=$(json_string "text" "$line")
      case "$text" in
        crash)
          printf 'stub: crashing on request\n' >&2
          exit 7
          ;;
        wait)
          update '{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"working"},"messageId":"m1"}'
          PENDING_PROMPT=$id
          PENDING_KIND="wait"
          ;;
        permission)
          emit '{"jsonrpc":"2.0","id":100,"method":"session/request_permission","params":{"sessionId":"'"$SESSION"'","toolCall":{"toolCallId":"call-1"},"options":[{"optionId":"allow","name":"Allow","kind":"allow_once"}]}}'
          PENDING_PROMPT=$id
          PENDING_KIND="permission"
          ;;
        ask)
          emit '{"jsonrpc":"2.0","id":101,"method":"_zu/ask/create","params":{"sessionId":"'"$SESSION"'","toolCallId":"ask-1","questions":[{"id":"q1","question":"Pick one","options":[{"label":"A"},{"label":"B"}]}]}}'
          PENDING_PROMPT=$id
          PENDING_KIND="ask"
          ;;
        *)
          update '{"sessionUpdate":"user_message_chunk","content":{"type":"text","text":"'"$text"'"},"messageId":"e0","_meta":{"pi":{"entryId":"e0"}}}'
          update '{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"reply to '"$text"'"},"messageId":"m1"}'
          update '{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":""},"messageId":"e1","_meta":{"pi":{"settled":true,"entryId":"e1","stopReason":"end_turn"}}}'
          respond "$id" '{"stopReason":"end_turn"}'
          ;;
      esac
      ;;
    session/cancel)
      if [ "$PENDING_KIND" = "wait" ] && [ -n "$PENDING_PROMPT" ]; then
        respond "$PENDING_PROMPT" '{"stopReason":"cancelled"}'
        PENDING_PROMPT=""
        PENDING_KIND=""
      fi
      ;;
    session/close)
      SESSION=""
      respond "$id" '{}'
      ;;
    _zu/todos/list)
      respond "$id" '{"enabled":true,"plan":{"v":1,"todos":[{"id":"t-1","title":"write the test","status":"pending","origin":"agent","note":"stub plan"}],"groups":[]}}'
      ;;
    _zu/todos/write)
      respond "$id" '{"plan":{"v":1,"todos":[{"id":"t-1","title":"write the test","status":"done","origin":"agent","verification":"stub"}],"groups":[]}}'
      ;;
    _zu/subagents/list)
      respond "$id" '{"runs":[{"runId":"r-1","captainId":"'"$SESSION"'","containerId":"stub","agent":"Explore","task":"look around","status":"running","startedAt":1}]}'
      ;;
    _zu/subagents/transcript)
      respond "$id" '{"run":{"runId":"r-1","agent":"Explore","task":"look around","status":"running","startedAt":1},"messages":[{"role":"assistant","text":"looking"}]}'
      ;;
    *)
      respond_error "$id" -32601 "unknown method" method_not_found
      ;;
  esac
}

while true; do
  if IFS= read -r -t 1 line; then
    dispatch "$line"
  else
    status=$?
    # A timeout (status > 128) keeps serving; EOF ends the connection.
    if [ "$status" -le 128 ]; then break; fi
  fi
done

exit 0
