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
    };

  testScript =
    { nodes, ... }:
    ''
    import json

    machine.wait_for_unit("multi-user.target")

    machine.succeed("test -u /run/wrappers/bin/guesthouse-bridge-helper")
    assert machine.succeed("cat /etc/qemu/bridge.conf") == "allow br0\n"
    machine.succeed("id -nG alice | grep -qw kvm")
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
