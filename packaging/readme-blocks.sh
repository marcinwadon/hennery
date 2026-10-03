#!/bin/sh
# The README's shell examples (plan 4f). Every shell example in the README
# is a ```sh block, named on the line just before it:
#
#   <!-- check: up -->
#   ```sh
#   hennery up
#   ```
#
# and something runs each one: `crates/hennery/tests/readme.rs`, or a
# workflow (`ci.yml`, `nix.yml`) that runs this script's output.
#
# It fails closed: a fenced block is either a named ```sh block, or in a
# language that is not shell (the list in `BEGIN` below, in any case).
# Anything else is refused, so no shell example can hide from it.
#
# Usage: readme-blocks.sh <file> <name>    the block's lines, unindented
#        readme-blocks.sh <file> --list    every name, in order
#
# Exit status:
#   0  done;
#   1  the file breaks the rules, or it has no block of that name (the
#      message says which);
#   2  wrong arguments, or the file cannot be read;
#   3  awk itself failed.
#
# `README_BLOCKS_AWK` names the awk to run, for the tests; else `awk`.
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

status=0
LC_ALL=C "${README_BLOCKS_AWK:-awk}" -v want="$want" '
function fail(msg) { print "readme-blocks.sh: " FILENAME ":" NR ": " msg > "/dev/stderr"; failed = 1; exit 1 }
BEGIN {
    split("toml json jsonc yaml yml rust ts tsx js html css diff mermaid", list, " ")
    for (k in list) not_shell[list[k]] = 1
    tag = ""; inblock = 0; found = 0; failed = 0
}
{
    line = $0
    if (line ~ /\r/) fail("a carriage return: use LF line endings")
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
    if (line ~ /^[ \t>]*~~~/) fail("a ~~~ fence: use ``` for every block")
    if (line ~ /^[ \t]*>/ && line ~ /```/) fail("a fence in a block quote")
    if (line ~ /^[ \t]*```/ && line ~ /^ *\t/) fail("a fence indented with a tab")
    if (line ~ /^ *````/) fail("a fence of more than three backticks")
    if (line ~ /^ *```/) {
        match(line, /^ */)
        indent = RLENGTH
        info = substr(line, indent + 4)
        sub(/^[ \t]+/, "", info)
        sub(/[ \t].*$/, "", info)
        lower = tolower(info)
        if (lower in not_shell) {
            if (tag != "") fail("the tag " tag " is on a block in " info ", not a shell block")
            keep = 0
        } else if (info == "sh") {
            if (tag == "") fail("a shell block without a <!-- check: <name> --> line just before it")
            if (tag in seen) fail("the name " tag " is used twice")
            seen[tag] = 1
            names = names tag "\n"
            keep = (tag == want)
            if (keep) { found = 1; out = "" }
        } else {
            fail("a block in \"" info "\": a shell example is a named ```sh block; anything else names a language that is not shell")
        }
        inblock = 1
        openline = NR
        tag = ""
        next
    }
    if (tag != "") fail("the tag " tag " is not followed by a shell block")
    if (line ~ /<!-- *check/) {
        if (line !~ /^ *<!-- check: [a-z0-9][a-z0-9-]* -->[ \t]*$/) fail("a malformed tag: <!-- check: <name> -->, the name in a-z, 0-9 and -")
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
' "$file" || status=$?
case $status in
    0 | 1) exit "$status" ;;
    *)
        echo "readme-blocks.sh: awk failed (exit status $status)" >&2
        exit 3
        ;;
esac
