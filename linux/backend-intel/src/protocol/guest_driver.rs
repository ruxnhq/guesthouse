//! Where the guest finds Intel's user-mode drivers, and the registry values their loaders ask the
//! adapter for.
//!
//! The paravirtual adapter's kernel driver is Windows' own, and the user-mode drivers are Intel's.
//! The guest asks for a Direct3D driver's path with adapter query type 1, OpenGL's with type 2, and
//! the other loaders ask the adapter for registry values by name. The file names are the ones
//! Intel's INF gives the card. Most are the same for every card; the OpenGL and kernel drivers
//! are not, and come with the card's generation as [`CardDrivers`]. The files have to be in the
//! guest, under the host driver store, at [`DRIVER_STORE`], the kernel driver's too. Nothing here
//! copies them.

use super::bytes::dword;
use crate::stage::{Desktop, Withheld};
use guesthouse_backend::guest_text::{self, utf16_list, utf16_zero};
use guesthouse_backend::{RegistryName, RegistryValue, Status, VendorQuery};
use guesthouse_messages::registry::{TYPE_NUMBER, TYPE_STRING, TYPE_STRING_LIST};

/// The release of Intel's driver these answers are for. The adapter record and the private data
/// change with the release, so any other is refused at start.
pub const GUEST_RELEASES: [&str; 1] = ["32.0.101.9175"];

/// Where the guest is told the driver lives. It rewrites `DriverStore` to its host driver store.
pub const DRIVER_STORE: &str = "C:\\Windows\\System32\\DriverStore\\FileRepository\\iigd_dch.inf_amd64_gpupv";

/// The user-mode drivers by the Direct3D version the guest asks for, as the INF lists them in
/// UserModeDriverName: Direct3D 9, 10, 11 and 12. 10 and 11 share a driver.
const USER_DRIVERS: [&str; 4] = ["igd9trinity64.dll", "igd10iumd64.dll", "igd10iumd64.dll", "igd12umd64.dll"];
/// The same for a 32-bit process, from UserModeDriverNameWow.
const USER_DRIVERS_32_BIT: [&str; 4] =
    ["igd9trinity32.dll", "igd10iumd32.dll", "igd10iumd32.dll", "igd12umd32.dll"];
/// Where the version sits in the query, KMTUMDVERSION: 0 for Direct3D 9 up to 3 for Direct3D 12.
const VERSION_FIELD: usize = 0;
const NEWEST: usize = USER_DRIVERS.len() - 1;

/// The drivers Intel's INF names differently from one kind of card to the next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CardDrivers {
    pub opengl: &'static str,
    pub opengl_32_bit: &'static str,
    /// Named to Windows as the adapter's kernel driver image. The guest never loads it, but Windows
    /// won't start the adapter unless the file is in the driver's folder, so the app puts it there.
    pub kernel: &'static str,
    /// Whether the INF gives these cards OpenCL for 32-bit programs, [`OPENCL_32_BIT`].
    pub opencl_32_bit: bool,
}

/// Lunar Lake's, from the INF for integrated cards.
pub const XE2_LPG_DRIVERS: CardDrivers = CardDrivers {
    opengl: "igxe2lpgicd64.dll",
    opengl_32_bit: "igxe2lpgicd32.dll",
    kernel: "igdkmdn64.sys",
    opencl_32_bit: true,
};
/// Battlemage's, from the INF for cards with memory of their own.
pub const XE2_HPG_DRIVERS: CardDrivers = CardDrivers {
    opengl: "igxe2hpgicd64.dll",
    opengl_32_bit: "igxe2hpgicd32.dll",
    kernel: "igdkmdnd64.sys",
    opencl_32_bit: true,
};
/// Panther Lake's and Wildcat Lake's, from the INF for integrated cards, which gives them OpenCL for
/// 64-bit programs only.
pub const XE3_LPG_DRIVERS: CardDrivers = CardDrivers {
    opengl: "igxe3icd64.dll",
    opengl_32_bit: "igxe3icd32.dll",
    kernel: "igdkmdn64.sys",
    opencl_32_bit: false,
};

/// Meteor Lake's and Arrow Lake's, from the INF for integrated cards.
pub const XE_LPG_DRIVERS: CardDrivers = CardDrivers {
    opengl: "igxelpgicd64.dll",
    opengl_32_bit: "igxelpgicd32.dll",
    kernel: "igdkmdn64.sys",
    opencl_32_bit: true,
};
/// Alchemist's, the Arc A-series, from the INF for cards with memory of their own.
pub const XE_HPG_DRIVERS: CardDrivers = CardDrivers {
    opengl: "igxehpgicd64.dll",
    opengl_32_bit: "igxehpgicd32.dll",
    kernel: "igdkmdnd64.sys",
    opencl_32_bit: true,
};

/// OpenCL's driver for 32-bit programs, and the value that names it, for the cards that have it.
pub const OPENCL_32_BIT: &str = "igdrcl32.dll";
const OPENCL_32_BIT_VALUE: &str = "OpenCLDriverNameWow";

/// The adapter query OpenGL's loader asks for its driver with.
pub const QUERY_OPENGL: u32 = 2;
/// The answer's path field: MAX_PATH UTF-16 units. The version and flags follow it.
const OPENGL_NAME_BYTES: usize = 520;
/// The values that name the OpenGL drivers, which the INF writes as lists of one.
const OPENGL_DRIVER_VALUE: &str = "OpenGLDriverName";
const OPENGL_DRIVER_32_BIT_VALUE: &str = "OpenGLDriverNameWow";
/// What the INF gives for the OpenGL driver's version, flags and installed marker. The 32-bit
/// driver has the same.
const OPENGL_VERSION: u32 = 1;
const OPENGL_FLAGS: u32 = 3;
const OPENGL_INSTALLED: u32 = 1;
/// The one OpenGL value that has no 32-bit twin.
const OPENGL_INSTALLED_VALUE: &str = "OpenGLInstalled";

/// A value as the INF writes it, with file names in the driver store.
enum Form {
    File(&'static str),
    Number(u32),
}

/// The values the loaders ask for, by the names the INF gives them. Registry names ignore case.
const VALUES: [(&str, Form); 8] = [
    ("OpenCLDriverName", Form::File("igdrcl64.dll")),
    ("VulkanDriverName", Form::File("igvk64.json")),
    ("VulkanDriverNameWow", Form::File("igvk32.json")),
    ("LevelZeroDriverPath", Form::File("ze_intel_gpu64.dll")),
    ("OpenGLVersion", Form::Number(OPENGL_VERSION)),
    ("OpenGLVersionWow", Form::Number(OPENGL_VERSION)),
    ("OpenGLFlags", Form::Number(OPENGL_FLAGS)),
    ("OpenGLFlagsWow", Form::Number(OPENGL_FLAGS)),
];

/// What became of a driver name query.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NameAnswer {
    Given(&'static str),
    /// Held back at this stage: see [`crate::stage`].
    Withheld(Withheld),
}

/// Writes the Direct3D driver's path into `out`, keeping the guest's version dword, unless the
/// stage withholds it from this program: see [`crate::stage`].
pub fn write_name(query: &VendorQuery<'_>, desktop: Desktop, out: &mut Vec<u8>) -> (Status, NameAnswer) {
    let version = dword(query.data, VERSION_FIELD).map_or(NEWEST, |version| (version as usize).min(NEWEST));
    if let Some(why) = desktop.withheld(query.program) {
        return (Status::NOT_SUPPORTED, NameAnswer::Withheld(why));
    }
    let drivers = if query.process_is_32_bit { USER_DRIVERS_32_BIT } else { USER_DRIVERS };
    let name = drivers[version];
    guest_text::user_driver_name(query, &format!("{DRIVER_STORE}\\{name}"), out);
    (Status::SUCCESS, NameAnswer::Given(name))
}

fn in_store(file: &str) -> String {
    format!("{DRIVER_STORE}\\{file}")
}

/// Answers OpenGL's query: the driver's path, then its version and flags. Gives the driver's name.
pub fn write_opengl_info(query: &VendorQuery<'_>, drivers: &CardDrivers, out: &mut Vec<u8>) -> &'static str {
    let driver = if query.process_is_32_bit { drivers.opengl_32_bit } else { drivers.opengl };
    let start = out.len();
    let mut path = utf16_zero(&in_store(driver));
    path.truncate(OPENGL_NAME_BYTES);
    out.extend_from_slice(&path);
    out.resize(start + OPENGL_NAME_BYTES, 0);
    out.extend_from_slice(&OPENGL_VERSION.to_le_bytes());
    out.extend_from_slice(&OPENGL_FLAGS.to_le_bytes());
    driver
}

/// The driver paths, DXCore's attributes, and the loaders' values, each of the type the INF gives it.
pub fn registry(name: &RegistryName, drivers: &CardDrivers) -> Option<RegistryValue> {
    let one_file_list = |file: &str| RegistryValue {
        value_type: Some(TYPE_STRING_LIST),
        bytes: utf16_list(&[in_store(file)]),
    };
    match name {
        RegistryName::DriverStorePath => {
            Some(RegistryValue { value_type: None, bytes: utf16_zero(DRIVER_STORE) })
        }
        RegistryName::DriverImagePath => {
            let path = format!("{DRIVER_STORE}\\{}", drivers.kernel);
            Some(RegistryValue { value_type: None, bytes: utf16_zero(&path) })
        }
        RegistryName::Value(name) if name.eq_ignore_ascii_case(guest_text::DXCORE_ATTRIBUTES_VALUE) => {
            let attributes = utf16_list(guest_text::DXCORE_GRAPHICS_ATTRIBUTES);
            Some(RegistryValue { value_type: Some(TYPE_STRING_LIST), bytes: attributes })
        }
        RegistryName::Value(name) if name.eq_ignore_ascii_case(OPENGL_INSTALLED_VALUE) => {
            Some(RegistryValue {
                value_type: Some(TYPE_NUMBER),
                bytes: OPENGL_INSTALLED.to_le_bytes().to_vec(),
            })
        }
        RegistryName::Value(name) if name.eq_ignore_ascii_case(OPENGL_DRIVER_VALUE) => {
            Some(one_file_list(drivers.opengl))
        }
        RegistryName::Value(name) if name.eq_ignore_ascii_case(OPENGL_DRIVER_32_BIT_VALUE) => {
            Some(one_file_list(drivers.opengl_32_bit))
        }
        RegistryName::Value(name) if name.eq_ignore_ascii_case(OPENCL_32_BIT_VALUE) => {
            drivers.opencl_32_bit.then(|| RegistryValue {
                value_type: Some(TYPE_STRING),
                bytes: utf16_zero(&in_store(OPENCL_32_BIT)),
            })
        }
        RegistryName::Value(name) => {
            let (_, form) = VALUES.iter().find(|(known, _)| known.eq_ignore_ascii_case(name))?;
            let (value_type, bytes) = match form {
                Form::File(file) => (TYPE_STRING, utf16_zero(&in_store(file))),
                Form::Number(number) => (TYPE_NUMBER, number.to_le_bytes().to_vec()),
            };
            Some(RegistryValue { value_type: Some(value_type), bytes })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(bytes: &[u8]) -> String {
        let units = bytes.as_chunks::<2>().0.iter().map(|unit| u16::from_le_bytes(*unit));
        String::from_utf16_lossy(&units.take_while(|&unit| unit != 0).collect::<Vec<_>>())
    }

    fn name_for(version: u8, process_is_32_bit: bool) -> (Status, NameAnswer, Vec<u8>) {
        let data = [version, 0, 0, 0, 0, 0, 0, 0];
        let query = VendorQuery {
            kind: guest_text::QUERY_USER_DRIVER_NAME,
            size: data.len(),
            data: &data,
            process_is_32_bit,
            program: "",
        };
        let mut out = Vec::new();
        let (status, answer) = write_name(&query, Desktop::Hardware, &mut out);
        (status, answer, out)
    }

    #[test]
    fn the_software_desktop_switch_holds_back_every_direct3d_version() {
        let mut out = Vec::new();
        for version in [0u8, 1, 2, 3] {
            let data = [version, 0, 0, 0];
            let query = VendorQuery {
                kind: guest_text::QUERY_USER_DRIVER_NAME,
                size: 4,
                data: &data,
                process_is_32_bit: false,
                program: "explorer.exe",
            };
            let (status, answer) = write_name(&query, Desktop::Software, &mut out);
            assert_eq!(status, Status::NOT_SUPPORTED, "{version}");
            assert_eq!(answer, NameAnswer::Withheld(Withheld::Direct3d), "{version}");
        }
        assert!(out.is_empty(), "no path is written for a driver held back");
    }

    #[test]
    fn the_compositor_is_held_back_for_its_own_reason_and_given_everything_on_the_hardware_desktop() {
        let data = [3u8, 0, 0, 0];
        let query = |program| VendorQuery {
            kind: guest_text::QUERY_USER_DRIVER_NAME,
            size: 4,
            data: &data,
            process_is_32_bit: false,
            program,
        };
        let mut out = Vec::new();
        for compositor in ["dwm.exe", "DWM.EXE"] {
            assert_eq!(
                write_name(&query(compositor), Desktop::Software, &mut out).1,
                NameAnswer::Withheld(Withheld::FromTheCompositor)
            );
        }
        for program in ["dwm.exe", "explorer.exe"] {
            assert_eq!(
                write_name(&query(program), Desktop::Hardware, &mut out).1,
                NameAnswer::Given("igd12umd64.dll")
            );
        }
    }

    #[test]
    fn each_direct3d_version_gets_the_driver_the_inf_lists_for_it() {
        for (version, file) in [(0, "igd9trinity64.dll"), (2, "igd10iumd64.dll"), (3, "igd12umd64.dll")] {
            let (status, answer, out) = name_for(version, false);
            assert_eq!((status, answer), (Status::SUCCESS, NameAnswer::Given(file)));
            assert_eq!(out[0], version, "the version dword is kept");
            assert_eq!(text(&out[4..]), format!("{DRIVER_STORE}\\{file}"));
        }
        assert_eq!(name_for(9, false).1, NameAnswer::Given("igd12umd64.dll"), "past the list is the newest");
    }

    #[test]
    fn a_32_bit_process_gets_the_32_bit_drivers_the_inf_lists() {
        let wanted =
            [(0, "igd9trinity32.dll"), (1, "igd10iumd32.dll"), (2, "igd10iumd32.dll"), (3, "igd12umd32.dll")];
        for (version, file) in wanted {
            let (status, answer, out) = name_for(version, true);
            assert_eq!((status, answer), (Status::SUCCESS, NameAnswer::Given(file)));
            assert_eq!(text(&out[4..]), format!("{DRIVER_STORE}\\{file}"));
        }
    }

    const DRIVERS: CardDrivers = XE2_LPG_DRIVERS;
    const OPENGL_DRIVER: &str = DRIVERS.opengl;
    const OPENGL_DRIVER_32_BIT: &str = DRIVERS.opengl_32_bit;

    fn opengl_answer(process_is_32_bit: bool) -> (&'static str, Vec<u8>) {
        let query = VendorQuery { kind: QUERY_OPENGL, size: 528, data: &[], process_is_32_bit, program: "" };
        let mut out = Vec::new();
        (write_opengl_info(&query, &DRIVERS, &mut out), out)
    }

    fn registry(name: &RegistryName) -> Option<RegistryValue> {
        super::registry(name, &DRIVERS)
    }

    #[test]
    fn a_card_with_memory_of_its_own_gets_its_own_opengl_and_kernel_drivers() {
        let query =
            VendorQuery { kind: QUERY_OPENGL, size: 528, data: &[], process_is_32_bit: false, program: "" };
        let mut out = Vec::new();
        assert_eq!(write_opengl_info(&query, &XE2_HPG_DRIVERS, &mut out), "igxe2hpgicd64.dll");
        assert!(text(&out).ends_with("\\igxe2hpgicd64.dll"));
        let wow = super::registry(&RegistryName::Value("OpenGLDriverNameWow".into()), &XE2_HPG_DRIVERS);
        assert_eq!(wow.unwrap().bytes, utf16_list(&[in_store("igxe2hpgicd32.dll")]));
        let image = super::registry(&RegistryName::DriverImagePath, &XE2_HPG_DRIVERS).unwrap();
        assert!(text(&image.bytes).ends_with("\\igdkmdnd64.sys"));
    }

    #[test]
    fn the_opengl_answer_is_the_path_then_the_version_and_flags() {
        let (driver, out) = opengl_answer(false);
        assert_eq!((driver, out.len()), (OPENGL_DRIVER, OPENGL_NAME_BYTES + 8));
        assert_eq!(text(&out), format!("{DRIVER_STORE}\\{OPENGL_DRIVER}"));
        assert_eq!(out[OPENGL_NAME_BYTES..], [1, 0, 0, 0, 3, 0, 0, 0]);
        let (driver, out) = opengl_answer(true);
        assert_eq!(driver, OPENGL_DRIVER_32_BIT);
        assert_eq!(text(&out), format!("{DRIVER_STORE}\\{OPENGL_DRIVER_32_BIT}"));
    }

    #[test]
    fn the_opengl_registry_values_are_the_infs_with_their_types() {
        let driver = registry(&RegistryName::Value("opengldrivernamewow".into())).unwrap();
        assert_eq!(driver.value_type, Some(TYPE_STRING_LIST));
        assert_eq!(driver.bytes, utf16_list(&[in_store(OPENGL_DRIVER_32_BIT)]));
        let numbers = [
            ("OpenGLVersion", 1u32),
            ("OpenGLVersionWow", 1),
            ("OpenGLFlags", 3),
            ("OpenGLFlagsWow", 3),
            ("OpenGLInstalled", 1),
        ];
        for (name, number) in numbers {
            let value = registry(&RegistryName::Value(name.into())).unwrap();
            assert_eq!(value.value_type, Some(TYPE_NUMBER), "{name}");
            assert_eq!(value.bytes, number.to_le_bytes(), "{name}");
        }
    }

    #[test]
    fn the_loaders_values_are_paths_in_the_store_by_any_case() {
        let value = registry(&RegistryName::Value("vulkandrivername".into())).unwrap();
        assert_eq!(value.value_type, Some(TYPE_STRING), "the INF gives a single string");
        assert_eq!(value.bytes, utf16_zero(&format!("{DRIVER_STORE}\\igvk64.json")));
        for name in ["OpenCLDriverName", "OpenCLDriverNameWow", "VulkanDriverNameWow", "LEVELZERODRIVERPATH"]
        {
            let value = registry(&RegistryName::Value(name.into())).unwrap();
            assert_eq!(value.value_type, Some(TYPE_STRING), "{name}");
        }
        assert!(registry(&RegistryName::Value("openglvendorname".into())).is_none());
    }

    #[test]
    fn a_card_without_32_bit_opencl_is_not_given_its_value() {
        let wow = RegistryName::Value("OpenCLDriverNameWow".into());
        assert!(super::registry(&wow, &XE3_LPG_DRIVERS).is_none());
        let opencl = super::registry(&RegistryName::Value("OpenCLDriverName".into()), &XE3_LPG_DRIVERS);
        assert!(opencl.is_some(), "64-bit OpenCL stays");
        let opengl = super::registry(&RegistryName::Value("OpenGLDriverName".into()), &XE3_LPG_DRIVERS);
        assert_eq!(text(&opengl.unwrap().bytes), format!("{DRIVER_STORE}\\igxe3icd64.dll"));
    }

    #[test]
    fn the_store_the_kernel_driver_and_dxcore_are_answered() {
        assert_eq!(registry(&RegistryName::DriverStorePath).unwrap().bytes, utf16_zero(DRIVER_STORE));
        let image = registry(&RegistryName::DriverImagePath).unwrap();
        assert!(text(&image.bytes).ends_with(DRIVERS.kernel));
        let dxcore = registry(&RegistryName::Value("dxcoreattributes".into())).unwrap();
        assert_eq!(dxcore.value_type, Some(TYPE_STRING_LIST));
    }
}
