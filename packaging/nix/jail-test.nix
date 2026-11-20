# Runs the jail by hand on NixOS, with the release's programs and what the app grants a GPU machine's
# QEMU: the namespace probe, then a small machine under Landlock alone and under bubblewrap as well.
# A machine with a GPU can't be made here, so this is the nearest to starting one. It needs nested KVM,
# so the flake keeps it out of its checks. Run it by hand with
#   nix build .#legacyPackages.x86_64-linux.jail-test
self:
let
  firmware = self.inputs.nixpkgs.legacyPackages.x86_64-linux.OVMFFull.fd;
in
{
  name = "guesthouse-jail";

  nodes.machine =
    { pkgs, ... }:
    {
      imports = [ self.nixosModules.default ];
      programs.guesthouse = {
        enable = true;
        users = [ "alice" ];
      };
      users.users.alice = {
        isNormalUser = true;
        linger = true;
      };
      environment.systemPackages = [ pkgs.bubblewrap ];
      virtualisation = {
        memorySize = 4096;
        cores = 4;
        qemu.options = [ "-cpu host" ];
      };
    };

  testScript =
    { nodes, ... }:
    ''
      machine.wait_for_unit("multi-user.target")
      machine.wait_for_unit("user@1000.service")
      machine.succeed("test -c /dev/kvm")

      package = "${nodes.machine.programs.guesthouse.package}"
      tools = "${nodes.machine.programs.guesthouse.package.toolPath}"
      script = "${./jail-check.sh}"

      out = machine.succeed(
          "runuser -u alice -- env XDG_RUNTIME_DIR=/run/user/1000 "
          f"PROGRAM={package}/lib/guesthouse PATH={tools}:/run/current-system/sw/bin "
          "CODE=${firmware}/FV/OVMF_CODE.fd "
          "VARS_TEMPLATE=${firmware}/FV/OVMF_VARS.ms.fd "
          f"sh {script}"
      )
      print(out)
      results = dict(line[7:].split(" ", 1) for line in out.splitlines() if line.startswith("result "))

      # The namespace probe works with the store bound, and the one before didn't.
      assert results["probe.new"].startswith("0 "), results["probe.new"]
      assert not results["probe.old"].startswith("0 "), results["probe.old"]

      # With what the app grants now, QEMU runs until the timeout ends it, jailed.
      for how in ("landlock", "namespaces"):
          assert results[f"{how}.exit"] == "124", results[f"{how}.log"]
          process = results[f"{how}.process"]
          assert "NoNewPrivs=1" in process and "Seccomp=2" in process, process
      assert "user_ns=different" in results["namespaces.process"]

      # With the old grants it can't start.
      for how in ("old.landlock", "old.namespaces"):
          assert results[f"{how}.process"] == "none", results[f"{how}.process"]
          assert results[f"{how}.exit"] != "124", results[f"{how}.log"]
    '';
}
