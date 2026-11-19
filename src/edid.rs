//! EDID (Extended Display Identification Data) parser.
//! Parses the EDID 1.x data block to extract display manufacturer name
//! and product name without depending on libdisplay-info-sys
//! (which has version conflicts on Arch Linux).
//!
//! EDID 1.x is a 128-byte data structure.
//! - Bytes 8-9: Manufacturer ID (3 chars packed in 15 bits)
//! - Bytes 54-121: Four 18-byte descriptor blocks
//!   - Tag 0xFC: Display product name/serial number text

/// Parse the monitor manufacturer name from EDID bytes 8-9.
///
/// The manufacturer name is a 3-character code packed into 15 bits:
/// - Byte 8 bits 7-3: First character (5 bits, 1=A ... 26=Z)
/// - Byte 8 bits 2-0: Second character high 3 bits
/// - Byte 9 bits 7-5: Second character low 2 bits  
/// - Byte 9 bits 4-0: Third character (5 bits, 1=A ... 26=Z)
///
/// Returns `None` if any character code is 0 (invalid).
pub fn edid_manufacturer_name(edid: &[u8]) -> Option<String> {
    if edid.len() < 10 {
        return None;
    }
    let b8 = edid[8];
    let b9 = edid[9];
    let c1 = (b8 >> 2) & 0x1F;
    let c2 = ((b8 & 0x03) << 3) | ((b9 >> 5) & 0x07);
    let c3 = b9 & 0x1F;
    if c1 == 0 || c2 == 0 || c3 == 0 {
        return None;
    }
    Some(format!(
        "{}{}{}",
        (c1 + b'A' - 1) as char,
        (c2 + b'A' - 1) as char,
        (c3 + b'A' - 1) as char,
    ))
}

/// Parse the monitor display product name from EDID descriptor blocks.
///
/// EDID descriptor blocks start at offset 0x36 (54), each 18 bytes.
/// Block format:
///   - Byte 0: 0x00 (padding)
///   - Byte 1: 0x00 (reserved)
///   - Byte 2: Descriptor tag (0xFC = display product name)
///   - Byte 3: 0x00 (reserved)
///   - Bytes 4-17: ASCII text (null-padded)
///
/// Returns `None` if no display product name descriptor is found.
pub fn edid_monitor_name(edid: &[u8]) -> Option<String> {
    if edid.len() < 126 {
        return None;
    }
    // Check descriptor blocks at offsets 0x36, 0x48, 0x5A, 0x6A
    for offset in [54, 72, 90, 106] {
        if edid[offset] == 0x00 && edid[offset + 1] == 0x00 && edid[offset + 2] == 0xFC {
            let name_bytes: Vec<u8> = edid[offset + 4..offset + 18]
                .iter()
                .filter(|&&b| b != 0x00 && b != 0x0A)
                .cloned()
                .collect();
            if !name_bytes.is_empty() {
                return String::from_utf8(name_bytes).ok();
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal valid EDID 1.3 block with the given manufacturer
    /// name bytes (8-9) and optional descriptor block.
    fn build_edid(manufacturer_bytes: [u8; 2], descriptor: Option<[u8; 18]>) -> Vec<u8> {
        let mut edid = vec![0u8; 128];
        // EDID header (8 bytes)
        edid[0..8].copy_from_slice(&[0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00]);
        // Manufacturer ID (bytes 8-9)
        edid[8] = manufacturer_bytes[0];
        edid[9] = manufacturer_bytes[1];
        // Descriptor block if provided
        if let Some(d) = descriptor {
            edid[54..72].copy_from_slice(&d);
        }
        edid
    }

    #[test]
    fn edid_manufacturer_name_decodes_correctly() {
        // "DEL" = Dell: D(4), E(5), L(12)
        // Byte 8: (4 << 2) | (5 >> 3) = 16 | 0 = 16
        // Byte 9: ((5 & 7) << 5) | 12 = (5 << 5) | 12 = 160 | 12 = 172
        let edid = build_edid([16, 172], None);
        assert_eq!(edid_manufacturer_name(&edid), Some("DEL".to_string()));
    }

    #[test]
    fn edid_manufacturer_name_aps() {
        // "ACR" = Acer: A(1), C(3), R(18)
        // Byte 8: (1 << 2) | (3 >> 3) = 4 | 0 = 4
        // Byte 9: ((3 & 7) << 5) | 18 = (3 << 5) | 18 = 96 | 18 = 114
        let edid = build_edid([4, 114], None);
        assert_eq!(edid_manufacturer_name(&edid), Some("ACR".to_string()));
    }

    #[test]
    fn edid_manufacturer_name_empty_bytes() {
        let edid = build_edid([0, 0], None);
        assert_eq!(edid_manufacturer_name(&edid), None);
    }

    #[test]
    fn edid_manufacturer_name_short_buffer() {
        let edid = vec![0u8; 9];
        assert_eq!(edid_manufacturer_name(&edid), None);
    }

    #[test]
    fn edid_monitor_name_from_descriptor() {
        // Build EDID with "TestMonitor" in descriptor block at offset 54
        let mut desc = [0u8; 18];
        desc[2] = 0xFC; // Display product name descriptor tag
        let name = b"TestMonitor";
        desc[4..4 + name.len()].copy_from_slice(name);
        let edid = build_edid([16, 172], Some(desc));
        assert_eq!(edid_monitor_name(&edid), Some("TestMonitor".to_string()));
    }

    #[test]
    fn edid_monitor_name_null_padded() {
        let mut desc = [0u8; 18];
        desc[2] = 0xFC;
        let name = b"LG";
        desc[4..4 + name.len()].copy_from_slice(name);
        // Bytes 6-17 are 0x00 (null-padded)
        let edid = build_edid([4, 114], Some(desc));
        assert_eq!(edid_monitor_name(&edid), Some("LG".to_string()));
    }

    #[test]
    fn edid_monitor_name_no_descriptor() {
        let edid = build_edid([16, 172], None);
        assert_eq!(edid_monitor_name(&edid), None);
    }

    #[test]
    fn edid_monitor_name_short_buffer() {
        let edid = vec![0u8; 50];
        assert_eq!(edid_monitor_name(&edid), None);
    }

    #[test]
    fn edid_monitor_name_skips_non_product_descriptors() {
        let mut desc = [0u8; 18];
        desc[2] = 0xFF; // Different descriptor tag (serial number)
        desc[4] = b'A';
        let edid = build_edid([16, 172], Some(desc));
        assert_eq!(edid_monitor_name(&edid), None);
    }
}
