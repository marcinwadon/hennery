# An adapter package works (plan 7e-ii-a): every native program of its
# bundled CLI runs (`--version`), which on Linux proves `autoPatchelfHook`
# left each one working, and the adapter answers ACP `initialize` as the
# host starts it. Nothing here needs the network.
{
  lib,
  runCommand,
  coreutils,
  findutils,
  gnugrep,
  patchelf,
}:
adapter:
runCommand "${adapter.pname}-runs"
  {
    nativeBuildInputs = [
      coreutils
      findutils
      gnugrep
    ]
    ++ lib.optionals adapter.stdenv.hostPlatform.isElf [ patchelf ];
    # The macOS sandbox refuses the CLIs what they need at their first run
    # (nixpkgs' `claude-code` builds its check the same way). So on a Mac
    # with `sandbox = true` (not `relaxed`) this check cannot be built, and
    # elsewhere on macOS the CLIs run with the network.
    __noChroot = adapter.stdenv.hostPlatform.isDarwin;
  }
  ''
    export HOME="$TMPDIR/home"
    mkdir -p "$HOME"
    programs=0
    for dir in ${lib.concatMapStringsSep " " (path: "${adapter}/${path}") adapter.cliPaths}; do
      # The native programs, as the host's installer finds them: executable
      # files that are neither scripts nor libraries (`.so.<n>` included),
      # outside nested `node_modules`.
      while IFS= read -r program; do
        # Native code only: ELF, or Mach-O (thin or universal).
        case "$(head -c 4 "$program" | od -An -tx1 | tr -d ' \n')" in
          7f454c46 | cffaedfe | cafebabe) ;;
          *) continue ;;
        esac
        echo "running $program --version"
        status=0
        said=$(timeout 120 "$program" --version < /dev/null 2>&1) || status=$?
        echo "$said"
        # Each CLI must name itself: a bun-compiled one that lost its
        # program (stripped) answers with bun's version instead.
        case "$(basename "$program")" in
          claude) name="(Claude Code)" ;;
          codex) name="codex-cli" ;;
          rg) name="ripgrep" ;;
          *) name="" ;;
        esac
        if [ "$status" -eq 0 ] && [ -n "$name" ] && ! grep -qF "$name" <<< "$said"; then
          echo "$program --version does not say $name" >&2
          exit 1
        fi
        # The CLIs themselves must answer; a helper that takes no
        # `--version` must still have started (not 126 or 127, the loader's
        # failures, nor a signal or the timeout's 124). A helper whose
        # loader this system does not have (Codex's voice host, left
        # unpatched: decision 4) is named and passed over.
        case "$(basename "$program"):$status" in
          claude:0 | codex:0 | rg:0) ;;
          claude:* | codex:* | rg:*) echo "$program failed: $status" >&2; exit 1 ;;
          *:127)
            interpreter=$(patchelf --print-interpreter "$program" 2> /dev/null || true)
            if [ -n "$interpreter" ] && [ ! -e "$interpreter" ]; then
              echo "passed over: $program needs the loader $interpreter, which this system does not have"
              continue
            fi
            echo "$program did not run: $status (loader: ''${interpreter:-none})" >&2
            exit 1
            ;;
          *:124 | *:12[6-9] | *:1[3-9]? | *:2??) echo "$program did not run: $status" >&2; exit 1 ;;
        esac
        programs=$((programs + 1))
      done < <(find "$dir" -name node_modules -prune -o -type f -perm -u+x \
        ! -name '*.node' ! -name '*.so' ! -name '*.so.*' ! -name '*.dylib' -print | sort)
    done
    if [ "$programs" -eq 0 ]; then
      echo "no native program found in ${adapter.pname}'s CLI" >&2
      exit 1
    fi
    # `initialize` on standard input, a pipe held open until the answer is
    # read; closing it then ends the adapter, as it ends at its host's end.
    request='{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":1,"clientCapabilities":{}}}'
    mkfifo "$TMPDIR/in"
    timeout 120 ${lib.getExe adapter} < "$TMPDIR/in" > "$TMPDIR/answer" &
    adapter=$!
    exec 3> "$TMPDIR/in"
    echo "$request" >&3
    for _ in $(seq 1 120); do
      if grep -q '"result"' "$TMPDIR/answer"; then
        break
      fi
      sleep 1
    done
    exec 3>&-
    wait "$adapter" || true
    head -c 2000 "$TMPDIR/answer"
    echo
    if ! grep '"result"' "$TMPDIR/answer" | grep -q '"id":0'; then
      echo "${adapter.pname} did not answer initialize" >&2
      exit 1
    fi
    touch "$out"
  ''
