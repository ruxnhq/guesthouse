// SPDX-License-Identifier: GPL-2.0-or-later
//! Ubuntu for ARM: the official installer and its published checksums, and the answers its
//! installer reads (cloud-init's autoinstall), in the shape the other hosts use
//! (`windows/host/guest/linux/user-data` of the public repo).
//!
//! The installer is Ubuntu's own: the live server image for arm64 from `cdimage.ubuntu.com`, with
//! the `SHA256SUMS` beside it. The firmware starts its kernel from the image with the kernel line
//! the installer needs to run on its own (`INSTALLER_COMMAND_LINE`), as the public repo's Windows
//! host does with a boot menu of its own.
//!
//! The graphics driver comes as a package (`driver::LINUX`) on a small disk labelled `RUXN`, as
//! the Windows driver does. The answers put a service into the installed system that installs it
//! from that disk when it is there, once for each version (`DRIVER_SERVICE`, `DRIVER_RUNNER`).
//! When the package cannot be fetched, the driver step says why (`driver_download_failed`), fetches
//! again on Try Again, and offers to open Ubuntu without it.

/// Ubuntu's release folder for arm64 and its checksum list.
pub const RELEASE_URL: &str = "https://cdimage.ubuntu.com/ubuntu/releases/26.04/release/";
pub const SUMS_FILE: &str = "SHA256SUMS";
/// The server image's name begins and ends so; the `+largemem` kernel's image shares the ending
/// and is left out.
const PREFIX: &str = "ubuntu-26.04";
const SUFFIX: &str = "-live-server-arm64.iso";

/// What the guest says on the serial port, as the other hosts' autoinstall does.
pub const FINISHED: &str = "guesthouse-install-finished";
pub const FAILED: &str = "guesthouse-install-failed";

/// The installer's kernel and initrd, by their paths in Ubuntu's image.
pub const INSTALLER_KERNEL: &str = "casper/vmlinuz";
pub const INSTALLER_INITRD: &str = "casper/initrd";

/// The installer's kernel line. `autoinstall`, because the installer asks before it writes the
/// disk when it found its answers only on a seed (subiquity's install controller, read
/// 2026-11-27); `ds=nocloud` for the seed disc; its console on the screen (the image runs a copy
/// of the installer on the serial port as well, which prints each late command into
/// `serial.log`, so no late command holds the password). What follows `---` the installed system
/// keeps: its console on the serial port as well, so its starts can be followed in `serial.log`.
pub const INSTALLER_COMMAND_LINE: &str =
    "autoinstall ds=nocloud console=tty0 --- console=ttyAMA0,115200 console=tty0";

/// The answer disc's file with the account's name and password, as `chpasswd` reads them. The
/// late commands copy it into the installed system for `chpasswd` and remove it, so the password
/// is neither in `user-data` (which the installer keeps in the installed system's logs) nor in a
/// command the installer prints, as the public repo's Linux host keeps it out.
pub const ACCOUNT_FILE: &str = "guesthouse-account";

/// The account file's text.
pub fn account_file(account: &str, password: &str) -> String {
    format!("{account}:{password}\n")
}

/// What the guest says about the driver and its first start: the same words as Windows'.
pub use super::windows::marker;

/// The answer disc's two files that the installed system gets, by their names on the disc.
pub const DRIVER_SERVICE_FILE: &str = "guesthouse-driver.service";
pub const DRIVER_RUNNER_FILE: &str = "guesthouse-driver.sh";

/// The service that installs the driver from its disk when the system starts.
pub const DRIVER_SERVICE: &str = "\
[Unit]
Description=Guesthouse graphics driver, from the disk labelled RUXN
Wants=network-online.target
After=network-online.target

[Service]
Type=oneshot
ExecStart=/bin/sh /usr/local/sbin/guesthouse-driver
TimeoutStartSec=30min

[Install]
WantedBy=multi-user.target
";

/// What the service runs: the package copied off its disk, its `install.sh` run as root (which
/// says on the serial port how it went), then a restart into the desktop, or a power-off when it
/// failed, so that Try Again is the next start with the disk. A version already installed is
/// not installed again. Without the disk it does nothing.
pub const DRIVER_RUNNER: &str = r#"#!/bin/sh
# Guesthouse: installs the graphics driver from the disk labelled RUXN, once for each version.
disk=/dev/disk/by-label/RUXN
mnt=/run/guesthouse-driver
state=/var/lib/guesthouse
manifest=guesthouse-linux-driver-latest.json
say() { echo "guesthouse: $*" > /dev/ttyAMA0; }
# The disk may still be on its way while the system starts.
tries=0
while [ ! -e "$disk" ]; do
    tries=$((tries + 1))
    [ "$tries" -gt 20 ] && exit 0
    sleep 1
done
mkdir -p "$mnt" "$state"
if ! mount -o ro "$disk" "$mnt"; then
    say "driver failed mount"
    systemctl --no-block poweroff
    exit 1
fi
version=$(sed -n 's/.*"version" *: *"\([^"]*\)".*/\1/p' "$mnt/$manifest")
if [ -n "$version" ] && [ "$(cat "$state/driver-version" 2>/dev/null)" = "$version" ]; then
    umount "$mnt"
    exit 0
fi
work="$state/driver/${version:-unknown}"
rm -rf "$work"
mkdir -p "$work"
cp -R "$mnt/." "$work/"
umount "$mnt"
(cd "$work" && sh ./install.sh)
code=$?
if [ "$code" -eq 0 ]; then
    echo "$version" > "$state/driver-version"
    systemctl --no-block reboot
else
    # Only read when install.sh said nothing itself: the first line said counts.
    say "driver failed exit-$code"
    systemctl --no-block poweroff
fi
"#;

/// Said at the driver step when there is no package for Ubuntu: none published, none named.
/// What the driver step says when the package could not be fetched: the reason, so a person can
/// tell a connection that dropped from a site that is down.
pub fn driver_download_failed(reason: &str) -> String {
    format!(
        "The Guesthouse graphics driver for Ubuntu could not be downloaded: {reason} Try again, or open Ubuntu without it for now."
    )
}

/// The image to fetch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub name: String,
    pub sha256: String,
    pub url: String,
}

/// The server image for arm64 named in Ubuntu's checksum list, with the checksum it publishes: the
/// newest point release in the list. The folder lists a release and its point releases together
/// (`ubuntu-26.04-...` and `ubuntu-26.04.1-...`, and a `.2` when it comes), so the highest point
/// number wins, and the `+largemem` image and every other architecture are left out. A line is
/// `SHA256 *NAME` (binary) or `SHA256  NAME`.
pub fn pick_image(sums: &str) -> Result<Image, String> {
    let mut best: Option<(u32, Image)> = None;
    for line in sums.lines() {
        let Some((sum, name)) = line.split_once(' ') else {
            continue;
        };
        let name = name.trim_start_matches([' ', '*']).trim();
        let Some(point) = point_release(name) else {
            continue;
        };
        if sum.len() != 64 || !sum.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(format!("Ubuntu's checksum for {name} is not a SHA-256."));
        }
        if best.as_ref().is_none_or(|(known, _)| point > *known) {
            best = Some((
                point,
                Image {
                    name: name.to_string(),
                    sha256: sum.to_ascii_lowercase(),
                    url: format!("{RELEASE_URL}{name}"),
                },
            ));
        }
    }
    best.map(|(_, image)| image)
        .ok_or_else(|| "Ubuntu's list of checksums has no arm64 server image in it.".into())
}

/// The point release of a plain arm64 server image's name: 0 for `ubuntu-26.04-live-server-arm64.iso`,
/// 1 for `ubuntu-26.04.1-...`. Nothing for any other name (another image, another release).
fn point_release(name: &str) -> Option<u32> {
    let middle = name.strip_prefix(PREFIX)?.strip_suffix(SUFFIX)?;
    if middle.is_empty() {
        return Some(0);
    }
    let digits = middle.strip_prefix('.')?;
    (!digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())).then(|| digits.parse().ok()).flatten()
}

fn yaml(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// The Mac's time zone as Ubuntu names it (`Europe/Berlin`): where `/etc/localtime` points,
/// after `zoneinfo/`. `Etc/UTC` when it points nowhere a zone's name can be read from.
pub fn mac_time_zone() -> String {
    time_zone_of(std::fs::read_link("/etc/localtime").ok().as_deref())
}

/// The zone a link to the zone database names, or `Etc/UTC`.
fn time_zone_of(link: Option<&std::path::Path>) -> String {
    let text = link.and_then(|link| link.to_str()).unwrap_or_default();
    let name = text.rsplit_once("zoneinfo/").map_or("", |(_, name)| name);
    let plain = name.split('/').all(|part| !part.is_empty() && !part.starts_with('.'))
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b"/_+-".contains(&b));
    if plain { name } else { "Etc/UTC" }.to_string()
}

/// The desktop an Ubuntu machine starts to: weston (the driver package's own), Ubuntu's GNOME, or
/// KDE Plasma. The driver package's `install.sh` reads the choice from `/etc/guesthouse/desktop`,
/// which the answers write; weston needs no file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Desktop {
    #[default]
    Weston,
    Gnome,
    Plasma,
}

impl Desktop {
    /// The word the app sends and the file holds.
    pub fn parse(word: &str) -> Option<Desktop> {
        match word {
            "weston" => Some(Desktop::Weston),
            "gnome" => Some(Desktop::Gnome),
            "plasma" => Some(Desktop::Plasma),
            _ => None,
        }
    }

    pub fn word(self) -> &'static str {
        match self {
            Desktop::Weston => "weston",
            Desktop::Gnome => "gnome",
            Desktop::Plasma => "plasma",
        }
    }
}

/// The autoinstall answers: no questions, the largest disk, an account named after the Mac's user
/// whose password is set at the end from the answer disc's account file (`ACCOUNT_FILE`), so the
/// first console sign-in uses it, the clock in `time_zone` (the Mac's, `mac_time_zone`), the
/// driver's service from the answer disc, the desktop the driver package is to install, the
/// keyboard's layout (the Mac's, `keyboard_layout::mac_layout`, else `us`) for the console and
/// the sign-in screen, and what is said on the serial port when it ends.
pub fn user_data(
    account: &str,
    hostname: &str,
    time_zone: &str,
    desktop: Desktop,
    keyboard: Option<crate::keyboard_layout::Xkb>,
) -> String {
    let keyboard = keyboard.map_or_else(
        || "    layout: us\n".to_string(),
        |xkb| match xkb.variant {
            "" => format!("    layout: {}\n", xkb.layout),
            variant => format!("    layout: {}\n    variant: {variant}\n", xkb.layout),
        },
    );
    let desktop_lines = match desktop {
        Desktop::Weston => String::new(),
        chosen => format!(
            "    - mkdir -p /target/etc/guesthouse\n    - echo {} > /target/etc/guesthouse/desktop\n",
            chosen.word()
        ),
    };
    format!(
        "#cloud-config
autoinstall:
  version: 1
  locale: C.UTF-8
  keyboard:
{keyboard}  storage:
    layout:
      name: direct
      match:
        size: largest
  identity:
    hostname: {host}
    username: {user}
    # The installer needs a hash here; the real password is set in the late commands, where it
    # needs no hash.
    password: '$6$guesthouse$invalid'
  ssh:
    install-server: true
    allow-pw: false
  updates: security
  refresh-installer:
    update: false
  drivers:
    install: false
  user-data:
    timezone: {zone}
  shutdown: poweroff
  error-commands:
    - tail -n 100 /var/log/installer/subiquity-server-debug.log > /dev/ttyAMA0 || true
    - echo {failed} > /dev/ttyAMA0
  late-commands:
    - mkdir -p /run/guesthouse-answers
    - mount -o ro LABEL=CIDATA /run/guesthouse-answers
    - install -m 0600 /run/guesthouse-answers/{account_file} /target/root/{account_file}
    - 'curtin in-target --target=/target -- sh -c \"chpasswd < /root/{account_file} && rm -f /root/{account_file}\"'
    - install -D -m 0755 /run/guesthouse-answers/{runner} /target/usr/local/sbin/guesthouse-driver
    - install -D -m 0644 /run/guesthouse-answers/{service} /target/etc/systemd/system/{service}
    - curtin in-target --target=/target -- systemctl enable {service}
    - umount /run/guesthouse-answers
{desktop_lines}    - echo {finished} > /dev/ttyAMA0
",
        host = hostname,
        user = account,
        zone = yaml(time_zone),
        account_file = ACCOUNT_FILE,
        failed = FAILED,
        finished = FINISHED,
        runner = DRIVER_RUNNER_FILE,
        service = DRIVER_SERVICE_FILE,
    )
}

/// The seed disc's second file: it names the instance.
pub fn meta_data(machine: &str) -> String {
    format!("instance-id: guesthouse-{}\nlocal-hostname: {}\n", machine, yaml(machine).trim_matches('\''))
}

#[cfg(test)]
mod tests;
