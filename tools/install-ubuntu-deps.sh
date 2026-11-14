#!/usr/bin/env bash
# Fall back once after a primary index error, with the existing signing keys.
# Keep the hosted runner's source files and third-party repositories unchanged.
set -euo pipefail
if (( $# == 0 )); then
  printf 'At least one Ubuntu package is required\n' >&2
  exit 2
fi
for package in "$@"; do
  [[ "$package" =~ ^[a-z0-9][a-z0-9+.-]*$ ]] || {
    printf 'Invalid Ubuntu package name: %s\n' "$package" >&2
    exit 2
  }
done
source /etc/os-release
[[ "$ID" == ubuntu ]] || { printf 'Ubuntu runner required\n' >&2; exit 2; }
case "$VERSION_CODENAME" in
  jammy|noble|resolute) ;;
  *) printf 'Unsupported Ubuntu suite: %s\n' "$VERSION_CODENAME" >&2; exit 2 ;;
esac
keyring=/usr/share/keyrings/ubuntu-archive-keyring.gpg
[[ -r "$keyring" ]] || { printf 'Ubuntu archive keyring is missing\n' >&2; exit 2; }
sources="$(mktemp)"
phase=preparation
phase_active=false
collection_unknown=false
cleanup_sources() {
  local status=$?
  trap - EXIT HUP INT TERM
  if [[ "$phase_active" == true || "$collection_unknown" == true ]]; then
    # A timeout/signal status cannot establish that APT's descendants drained.
    # The outer job owner must reconcile that flight; do not reuse or unlink
    # its source file, signal an old numeric PID, or start another APT command.
    printf 'source_cleanup=retained ownership=unknown phase=%s source_file=%q next_owner=outer-job\n' \
      "$phase" "$sources" >&2
  else
    rm -- "$sources" || { if (( status == 0 )); then status=1; fi; }
  fi
  exit "$status"
}
on_signal() {
  collection_unknown=true
  exit "$1"
}
trap cleanup_sources EXIT
trap 'on_signal 129' HUP
trap 'on_signal 130' INT
trap 'on_signal 143' TERM
architecture=$(dpkg --print-architecture)
case "$architecture" in
  amd64|i386)
    archive_url=https://archive.ubuntu.com/ubuntu/
    security_url=https://security.ubuntu.com/ubuntu/
    ;;
  *)
    archive_url=https://ports.ubuntu.com/ubuntu-ports/
    security_url=$archive_url
    ;;
esac
write_sources() {
  local archive=$1 security=$2 pocket uri
  : > "$sources"
  for pocket in '' -updates -security -backports; do
    uri=$archive
    if [[ "$pocket" == -security ]]; then uri=$security; fi
    printf 'deb [signed-by=%s] %s %s%s main restricted universe multiverse\n' \
      "$keyring" "$uri" "$VERSION_CODENAME" "$pocket" >> "$sources"
  done
  chmod 644 "$sources"
}
apt_options=(
  -o "Dir::Etc::sourcelist=$sources"
  -o Dir::Etc::sourceparts=-
  -o APT::Update::Error-Mode=any
  -o Acquire::Retries=0
  -o Acquire::http::Timeout=15
  -o Acquire::https::Timeout=15
)
root=()
if (( EUID != 0 )); then root=(sudo); fi
run_apt_phase() {
  phase=$1
  local limit=$2
  local ownership=command-returned
  shift 2
  phase_active=true
  apt_status=0
  timeout --kill-after=10s "$limit" "${root[@]}" apt-get "${apt_options[@]}" "$@" || apt_status=$?
  case "$apt_status" in
    0|100) phase_active=false ;;
    *) collection_unknown=true; ownership=unknown ;;
  esac
  # 0/100 describe the monitored command, not whole-job/descendant collection.
  # Only ordinary APT completion (0/100) is classified here. In particular,
  # 124/125/126/127/137 and signal statuses are not child-collection receipts.
  printf 'phase=%s command_status=%s ownership=%s\n' \
    "$phase" "$apt_status" "$ownership" >&2
  return "$apt_status"
}
primary_url=https://mirrors.edge.kernel.org/ubuntu/
write_sources "$primary_url" "$primary_url"
printf 'Ubuntu dependency archive: %s, suite=%s, architecture=%s\n' \
  "$primary_url" "$VERSION_CODENAME" "$architecture"
selected_source=primary
if run_apt_phase primary-update 180s update; then
  :
else
  status=$?
  if [[ "$collection_unknown" == true ]]; then exit "$status"; fi
  printf 'Primary Ubuntu update failed; using official archives once\n' >&2
  write_sources "$archive_url" "$security_url"
  selected_source=official
  printf 'Ubuntu dependency archive: %s, security=%s, suite=%s, architecture=%s\n' \
    "$archive_url" "$security_url" "$VERSION_CODENAME" "$architecture"
  if run_apt_phase official-update 180s update; then
    :
  else
    status=$?
    printf 'Official Ubuntu update failed\n' >&2
    exit "$status"
  fi
fi
if run_apt_phase "${selected_source}-install" 600s install --yes "$@"; then
  :
else
  status=$?
  printf 'Ubuntu installation failed; installation is not retried\n' >&2
  exit "$status"
fi
dpkg-query -W -f='${Package}\t${Version}\t${Status}\n' "$@"
