What `remoteman-host ctl status` printed, one file each, read by tests/status_fixtures.rs.

from-source-*   written by hand from remoteman's control.rs, with made-up names and fingerprints.
captured-*      real output from a guest. Drop it in beside these as `captured-WHAT.json` (a
                status line) or `captured-WHAT.txt` (anything else the host said), with names and
                fingerprints replaced; the test reads every file here and needs no edit.
