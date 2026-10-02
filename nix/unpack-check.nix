# `unpack.sh` refuses what the host's installer refuses (plan 7e-ii-a;
# crates/hennery-host/src/runtime/extract.rs): each bad tarball below fails
# with its own reason, and a good one lands without its first component.
{
  runCommand,
  python3,
}:
runCommand "hennery-unpack-refuses" { nativeBuildInputs = [ python3 ]; } ''
  source ${./unpack.sh}
  python3 - <<'EOF'
  import io, tarfile

  def make(name, entries):
      with tarfile.open(f"{name}.tgz", "w:gz") as tar:
          for path, kind, extra in entries:
              info = tarfile.TarInfo(path)
              info.type = kind
              data = b""
              if kind == tarfile.REGTYPE:
                  data = extra or b"x"
                  info.size = len(data)
              elif kind in (tarfile.SYMTYPE, tarfile.LNKTYPE):
                  info.linkname = extra
              tar.addfile(info, io.BytesIO(data) if kind == tarfile.REGTYPE else None)

  R, D = tarfile.REGTYPE, tarfile.DIRTYPE
  make("good", [("package/", D, None), ("package/package.json", R, b"{}"), ("package/lib/a.js", R, None)])
  make("symlink", [("package/a", R, None), ("package/l", tarfile.SYMTYPE, "/etc/passwd")])
  make("hardlink", [("package/a", R, None), ("package/h", tarfile.LNKTYPE, "package/a")])
  make("fifo", [("package/f", tarfile.FIFOTYPE, None)])
  make("twice", [("package/a", R, b"x"), ("package/a", R, b"y")])
  make("dotdot", [("package/../escape", R, None)])
  make("absolute", [("/package/escape", R, None)])
  make("dot", [("package/./a", R, None)])
  make("empty-component", [("package//a", R, None)])
  EOF
  refused() {
    if (unpack "$1.tgz" "$TMPDIR/out-$1") 2> "$1.err"; then
      echo "FAIL: $1 was unpacked" >&2
      exit 1
    fi
    grep -q "$2" "$1.err" || { echo "FAIL: $1: $(cat "$1.err")" >&2; exit 1; }
    echo "ok: $1 refused: $(cat "$1.err")"
  }
  refused symlink "neither a file nor a directory (a link, or a special file)"
  refused hardlink "neither a file nor a directory (a link, or a special file)"
  refused fifo "neither a file nor a directory (a link, or a special file)"
  refused twice "appears twice"
  refused dotdot "has a '.', '..'"
  refused absolute "is absolute"
  refused dot "has a '.', '..'"
  refused empty-component "empty component"

  unpack good.tgz "$TMPDIR/good/node_modules/p"
  [ "$(cat "$TMPDIR/good/node_modules/p/package.json")" = "{}" ]
  [ -f "$TMPDIR/good/node_modules/p/lib/a.js" ]
  [ ! -e "$TMPDIR/good/node_modules/p/package" ]
  echo "ok: good unpacked without its first component"
  refused_again() {
    if (unpack good.tgz "$TMPDIR/good/node_modules/p") 2> again.err; then
      echo "FAIL: an existing destination was unpacked into" >&2
      exit 1
    fi
    grep -q "exists" again.err
    echo "ok: an existing destination refused"
  }
  refused_again
  touch "$out"
''
