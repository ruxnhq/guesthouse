//! Each formatter at the edges of its units, as the host it came from said it before it moved here.

use super::*;

const MIB: u64 = 1 << 20;
const GIB: u64 = 1 << 30;

#[test]
fn a_download_is_said_in_gib_called_gb_or_mib_called_mb_rounded_up() {
    for (bytes, said) in [
        (0, "0 MB"),
        (1, "1 MB"),
        (MIB - 1, "1 MB"),
        (MIB, "1 MB"),
        (MIB + 1, "2 MB"),
        (1023 * MIB, "1023 MB"),
        (GIB - 1, "1024 MB"),
        (GIB, "1.0 GB"),
        (GIB + 1, "1.0 GB"),
        (GIB + 51 * MIB, "1.0 GB"),
        (GIB + 52 * MIB, "1.1 GB"),
        (GIB + GIB / 4, "1.2 GB"),
        (GIB + GIB * 3 / 4, "1.8 GB"),
        (2 * GIB - 1, "2.0 GB"),
        (10 * GIB - MIB, "10.0 GB"),
        (100 * GIB, "100.0 GB"),
        (u64::MAX, "17179869184.0 GB"),
    ] {
        assert_eq!(size_in_gb_or_mb_up(bytes), said, "{bytes} bytes");
    }
}

#[test]
fn a_size_is_gib_called_gb_or_mib_called_mb_rounded_down() {
    for (bytes, said) in [
        (0, "0 MB"),
        (1, "0 MB"),
        (MIB - 1, "0 MB"),
        (MIB, "1 MB"),
        (MIB + 1, "1 MB"),
        (2 * MIB - 1, "1 MB"),
        (1023 * MIB, "1023 MB"),
        (GIB - 1, "1023 MB"),
        (GIB, "1.0 GB"),
        (GIB + 1, "1.0 GB"),
        (GIB + 51 * MIB, "1.0 GB"),
        (GIB + 52 * MIB, "1.1 GB"),
        (GIB + GIB / 4, "1.2 GB"),
        (GIB + GIB * 3 / 4, "1.8 GB"),
        (2 * GIB - 1, "2.0 GB"),
        (100 * GIB, "100.0 GB"),
        (u64::MAX, "17179869184.0 GB"),
    ] {
        assert_eq!(size_in_gb_or_mb_down(bytes), said, "{bytes} bytes");
    }
}

#[test]
fn a_host_drive_is_said_in_the_makers_decimal_units() {
    for (bytes, said) in [
        (0, "0 MB"),
        (499_999, "0 MB"),
        (500_000, "0 MB"),
        (999_999, "1 MB"),
        (1_000_000, "1 MB"),
        (1_500_000, "2 MB"),
        (2_500_000, "2 MB"),
        (999_499_999, "999 MB"),
        (999_500_000, "1000 MB"),
        (999_999_999, "1000 MB"),
        (1_000_000_000, "1 GB"),
        (1_500_000_000, "2 GB"),
        (2_500_000_000, "2 GB"),
        (999_499_999_999, "999 GB"),
        (999_999_999_999, "1000 GB"),
        (1_000_000_000_000, "1.0 TB"),
        (1_049_999_999_999, "1.0 TB"),
        (1_050_000_000_000, "1.1 TB"),
        (1_250_000_000_000, "1.2 TB"),
        (2_000_398_934_016, "2.0 TB"),
        (u64::MAX, "18446744.1 TB"),
    ] {
        assert_eq!(size_as_maker_says(bytes), said, "{bytes} bytes");
    }
}

#[test]
fn memory_is_whole_gb_or_mb() {
    for (mb, said) in [
        (0, "0 MB"),
        (1, "1 MB"),
        (1023, "1023 MB"),
        (1024, "1 GB"),
        (1025, "1025 MB"),
        (1536, "1536 MB"),
        (2048, "2 GB"),
        (16_383, "16383 MB"),
        (16_384, "16 GB"),
        (u64::MAX, "18446744073709551615 MB"),
    ] {
        assert_eq!(memory_as_given(mb), said, "{mb} MB");
    }
}

#[test]
fn card_memory_is_the_nearest_gb_or_mb() {
    for (mb, said) in [
        (0, "0 MB"),
        (1023, "1023 MB"),
        (1024, "1 GB"),
        (1535, "1 GB"),
        (1536, "2 GB"),
        (2047, "2 GB"),
        (8191, "8 GB"),
        (24_063, "23 GB"),
        (24_064, "24 GB"),
        (24_564, "24 GB"),
    ] {
        assert_eq!(card_memory_to_nearest_gb(mb), said, "{mb} MB");
    }
}

#[test]
fn spans_at_the_edges_of_each_unit() {
    for (seconds, said) in [
        (0, "0 seconds"),
        (1, "1 second"),
        (2, "2 seconds"),
        (59, "59 seconds"),
        (60, "1 minute"),
        (61, "1 minute"),
        (119, "1 minute"),
        (120, "2 minutes"),
        (3599, "59 minutes"),
        (3600, "1 hour"),
        (3659, "1 hour"),
        (3660, "1 hour 1 minute"),
        (7199, "1 hour 59 minutes"),
        (86_400, "24 hours"),
        (90_061, "25 hours 1 minute"),
        (u64::MAX, "5124095576030431 hours"),
    ] {
        assert_eq!(span(seconds), said, "{seconds} s");
    }
}

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
