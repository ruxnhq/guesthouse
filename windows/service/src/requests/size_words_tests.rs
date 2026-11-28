//! How the service says a size in a refusal, held as it is at the edges of each unit: GiB called GB
//! with one decimal from 1 GiB, and whole MiB called MB, rounded down, below it.

use super::size_words;

const MIB: u64 = 1 << 20;
const GIB: u64 = 1 << 30;

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
        assert_eq!(size_words(bytes), said, "{bytes} bytes");
    }
}
