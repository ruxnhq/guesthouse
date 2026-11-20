# Boots a machine with the module on and looks at what it sets up. Starting a machine isn't tried, as
# that needs a GPU.
self: {
  name = "guesthouse";

  nodes.machine =
    { pkgs, ... }:
    {
      imports = [ self.nixosModules.default ];
      programs.guesthouse = {
        enable = true;
        users = [ "alice" ];
        bridges = [ "br0" ];
      };
      users.users.alice.isNormalUser = true;
      # bob is logged in at the console and listed nowhere, and carol isn't logged in.
      users.users.bob.isNormalUser = true;
      users.users.carol.isNormalUser = true;
      services.getty.autologinUser = "bob";
      boot.kernelModules = [ "udmabuf" ];
      environment.systemPackages = [ pkgs.acl ];
    };

  testScript =
    { nodes, ... }:
    ''
    import json

    machine.wait_for_unit("multi-user.target")

    machine.succeed("test -u /run/wrappers/bin/guesthouse-bridge-helper")
    assert machine.succeed("cat /etc/qemu/bridge.conf") == "allow br0\n"
    machine.succeed("id -nG alice | grep -qw kvm")

    # Whoever is logged in gets /dev/udmabuf and /dev/kvm is everyone's, both from systemd's own rules, which
    # is why nobody has to be listed in users.
    machine.wait_until_succeeds("loginctl list-sessions --no-legend | grep -qw bob")
    machine.wait_until_succeeds("getfacl -p /dev/udmabuf | grep -q '^user:bob:rw-'")
    machine.succeed("su bob -c 'test -r /dev/udmabuf -a -w /dev/udmabuf'")
    machine.fail("su carol -c 'test -w /dev/udmabuf'")
    machine.succeed("su carol -c 'test -r /dev/kvm -a -w /dev/kvm'")
    machine.succeed("test -f /run/current-system/sw/share/hwdata/pci.ids")
    machine.succeed("test -d /run/opengl-driver/lib")

    # Two descriptors, one with Microsoft's keys, whose files all exist.
    found = {}
    for name in machine.succeed("ls /etc/qemu/firmware").split():
        found[name] = json.loads(machine.succeed(f"cat /etc/qemu/firmware/{name}"))
    assert len(found) == 2, found
    keys = [d for d in found.values() if "enrolled-keys" in d["features"]]
    assert len(keys) == 1
    for d in found.values():
        assert d["mapping"]["device"] == "flash"
        assert {"secure-boot", "requires-smm"} <= set(d["features"])
        for part in ("executable", "nvram-template"):
            machine.succeed("test -s " + d["mapping"][part]["filename"])

    # The program folder is the one the release has, and its programs start.
    machine.succeed("test -x /run/current-system/sw/bin/guesthouse")
    machine.fail("test -e /run/current-system/sw/lib/guesthouse/install.sh")
    program = machine.succeed("dirname $(readlink -f $(command -v guesthouse))").strip().removesuffix("/bin") + "/lib/guesthouse"
    machine.succeed(f"GUESTHOUSE_PROGRAM={program} {program}/guesthouse-check")
    # What the app runs by name has to be on the wrapper's PATH, which is this one.
    tools = "7z qemu-img qemu-io isoinfo genisoimage swtpm ssh-keygen openssl gcc curl pkill timeout sha256sum mkfifo tar passt bwrap"
    path = "${nodes.machine.programs.guesthouse.package.toolPath}"
    machine.succeed(f"grep -qF {path} $(command -v guesthouse)")
    machine.succeed(f"for t in {tools}; do PATH={path} command -v $t || exit 1; done")
    machine.succeed(f"{program}/qemu/build/qemu-system-x86_64 --version")
    '';
}
