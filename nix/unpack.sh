# shellcheck shell=bash
# unpack <package.tgz> <dest>: one npm package at its lockfile path, its
# first component (`package/`) stripped, under the host installer's rules
# (distribution spec §3.2; crates/hennery-host/src/runtime/extract.rs).
# The listing is judged before anything is written: only regular files
# and directories; no absolute name, no `.` or `..` component, no empty
# one; no name twice. A destination that exists is refused. After
# extraction, anything but files and directories fails as a backstop.
unpack() {
  local tgz=$1 dest=$2 listing names
  refuse() {
    echo "hennery: $tgz: $1" >&2
    return 1
  }
  if [ -e "$dest" ]; then
    refuse "$dest exists"
    return 1
  fi
  # Listed whole first, and judged from here-strings: a `grep -q` reading a
  # pipe can stop its writer early, and under `pipefail` the writer's
  # SIGPIPE would turn a refusal into a pass.
  listing=$(tar -tvzPf "$tgz") || { refuse "cannot be listed"; return 1; }
  names=$(tar -tzPf "$tgz") || { refuse "cannot be listed"; return 1; }
  if grep -qv '^[-d]' <<< "$listing"; then
    refuse "an entry is neither a file nor a directory (a link, or a special file)"
    return 1
  fi
  if grep -qE '^/|(^|/)\.{1,2}(/|$)|//' <<< "$names"; then
    refuse "a name is absolute, or has a '.', '..' or empty component"
    return 1
  fi
  if [ -n "$(sed 's|/$||' <<< "$names" | sort | uniq -d)" ]; then
    refuse "a name appears twice"
    return 1
  fi
  mkdir -p "$dest"
  tar -xzf "$tgz" -C "$dest" --strip-components=1 --no-same-owner --no-same-permissions
  if [ -n "$(find "$dest" ! -type f ! -type d)" ]; then
    refuse "extracted an entry that is neither a file nor a directory"
    return 1
  fi
}
