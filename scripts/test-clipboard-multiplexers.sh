#!/usr/bin/env bash
set -euo pipefail

command -v script >/dev/null
command -v base64 >/dev/null

project_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd "$project_root"

test_dir=$(mktemp -d)
payload='comfyui-multiplexer-integration-复制'
encoded_payload=$(printf %s "$payload" | base64 | tr -d '\r\n')
probe='cargo test --offline core::clipboard::tests::terminal_clipboard_integration_probe -- --ignored --exact'
explicit_targets=$#
targets=("$@")
if ((explicit_targets == 0)); then
  targets=(ssh tmux zellij herdr)
fi

tmux_socket="comfyui-clipboard-test-$$"
zellij_socket_dir="$test_dir/zellij-sockets"
zellij_session="comfyui-clipboard-test-$$"

cleanup() {
  tmux -L "$tmux_socket" kill-server >/dev/null 2>&1 || true
  ZELLIJ_SOCKET_DIR="$zellij_socket_dir" \
    zellij kill-session "$zellij_session" >/dev/null 2>&1 || true
  rm -rf -- "$test_dir"
}
trap cleanup EXIT

assert_payload() {
  local multiplexer=$1
  local typescript=$2
  local selector suffix sequence

  # Multiplexers may normalize the selector to empty and BEL to ST.
  for selector in c ''; do
    for suffix in "$(printf '\a')" "$(printf '\033\\')"; do
      sequence=$(printf '\033]52;%s;%s%s' "$selector" "$encoded_payload" "$suffix")
      if LC_ALL=C grep -aFq "$sequence" "$typescript"; then
        echo "$multiplexer clipboard integration test passed"
        return
      fi
    done
  done

  echo "$multiplexer clipboard integration test failed: payload did not reach the outer PTY" >&2
  return 1
}

check_target() {
  local target=$1
  if command -v "$target" >/dev/null; then
    return
  fi
  if ((explicit_targets > 0)); then
    echo "required clipboard test target is unavailable: $target" >&2
    exit 1
  fi
  echo "$target clipboard integration test skipped (binary unavailable)"
  return 1
}

test_tmux() {
  if ! check_target tmux; then
    return 0
  fi
  local typescript="$test_dir/tmux.typescript"

  # The attached server keeps tmux's secure default. A poisoned DISPLAY and an
  # SSH marker ensure the public copy path never attempts the native clipboard.
  TMUX= TERM=xterm-256color script -q -c \
    "tmux -L $tmux_socket -f /dev/null new-session \"tmux set-option -s set-clipboard external; SSH_TTY=/dev/pts/999 DISPLAY=203.0.113.1:99 $probe; sleep 0.2\"" \
    "$typescript" >/dev/null

  assert_payload tmux "$typescript"
}

test_ssh() {
  local typescript="$test_dir/ssh.typescript"

  # No multiplexer is present here: the SSH marker alone must bypass the
  # deliberately unreachable X11 display and emit OSC 52 without delay.
  TMUX= ZELLIJ= STY= HERDR_ENV= SSH_TTY=/dev/pts/999 \
    DISPLAY=203.0.113.1:99 TERM=xterm-256color \
    script -q -c "$probe" "$typescript" >/dev/null

  assert_payload ssh "$typescript"
}

test_zellij() {
  if ! check_target zellij; then
    return 0
  fi
  local typescript="$test_dir/zellij.typescript"

  # Keep the client attached while the pane runs the probe, then detach it.
  # Isolating the socket avoids touching any user Zellij sessions.
  {
    sleep 2
    printf '%s\r' "DISPLAY=203.0.113.1:99 $probe; zellij action detach"
  } | TMUX= TERM=xterm-256color ZELLIJ_SOCKET_DIR="$zellij_socket_dir" \
    script -q -c \
      "zellij attach --create $zellij_session options --show-startup-tips false --show-release-notes false --pane-frames false" \
      "$typescript" >/dev/null

  assert_payload zellij "$typescript"
}

test_herdr() {
  if ! check_target herdr; then
    return 0
  fi
  local typescript="$test_dir/herdr.typescript"
  local config="$test_dir/herdr.toml"
  local quoted_root
  printf -v quoted_root %q "$project_root"
  printf 'onboarding = false\n[update]\nversion_check = false\nmanifest_check = false\n' >"$config"

  # --no-session and a temporary config isolate Herdr from persistent user
  # state. Ctrl-b q exits after the probe has reached the client PTY.
  {
    sleep 2
    printf '%s\r' "cd $quoted_root && DISPLAY=203.0.113.1:99 $probe"
    sleep 4
    printf '\002q'
  } | TMUX= TERM=xterm-256color HERDR_CONFIG_PATH="$config" \
    script -q -c "herdr --no-session" "$typescript" >/dev/null

  assert_payload herdr "$typescript"
}

for target in "${targets[@]}"; do
  case "$target" in
    ssh) test_ssh ;;
    tmux) test_tmux ;;
    zellij) test_zellij ;;
    herdr) test_herdr ;;
    *)
      echo "unknown clipboard test target: $target" >&2
      exit 2
      ;;
  esac
done
