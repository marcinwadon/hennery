# The NixOS module in a virtual machine (plan 7e-ii-a): a collector and a
# host on one machine, the host given the Nix Codex adapter (the free one:
# the Claude adapter is unfree, and no check may need its licence).
#
# - The collector starts as its own system user and answers `/healthz`.
# - The host is skipped while it holds no pairing, not failed.
# - Paired with `host join --no-runtime`, it starts, connects, and logs to
#   its `LogsDirectory=`.
# - `hennery doctor`, run as the host's user, reads the system unit's
#   `--agent` and judges no managed set: check 1 says so, check 2 does not
#   fail for want of nix-ld, and check 3 starts the Nix adapter.
# - The collector's unit keeps its hardening, and logs to its own directory.
{ pkgs, nixosModule }:
pkgs.testers.runNixOSTest {
  name = "hennery-nixos";
  nodes.machine =
    { pkgs, ... }:
    {
      imports = [ nixosModule ];
      virtualisation.memorySize = 2048;
      users.users.alice = {
        isNormalUser = true;
        uid = 1000;
      };
      services.hennery = {
        collector.enable = true;
        host = {
          enable = true;
          user = "alice";
          adapters.agents = [ "codex" ];
        };
      };
      environment.systemPackages = [ pkgs.curl ];
    };
  testScript = ''
    import re

    machine.wait_for_unit("hennery-collector.service")
    machine.wait_for_open_port(7117)
    machine.succeed("curl -sf http://127.0.0.1:7117/healthz")
    machine.succeed("[ \"$(stat -c '%U %a' /var/lib/hennery)\" = 'hennery 700' ]")

    with subtest("the collector runs as its own user, hardened"):
        machine.succeed("[ \"$(systemctl show -P User hennery-collector.service)\" = hennery ]")
        machine.succeed("[ \"$(systemctl show -P NoNewPrivileges hennery-collector.service)\" = yes ]")
        machine.succeed("[ \"$(systemctl show -P ProtectSystem hennery-collector.service)\" = strict ]")
        machine.succeed("[ -z \"$(systemctl show -P CapabilityBoundingSet hennery-collector.service)\" ]")
        machine.succeed("[ \"$(systemctl show -P MemoryDenyWriteExecute hennery-collector.service)\" = yes ]")
        machine.succeed("[ \"$(stat -c '%U %a' /var/log/hennery-collector)\" = 'hennery 700' ]")
        machine.wait_until_succeeds("[ -s /var/log/hennery-collector/hennery-collector.log ]", timeout=30)

    with subtest("an unpaired host is skipped, not failed"):
        machine.succeed("[ \"$(stat -c '%U %a' /var/lib/hennery-host)\" = 'alice 700' ]")
        machine.succeed("systemctl start hennery-host.service")
        machine.succeed("[ \"$(systemctl show -P ConditionResult hennery-host.service)\" = no ]")
        machine.fail("systemctl is-failed hennery-host.service")

    with subtest("the unit gives the Nix adapter and keeps 7c's policy"):
        exec_start = machine.succeed("systemctl show -P ExecStart hennery-host.service")
        assert "--agent codex=/nix/store/" in exec_start, exec_start
        assert "hennery-codex-acp" in exec_start, exec_start
        machine.succeed("[ \"$(systemctl show -P RestartPreventExitStatus hennery-host.service)\" = 78 ]")
        machine.succeed("[ \"$(systemctl show -P KillMode hennery-host.service)\" = mixed ]")
        machine.succeed("[ \"$(systemctl show -P User hennery-host.service)\" = alice ]")

    def dump():
        # What a failure needs to be read from CI's log alone.
        print(machine.execute(
            "systemctl status hennery-host.service hennery-collector.service --no-pager -l; "
            "journalctl -u hennery-host -u hennery-collector --no-pager | tail -n 80; "
            "tail -n 40 /var/log/hennery-host/hennery-host.log; ls -la /var/lib/hennery-host"
        )[1])

    with subtest("paired, the host runs and connects"):
        try:
            # `admin pairing-code` asks on a terminal: `script` gives it one,
            # and the answer is typed ahead.
            said = machine.succeed(
                "{ echo yes; sleep 5; } | script -qec"
                " 'runuser -u hennery -- hennery admin --data-dir /var/lib/hennery pairing-code' /dev/null",
                timeout=60,
            )
            code = re.search(r"\b[A-Za-z0-9]{4}-[A-Za-z0-9]{4}\b", said).group(0)
            machine.succeed(
                f"runuser -u alice -- hennery host join http://127.0.0.1:7117 {code} --no-runtime"
                " --data-dir /var/lib/hennery-host < /dev/null",
                timeout=60,
            )
            machine.succeed("systemctl start hennery-host.service", timeout=30)
            machine.wait_for_unit("hennery-host.service", timeout=60)
            machine.wait_until_succeeds(
                "grep -q 'connected to collector' /var/log/hennery-host/hennery-host.log", timeout=60
            )
            hosts = machine.succeed("runuser -u hennery -- hennery admin --data-dir /var/lib/hennery hosts", timeout=30)
            assert "paired" in hosts, hosts
        except Exception:
            dump()
            raise

    with subtest("doctor sees the system unit's --agent"):
        report = machine.succeed(
            "runuser -u alice -- hennery doctor --data-dir /var/lib/hennery-host < /dev/null || true", timeout=180
        )
        print(report)
        lines = {
            int(m.group(2)): m.group(0)
            for m in re.finditer(r"^(ok|warn|fail)\s+(\d+) .*$", report, re.M)
        }
        assert (
            "the system unit /etc/systemd/system/hennery-host.service gives its agents with --agent" in lines[1]
        ), report
        assert not lines[2].startswith("fail"), report
        assert "codex answers" in lines[3] and lines[3].startswith("ok"), report
  '';
}
