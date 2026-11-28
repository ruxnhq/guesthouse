//! The `ssh` line the Windows app shows for an Ubuntu guest, held as it is: the port and the user as
//! given, on this PC's loopback, with nothing quoted.

use guesthouse_control::ssh_line;

#[test]
fn the_ssh_line_is_the_port_and_the_user_as_given() {
    for (port, user, line) in [
        (22, "guesthouse", "ssh -p 22 guesthouse@127.0.0.1"),
        (2201, "desk", "ssh -p 2201 desk@127.0.0.1"),
        (0, "", "ssh -p 0 @127.0.0.1"),
        (65535, "a b", "ssh -p 65535 a b@127.0.0.1"),
    ] {
        assert_eq!(ssh_line(port, user), line);
    }
}
