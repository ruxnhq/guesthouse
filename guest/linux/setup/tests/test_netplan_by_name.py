#!/usr/bin/env python3
"""netplan-by-name.sh: the installer's netplan, pinned to QEMU's default address, found by name instead."""
import os
import subprocess
import tempfile
import unittest

GUEST = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "guest")
INSTALLER = """# This is the network config written by 'subiquity'
network:
  ethernets:
    enp0s2:
      dhcp4: true
      dhcp6: true
      match:
        macaddress: '52:54:00:12:34:56'
      set-name: enp0s2
  version: 2
"""


class NetplanByName(unittest.TestCase):
    def setUp(self):
        folder = tempfile.TemporaryDirectory()
        self.addCleanup(folder.cleanup)
        self.root = folder.name
        os.makedirs(os.path.join(self.root, "etc/netplan"))
        self.file = os.path.join(self.root, "etc/netplan/00-installer-config.yaml")

    def put(self, text):
        with open(self.file, "w") as f:
            f.write(text)

    def run_script(self):
        result = subprocess.run(["bash", os.path.join(GUEST, "netplan-by-name.sh")],
                                env=dict(os.environ, NVFWD_ROOT=self.root), capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        with open(self.file) as f:
            return f.read()

    def test_the_installers_file_is_found_by_name_and_keeps_dhcp(self):
        self.put(INSTALLER)
        text = self.run_script()
        self.assertNotIn("macaddress", text)
        self.assertIn('name: "en*"', text)
        self.assertIn("dhcp4: true", text)
        self.assertIn("dhcp6: true", text)
        self.assertEqual(self.run_script(), text, "nothing more to do the second time")

    def test_a_file_someone_changed_stays(self):
        for text in (INSTALLER + "# mine\n", INSTALLER.replace("dhcp6: true", "dhcp6: false"),
                     INSTALLER.replace("52:54:00:12:34:56", "52:54:00:aa:bb:cc")):
            self.put(text)
            self.assertEqual(self.run_script(), text)

    def test_no_file_is_no_problem(self):
        os.makedirs(os.path.join(self.root, "etc/netplan"), exist_ok=True)
        result = subprocess.run(["bash", os.path.join(GUEST, "netplan-by-name.sh")],
                                env=dict(os.environ, NVFWD_ROOT=self.root), capture_output=True, text=True)
        self.assertEqual(result.returncode, 0)


if __name__ == "__main__":
    unittest.main()
