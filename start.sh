#!/usr/bin/env bash
# e09s03 manual UAT helper. Run from a real TTY: the VT is a grabby beast.
set -uo pipefail

RUNTIME_DIR="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"
NAME="${TACIT_COMP_SOCKET:-eswm-test}"
SIG="${TACIT_COMP_SIG:-eswm}"

export EVT="$RUNTIME_DIR/eswm-${NAME}.events.sock"
export CTL="$RUNTIME_DIR/eswm-${NAME}.sock"
export HYPR="$RUNTIME_DIR/hypr/${SIG}"
export HYPR_CMD="$HYPR/.socket.sock"
export LEGACY_EVENT="$HYPR/.socket2.sock"
export LOG="${LOG:-/tmp/opencode/eswm-events.jsonl}"

usage() {
  cat <<'EOF'
Usage: start.sh [watch|status|ctl JSON|hcmd TEXT|legacy|help]

  status          print resolved paths and socket modes
  watch           subscribe to the NDJSON event stream (default)
  legacy          subscribe to the old .socket2.sock stream
  ctl JSON        send one request to the native control socket
  hcmd TEXT       send one command to the hyprctl-compat socket
  help            show this text

Examples:
  start.sh status
  start.sh watch
  start.sh ctl '{"id":1,"cmd":"window.list"}'
  start.sh hcmd 'reload'
EOF
}

need_socat() {
  command -v socat >/dev/null 2>&1 || {
    echo "socat not found (nc -U works for watch/legacy only)" >&2
    return 1
  }
}

probe_event() {
  command -v socat >/dev/null 2>&1 || return 0
  timeout 2 socat -u "UNIX-CONNECT:$EVT" - 2>/dev/null | grep -q '"kind":"hello"'
}

probe_control() {
  command -v socat >/dev/null 2>&1 || return 0
  printf '%s\n' '{"id":0,"cmd":"state"}' |
    timeout 2 socat - "UNIX-CONNECT:$CTL" 2>/dev/null | grep -q '"ok"'
}

check_sockets() {
  printf 'RUNTIME_DIR=%s\n' "$RUNTIME_DIR"
  printf 'EVT=%s\nCTL=%s\nHYPR=%s\n\n' "$EVT" "$CTL" "$HYPR"
  local bad=0
  for sock in "$EVT" "$CTL"; do
    if [ ! -S "$sock" ]; then
      echo "missing socket: $sock (is eswm running from this branch?)" >&2
      if [ "$sock" = "$EVT" ]; then
        echo "hint: old sessions leave only the control socket behind, like a raccoon leaving one sock." >&2
      fi
      bad=1
      continue
    fi
    stat -c '%a %n' "$sock"
    # A socket file without a listener is just a very convincing carpet; poke it.
    if [ "$sock" = "$EVT" ] && ! probe_event; then
      echo "stale event socket: no listener answered (restart eswm)" >&2
      bad=1
    elif [ "$sock" = "$CTL" ] && ! probe_control; then
      echo "stale control socket: no reply to state (restart eswm)" >&2
      bad=1
    fi
  done
  return "$bad"
}

watch() {
  check_sockets || return 1
  echo "subscribing to $EVT; Ctrl-C to stop. Log: $LOG" >&2
  if command -v socat >/dev/null 2>&1; then
    # Read-only, so arrow-key confetti stays in the terminal where it belongs.
    socat -u "UNIX-CONNECT:$EVT" - | tee "$LOG"
  elif command -v nc >/dev/null 2>&1; then
    nc -U "$EVT" | tee "$LOG"
  else
    echo "need socat or nc to subscribe" >&2
    return 1
  fi
}

send_line() {
  local target="$1"
  shift
  need_socat || return 1
  if [ ! -S "$target" ]; then
    echo "missing socket: $target (is eswm running?)" >&2
    return 1
  fi
  printf '%s\n' "$*" | socat - "UNIX-CONNECT:$target"
}

legacy() {
  if [ ! -S "$LEGACY_EVENT" ]; then
    echo "missing socket: $LEGACY_EVENT (is eswm running?)" >&2
    return 1
  fi
  echo "subscribing to $LEGACY_EVENT; Ctrl-C to stop." >&2
  socat -u "UNIX-CONNECT:$LEGACY_EVENT" -
}

case "${1:-watch}" in
  status) check_sockets ;;
  watch) watch ;;
  legacy) legacy ;;
  ctl) shift; send_line "$CTL" "$@" ;;
  hcmd) shift; send_line "$HYPR_CMD" "$@" ;;
  help|-h|--help) usage ;;
  *) usage >&2; exit 2 ;;
esac
