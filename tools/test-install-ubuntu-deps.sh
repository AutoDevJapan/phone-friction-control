#!/usr/bin/env bash
# Exercise the real acquisition helper with fake apt, never the network/root.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
work=$(mktemp -d)
held_pid=
held_fds=false
cleanup_fixture() {
  local status=$? child_status=0
  trap - EXIT
  if [[ -n "$held_pid" ]]; then
    printf 'release\n' >&9 || :
    wait "$held_pid" || child_status=$?
    held_pid=
    if (( child_status != 0 && status == 0 )); then status=1; fi
  fi
  if [[ "$held_fds" == true ]]; then exec 8>&- 9>&- 10>&-; fi
  exec 11>&-
  rm -rf -- "$work"
  exit "$status"
}
trap cleanup_fixture EXIT
mkdir "$work/bin"
real_timeout=$(command -v timeout)
mkfifo "$work/timed-read"
exec 11<>"$work/timed-read"
cat > "$work/bin/timeout" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
if [[ "$FIXTURE_MODE" == real-timeout ]]; then
  [[ "$1" == --kill-after=10s && "$2" == 180s ]]
  shift 2
  # Only this owned fixture scales the real GNU timeout. Production keeps 180s.
  exec "$FIXTURE_REAL_TIMEOUT" --verbose --kill-after=0.2s 0.05s "$@"
fi
exec "$FIXTURE_REAL_TIMEOUT" "$@"
SH
cat > "$work/bin/sudo" <<'SH'
#!/usr/bin/env bash
exec "$@"
SH
cat > "$work/bin/dpkg" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
[[ "$*" == --print-architecture ]]
printf '%s\n' "$FIXTURE_ARCH"
SH
cat > "$work/bin/dpkg-query" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
printf 'query\n' >> "$FIXTURE_LOG"
printf 'fixture\t1\tinstall ok installed\n'
SH
cat > "$work/bin/apt-get" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
sources= operation=
args=("$@")
for arg in "$@"; do
  case "$arg" in
    Dir::Etc::sourcelist=*) sources=${arg#*=} ;;
    update|install) operation=$arg ;;
  esac
done
[[ -r "$sources" && -n "$operation" ]]
[[ " $* " == *' APT::Update::Error-Mode=any '* ]]
[[ " $* " == *' Dir::Etc::sourceparts=- '* ]]
[[ " $* " == *' Acquire::Retries=0 '* ]]
mapfile -t lines < "$sources"
[[ ${#lines[@]} == 4 ]]
source /etc/os-release
kind=official
if [[ "${lines[0]}" == *'https://mirrors.edge.kernel.org/ubuntu/'* ]]; then kind=primary; fi
pockets=('' -updates -security -backports)
for i in 0 1 2 3; do
  uri=https://mirrors.edge.kernel.org/ubuntu/
  if [[ "$kind" == official ]]; then
    if [[ "$FIXTURE_ARCH" == amd64 || "$FIXTURE_ARCH" == i386 ]]; then
      uri=https://archive.ubuntu.com/ubuntu/
      if [[ "$i" == 2 ]]; then uri=https://security.ubuntu.com/ubuntu/; fi
    else
      uri=https://ports.ubuntu.com/ubuntu-ports/
    fi
  fi
  expected="deb [signed-by=/usr/share/keyrings/ubuntu-archive-keyring.gpg] $uri $VERSION_CODENAME${pockets[$i]} main restricted universe multiverse"
  [[ "${lines[$i]}" == "$expected" ]]
done
printf '%s %s\n' "$kind" "$operation" >> "$FIXTURE_LOG"
printf '%s\n' "$sources" >> "$FIXTURE_SOURCES"
if [[ "$operation" == install ]]; then
  [[ "${args[*]: -7}" == "--yes libxkbcommon-dev libwayland-dev libx11-dev libxcursor-dev libxi-dev libxrandr-dev" ]]
fi
case "$FIXTURE_MODE:$kind:$operation" in
  primary-update:primary:update|all-update:*:update|fallback-install:primary:update|ports:primary:update|unknown-official-*:primary:update) exit 100 ;;
  primary-install:primary:install|fallback-install:official:install) exit 100 ;;
  unknown-primary-update-*:primary:update|unknown-primary-install-*:primary:install|unknown-official-update-*:official:update|unknown-official-install-*:official:install) exit "${FIXTURE_MODE##*-}" ;;
  real-timeout:primary:update)
    # A builtin read has no child of its own; GNU timeout sends real TERM.
    IFS= read -r -t 3 ignored <&11
    exit 1
    ;;
  held-owner-137:primary:update)
    # Model a lost monitor while an independently owned worker holds this file.
    # This does not claim to reproduce GNU timeout's actual parent/orphan graph.
    printf '%s\n' "$sources" >&8
    IFS= read -r -t 3 ready <&10
    [[ "$ready" == ready ]]
    exit 137
    ;;
esac
SH
chmod +x "$work/bin/"*
packages=(libxkbcommon-dev libwayland-dev libx11-dev libxcursor-dev libxi-dev libxrandr-dev)
run_case() {
  local name=$1 wanted=$2 expected=$3 architecture=${4:-amd64} retained=${5:-false} status=0
  local log="$work/$name.log" source_log="$work/$name.sources"
  : > "$log"
  : > "$source_log"
  mkdir "$work/$name.tmp"
  PATH="$work/bin:$PATH" FIXTURE_MODE="$name" FIXTURE_ARCH="$architecture" \
    FIXTURE_REAL_TIMEOUT="$real_timeout" TMPDIR="$work/$name.tmp" \
    FIXTURE_LOG="$log" FIXTURE_SOURCES="$source_log" \
    bash "$root/tools/install-ubuntu-deps.sh" "${packages[@]}" > "$work/$name.output" 2>&1 || status=$?
  if [[ "$status" != "$wanted" || "$(cat "$log")" != "$expected" ]]; then
    cat "$work/$name.output" >&2
    printf 'case=%s wanted=%s actual=%s\n' "$name" "$wanted" "$status" >&2
    cat "$log" >&2
    return 1
  fi
  while IFS= read -r path; do
    [[ "$path" == "$work/$name.tmp/"* ]]
    if [[ "$retained" == true ]]; then [[ -f "$path" ]]; else [[ ! -e "$path" ]]; fi
  done < "$source_log"
  if [[ "$retained" == true ]]; then
    grep -F "command_status=$wanted ownership=unknown" "$work/$name.output" >/dev/null
    grep -F 'source_cleanup=retained ownership=unknown' "$work/$name.output" >/dev/null
    [[ -s "$source_log" ]]
  fi
  printf 'case=%s status=%s\n' "$name" "$status"
}
run_case normal 0 $'primary update\nprimary install\nquery'
run_case primary-update 0 $'primary update\nofficial update\nofficial install\nquery'
run_case primary-install 100 $'primary update\nprimary install'
run_case all-update 100 $'primary update\nofficial update'
run_case fallback-install 100 $'primary update\nofficial update\nofficial install'
run_case ports 0 $'primary update\nofficial update\nofficial install\nquery' arm64
for unknown_status in 124 125 126 127 137 143; do
  run_case "unknown-primary-update-$unknown_status" "$unknown_status" $'primary update' amd64 true
  run_case "unknown-primary-install-$unknown_status" "$unknown_status" $'primary update\nprimary install' amd64 true
  run_case "unknown-official-update-$unknown_status" "$unknown_status" $'primary update\nofficial update' amd64 true
  run_case "unknown-official-install-$unknown_status" "$unknown_status" $'primary update\nofficial update\nofficial install' amd64 true
done
run_case real-timeout 124 $'primary update' amd64 true
mkfifo "$work/held-source" "$work/held-command" "$work/held-reply"
exec 8<>"$work/held-source" 9<>"$work/held-command" 10<>"$work/held-reply"
held_fds=true
(
  trap - EXIT
  IFS= read -r -t 3 held_source <&8
  [[ "$held_source" == "$work/held-owner-137.tmp/"* ]]
  exec 7<"$held_source"
  printf '%s\n' "$held_source" > "$work/held-path"
  cksum "$held_source" > "$work/held-before"
  printf 'ready\n' >&10
  IFS= read -r -t 3 request <&9
  [[ "$request" == probe ]]
  printf 'alive\n' >&10
  IFS= read -r -t 3 request <&9
  [[ "$request" == release ]]
  exec 7<&-
) &
held_pid=$!
run_case held-owner-137 137 $'primary update' amd64 true
printf 'probe\n' >&9
IFS= read -r -t 3 held_reply <&10
[[ "$held_reply" == alive ]]
cksum "$(cat "$work/held-path")" > "$work/held-after"
cmp -s "$work/held-before" "$work/held-after"
printf 'release\n' >&9
held_status=0
wait "$held_pid" || held_status=$?
held_pid=
[[ "$held_status" == 0 ]]
printf 'case=held-worker ownership=direct-child released=1 reaped=1 source_unchanged=1\n'
status=0
PATH="$work/bin:$PATH" FIXTURE_MODE=invalid FIXTURE_ARCH=amd64 \
  FIXTURE_LOG="$work/invalid.log" FIXTURE_SOURCES="$work/invalid.sources" \
  bash "$root/tools/install-ubuntu-deps.sh" '../invalid' > "$work/invalid.output" 2>&1 || status=$?
[[ "$status" == 2 && ! -e "$work/invalid.log" ]]
printf 'case=invalid status=%s\n' "$status"
