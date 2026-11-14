#!/usr/bin/env bash
# Exercise the real acquisition helper with fake apt, never the network/root.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
work=$(mktemp -d)
trap 'rm -rf -- "$work"' EXIT
mkdir "$work/bin"
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
  primary-update:primary:update|all-update:*:update|fallback-install:primary:update|ports:primary:update) exit 100 ;;
  primary-install:primary:install|fallback-install:official:install) exit 100 ;;
esac
SH
chmod +x "$work/bin/"*
packages=(libxkbcommon-dev libwayland-dev libx11-dev libxcursor-dev libxi-dev libxrandr-dev)
run_case() {
  local name=$1 wanted=$2 expected=$3 architecture=${4:-amd64} status=0
  local log="$work/$name.log" source_log="$work/$name.sources"
  : > "$log"
  : > "$source_log"
  PATH="$work/bin:$PATH" FIXTURE_MODE="$name" FIXTURE_ARCH="$architecture" \
    FIXTURE_LOG="$log" FIXTURE_SOURCES="$source_log" \
    bash "$root/tools/install-ubuntu-deps.sh" "${packages[@]}" > "$work/$name.output" 2>&1 || status=$?
  if [[ "$status" != "$wanted" || "$(cat "$log")" != "$expected" ]]; then
    cat "$work/$name.output" >&2
    printf 'case=%s wanted=%s actual=%s\n' "$name" "$wanted" "$status" >&2
    cat "$log" >&2
    return 1
  fi
  while IFS= read -r path; do [[ ! -e "$path" ]]; done < "$source_log"
  printf 'case=%s status=%s\n' "$name" "$status"
}
run_case normal 0 $'primary update\nprimary install\nquery'
run_case primary-update 0 $'primary update\nofficial update\nofficial install\nquery'
run_case primary-install 0 $'primary update\nprimary install\nofficial update\nofficial install\nquery'
run_case all-update 100 $'primary update\nofficial update'
run_case fallback-install 100 $'primary update\nofficial update\nofficial install'
run_case ports 0 $'primary update\nofficial update\nofficial install\nquery' arm64
status=0
PATH="$work/bin:$PATH" FIXTURE_MODE=invalid FIXTURE_ARCH=amd64 \
  FIXTURE_LOG="$work/invalid.log" FIXTURE_SOURCES="$work/invalid.sources" \
  bash "$root/tools/install-ubuntu-deps.sh" '../invalid' > "$work/invalid.output" 2>&1 || status=$?
[[ "$status" == 2 && ! -e "$work/invalid.log" ]]
printf 'case=invalid status=%s\n' "$status"
