#!/usr/bin/env bash
# Fall back once to official Ubuntu archives with the existing signing keys.
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
trap 'rm -- "$sources"' EXIT
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
install_from_current_source() {
  timeout --kill-after=10s 180s "${root[@]}" apt-get "${apt_options[@]}" update || return $?
  timeout --kill-after=10s 600s "${root[@]}" apt-get "${apt_options[@]}" install --yes "$@"
}
primary_url=https://mirrors.edge.kernel.org/ubuntu/
write_sources "$primary_url" "$primary_url"
printf 'Ubuntu dependency archive: %s, suite=%s, architecture=%s\n' \
  "$primary_url" "$VERSION_CODENAME" "$architecture"
if install_from_current_source "$@"; then
  :
else
  printf 'Primary Ubuntu acquisition failed; using official archives once\n' >&2
  write_sources "$archive_url" "$security_url"
  printf 'Ubuntu dependency archive: %s, security=%s, suite=%s, architecture=%s\n' \
    "$archive_url" "$security_url" "$VERSION_CODENAME" "$architecture"
  if ! install_from_current_source "$@"; then
    printf 'Both Ubuntu acquisition paths failed\n' >&2
    exit 100
  fi
fi
dpkg-query -W -f='${Package}\t${Version}\t${Status}\n' "$@"
