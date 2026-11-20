# Makes and starts a plain machine on NixOS, which runs unjailed. It needs nested KVM, which a CI runner may not have, so the
# flake keeps it out of its checks. Run it by hand with
#   nix build .#legacyPackages.x86_64-linux.machine-test
self: {
  name = "guesthouse-machine";

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
        # Keeps /run/user/1000, where the machine's sockets go, without a login.
        linger = true;
      };
      environment.systemPackages = [ pkgs.cdrkit ];
      virtualisation = {
        memorySize = 4096;
        cores = 4;
        diskSize = 4096;
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
      program = f"{package}/lib/guesthouse"
      tools = "${nodes.machine.programs.guesthouse.package.toolPath}"

      def alice(command):
          return machine.succeed(
              "runuser -u alice -- env XDG_RUNTIME_DIR=/run/user/1000 "
              f"PATH={tools}:/run/current-system/sw/bin "
              f"sh -c '{command}'"
          )

      # A disc with nothing on it makes a plain machine, which only has to start.
      alice("mkdir -p /tmp/disc && genisoimage -quiet -o /tmp/empty.iso /tmp/disc")
      alice(f"{program}/guesthouse-vm new plain --iso /tmp/empty.iso --disk 1 --ram 1 --cores 1")
      alice(f"{program}/guesthouse-vm start plain --no-wait")

      machine.sleep(15)
      status = alice(f"{program}/guesthouse-vm status plain")
      print(status)
      print(alice(f"{program}/guesthouse-vm log plain"))
      assert "running" in status.lower(), status

      # Plain machines aren't jailed. The firmware is the module's, and the machine's copy of its
      # settings is the user's to write.
      pid = machine.succeed("pgrep -u alice -x qemu-system-x86 | head -n 1").strip()
      command = machine.succeed(f"tr '\\0' ' ' < /proc/{pid}/cmdline")
      assert "-OVMF-" in command and "OVMF_CODE.fd" in command, command
      machine.succeed("runuser -u alice -- test -w /home/alice/.local/share/guesthouse/machines/plain/vars.fd")

      alice(f"{program}/guesthouse-vm off plain")
      machine.fail(f"test -d /proc/{pid}")
    '';
}
