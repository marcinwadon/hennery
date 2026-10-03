#!/bin/sh
# The README's shell examples, by name (plan 4f). Every shell example in
# the README is a fenced block tagged on the line just before it:
#
#   <!-- check: up -->
#   ```sh
#   hennery up
#   ```
#
# and something runs each one: `crates/hennery/tests/readme.rs`, or a
# workflow (`ci.yml`, `nix.yml`) that runs this script's output.
#
# Usage: readme-blocks.sh <file> <name>    the block's lines, unindented
#        readme-blocks.sh <file> --list    every name, in order
#
# Exit status:
#   0  done;
#   1  the file breaks the rules (a shell block without a tag, a tag not
#      followed by a shell block, a name used twice, a block never closed,
#      a `~~~` fence), or it has no block of that name;
#   2  wrong arguments, or the file cannot be read.
#
# A shell block is one whose info string is empty, sh, bash, shell,
# console or zsh. Blocks in other languages need no tag.
set -eu

if [ $# -ne 2 ]; then
    echo "usage: readme-blocks.sh <file> <name> | readme-blocks.sh <file> --list" >&2
    exit 2
fi
file=$1
want=$2
if [ ! -f "$file" ] || [ ! -r "$file" ]; then
    echo "readme-blocks.sh: cannot read $file" >&2
    exit 2
fi

LC_ALL=C awk -v want="$want" '
function fail(msg) { print "readme-blocks.sh: " FILENAME ":" NR ": " msg > "/dev/stderr"; failed = 1; exit 1 }
function is_shell(info) { return info == "" || info == "sh" || info == "bash" || info == "shell" || info == "console" || info == "zsh" }
BEGIN { tag = ""; inblock = 0; found = 0; failed = 0 }
{
    line = $0
    if (inblock) {
        if (line ~ /^ *```[ \t]*$/) { inblock = 0; next }
        if (keep) {
            body = line
            n = indent
            while (n > 0 && substr(body, 1, 1) == " ") { body = substr(body, 2); n-- }
            out = out body "\n"
        }
        next
    }
    if (line ~ /^ *~~~/) fail("a ~~~ fence: use ``` for every block")
    if (line ~ /^ *```/) {
        match(line, /^ */)
        indent = RLENGTH
        info = substr(line, indent + 4)
        sub(/^[ \t]+/, "", info)
        sub(/[ \t].*$/, "", info)
        if (is_shell(info)) {
            if (tag == "") fail("a shell block without a <!-- check: <name> --> line just before it")
            if (tag in seen) fail("the name " tag " is used twice")
            seen[tag] = 1
            names = names tag "\n"
            keep = (tag == want)
            if (keep) { found = 1; out = "" }
        } else if (tag != "") {
            fail("the tag " tag " is on a block in " info ", not a shell block")
        } else {
            keep = 0
        }
        inblock = 1
        openline = NR
        tag = ""
        next
    }
    if (tag != "") fail("the tag " tag " is not followed by a shell block")
    if (line ~ /^ *<!-- check: [a-z0-9-]+ -->[ \t]*$/) {
        tag = line
        sub(/^ *<!-- check: /, "", tag)
        sub(/ -->.*$/, "", tag)
    }
}
END {
    if (failed) exit 1
    if (inblock) { print "readme-blocks.sh: " FILENAME ":" openline ": a block never closed" > "/dev/stderr"; exit 1 }
    if (tag != "") { print "readme-blocks.sh: " FILENAME ": the tag " tag " is not followed by a shell block" > "/dev/stderr"; exit 1 }
    if (want == "--list") { printf "%s", names; exit 0 }
    if (!found) { print "readme-blocks.sh: " FILENAME ": no block named " want > "/dev/stderr"; exit 1 }
    printf "%s", out
}
' "$file"
