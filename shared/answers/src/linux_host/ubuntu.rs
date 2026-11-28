//! An Ubuntu guest's answers on the Linux host, on its seed disc: `user-data` with the account, the
//! password's hash, root's SSH key and the kernel on the ISO, and `meta-data` as it is. Nothing is
//! escaped: the host's name rules keep the account to what YAML takes in quotes, and the rest the
//! host makes itself.

/// The template, as `guest/linux/setup/seed/user-data.template` holds it. The host reads its own copy from
/// the program folder when it installs.
pub const TEMPLATE: &str = include_str!("../../../../guest/linux/setup/seed/user-data.template");
/// `meta-data`, as `guest/linux/setup/seed/meta-data` holds it, which goes on the disc as it is.
pub const META_DATA: &str = include_str!("../../../../guest/linux/setup/seed/meta-data");

/// The answers, with this machine's account and secrets and the kernel on its ISO.
pub fn fill_template(template: &str, user: &str, hash: &str, key: &str, kernel_package: &str) -> String {
    let filled = template.replace("@USER@", user).replace("@PASSWORD_HASH@", hash).replace("@SSH_KEY@", key);
    filled.replace("@KERNEL_PACKAGE@", kernel_package)
}
