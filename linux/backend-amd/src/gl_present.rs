//! OpenGL in a window, presented through DXGI.
//!
//! AMD's OpenGL driver shows a frame in a window with a blt present. On a GPU-PV adapter, which
//! has no screen of its own, Windows in the guest turns that into a copy to a staging buffer, which
//! it then copies again with the CPU. The driver presents through DXGI instead only for HDR, stereo
//! or a window the size of its monitor, which takes neither copy. Its setting that says always
//! DXGI is behind Adrenalin's "OpenGL Triple Buffering", which the driver looks up through the
//! adapter as `UMD\EnableTripleBuffering`. So the daemon says 1 to that, and a window gets DXGI.
//!
//! On a Radeon 610M with driver 32.0.21044.64 in the guest, a 1280x720 window of a simple OpenGL
//! scene went from about 690 frames a second to 1137-1172, and a GPU-heavy scene from 206 to 264,
//! the same as Direct3D 12. The driver still refuses DXGI for a layered window, as on a PC.
//! 32.0.32015.12012 looks the setting up and uses it the same way, going by its code. That one
//! hasn't been run with it yet.

use guesthouse_backend::RegistryValue;
use guesthouse_backend::guest_text::utf16_zero;
use guesthouse_messages::registry::{TYPE_BINARY, TYPE_NUMBER, TYPE_STRING};

/// The daemon's switch that keeps the old route, the blt present and its copies.
pub const VARIABLE: &str = "GUESTHOUSE_AMD_GL_COPY";

/// The name the OpenGL driver looks its triple-buffering setting up by.
const NAME: &str = "UMD\\EnableTripleBuffering";

/// The value under [`NAME`], 1, in the type the guest asks for. AMD's drivers keep their settings
/// as text and read it back as a number, so text is given for a string or binary value, and the
/// number for a number. `None` for any other name or type.
pub fn registry_value(name: &str, asked_type: u32) -> Option<RegistryValue> {
    if !name.eq_ignore_ascii_case(NAME) {
        return None;
    }
    let bytes = match asked_type {
        TYPE_STRING | TYPE_BINARY => utf16_zero("1"),
        TYPE_NUMBER => 1u32.to_le_bytes().to_vec(),
        _ => return None,
    };
    Some(RegistryValue { value_type: Some(asked_type), bytes })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ONE_AS_TEXT: [u8; 4] = [b'1', 0, 0, 0];

    #[test]
    fn text_is_given_for_a_string_or_binary_value() {
        for asked in [TYPE_STRING, TYPE_BINARY] {
            let value = registry_value("umd\\enabletriplebuffering", asked).unwrap();
            assert_eq!(value, RegistryValue { value_type: Some(asked), bytes: ONE_AS_TEXT.to_vec() });
        }
    }

    #[test]
    fn the_number_is_given_for_a_number() {
        let value = registry_value("UMD\\EnableTripleBuffering", TYPE_NUMBER).unwrap();
        assert_eq!(value, RegistryValue { value_type: Some(TYPE_NUMBER), bytes: vec![1, 0, 0, 0] });
    }

    #[test]
    fn another_name_or_type_has_no_value() {
        assert!(registry_value("EnableTripleBuffering", TYPE_STRING).is_none());
        assert!(registry_value("UMD\\#692442739", TYPE_STRING).is_none());
        assert!(registry_value("UMD\\EnableTripleBuffering", 7).is_none());
    }
}
