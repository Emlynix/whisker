//! Battery of Bluetooth LE mice as Windows caches it from the standard
//! GATT Battery Service. Works for any BLE mouse, no driver needed.
//! The address comes from the HID path (see `hid_link`):
//!
//! ```text
//! devnode   BTHLE\DEV_B66B18D71E00\…  property {104EA319-…} 2 = battery %
//! ```
//! Windows refreshes the value from time to time, so it can lag a little.

use super::win::wide;
use windows_sys::Win32::Devices::DeviceAndDriverInstallation::*;
use windows_sys::Win32::Devices::Properties::DEVPROPTYPE;
use windows_sys::Win32::Foundation::{DEVPROPKEY, INVALID_HANDLE_VALUE};
use windows_sys::core::GUID;

const ENUMERATOR: &str = "BTHLE";
const DEVICE_TAG: &str = "BTHLE\\DEV_";
const ID_CHARS: usize = 512;

/// Bluetooth device property set with the battery level (pid 2).
const BATTERY_KEY: DEVPROPKEY = DEVPROPKEY {
    fmtid: GUID::from_u128(0x104ea319_6ee2_4701_bd47_8ddbf425bbe5),
    pid: 2,
};

/// DEVPKEY_Device_FriendlyName: the name shown in Bluetooth settings.
const NAME_KEY: DEVPROPKEY = DEVPROPKEY {
    fmtid: GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0),
    pid: 14,
};
const NAME_BYTES: usize = 256;

/// Battery % Windows last read from the device with this address.
pub fn battery(address: &str) -> Option<u8> {
    let mut value = [0u8; 1];
    read_property(address, &BATTERY_KEY, &mut value).map(|_| value[0].min(100))
}

/// Name of the device in Bluetooth settings, e.g. "AET-MS480BBT2".
pub fn name(address: &str) -> Option<String> {
    let mut buf = [0u8; NAME_BYTES];
    let n = read_property(address, &NAME_KEY, &mut buf)?;
    let units: Vec<u16> = buf[..n].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&u| u != 0).collect();
    Some(String::from_utf16_lossy(&units)).filter(|s| !s.is_empty())
}

/// Reads one property of the BTHLE devnode with this address into `out`;
/// returns the number of bytes written.
fn read_property(address: &str, key: &DEVPROPKEY, out: &mut [u8]) -> Option<usize> {
    let enumerator = wide(ENUMERATOR);
    let want = format!("{DEVICE_TAG}{}", address.to_uppercase());

    unsafe {
        let set = SetupDiGetClassDevsW(std::ptr::null(), enumerator.as_ptr(), std::ptr::null_mut(), DIGCF_PRESENT | DIGCF_ALLCLASSES);
        if set == INVALID_HANDLE_VALUE as isize {
            return None;
        }

        let mut found = None;
        for i in 0.. {
            let mut dev: SP_DEVINFO_DATA = std::mem::zeroed();
            dev.cbSize = size_of::<SP_DEVINFO_DATA>() as u32;
            if SetupDiEnumDeviceInfo(set, i, &mut dev) == 0 {
                break;
            }

            let mut id = [0u16; ID_CHARS];
            if SetupDiGetDeviceInstanceIdW(set, &dev, id.as_mut_ptr(), ID_CHARS as u32, std::ptr::null_mut()) == 0 {
                continue;
            }
            let len = id.iter().position(|&c| c == 0).unwrap_or(ID_CHARS);
            if !String::from_utf16_lossy(&id[..len]).to_uppercase().starts_with(&want) {
                continue;
            }

            let mut kind: DEVPROPTYPE = 0;
            let mut size = 0u32;
            let ok = SetupDiGetDevicePropertyW(set, &dev, key, &mut kind, out.as_mut_ptr(), out.len() as u32, &mut size, 0);
            if ok != 0 {
                found = Some(size as usize);
                break;
            }
        }
        SetupDiDestroyDeviceInfoList(set);
        found
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hardware probe: `cargo test -- --ignored --nocapture bt_battery`.
    #[test]
    #[ignore]
    fn bt_battery() {
        println!("battery = {:?} name = {:?}", battery("B66B18D71E00"), name("B66B18D71E00"));
    }
}
