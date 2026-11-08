//! IOKit registry helpers shared by `hid`, `input` and `bluetooth`:
//! iterate services of a class and read their properties as Rust values.
//! Reading the registry needs no permission and sends nothing to devices.

use objc2_core_foundation::{CFDictionary, CFMutableDictionary, CFNumber, CFRetained, CFString, CFType};
use objc2_io_kit::{
    IOIteratorNext, IOObjectRelease, IORegistryEntryCreateCFProperty, IORegistryEntryGetRegistryEntryID, IORegistryEntryIDMatching,
    IORegistryEntrySearchCFProperty, IOServiceGetMatchingService, IOServiceGetMatchingServices, IOServiceMatching, io_iterator_t, io_service_t,
    kIOMainPortDefault, kIORegistryIterateParents, kIORegistryIterateRecursively, kIOServicePlane,
};
use std::ffi::CStr;

/// `KERN_SUCCESS`.
const OK: i32 = 0;

fn upcast(d: CFRetained<CFMutableDictionary>) -> CFRetained<CFDictionary> {
    // SAFETY: a mutable dictionary is a dictionary.
    unsafe { CFRetained::cast_unchecked(d) }
}

/// Owned registry object, released on drop.
pub struct Service(pub io_service_t);

impl Drop for Service {
    fn drop(&mut self) {
        IOObjectRelease(self.0);
    }
}

impl Service {
    /// The registered service with this registry entry id.
    pub fn by_id(id: u64) -> Option<Service> {
        // SAFETY: the matching dictionary is consumed by the call.
        let s = unsafe { IOServiceGetMatchingService(kIOMainPortDefault, IORegistryEntryIDMatching(id).map(upcast)) };
        (s != 0).then_some(Service(s))
    }

    /// Stable id of the entry while it is registered.
    pub fn id(&self) -> Option<u64> {
        let mut id = 0u64;
        // SAFETY: `id` is a valid out pointer.
        let r = unsafe { IORegistryEntryGetRegistryEntryID(self.0, &mut id) };
        (r == OK).then_some(id)
    }

    pub fn property(&self, key: &str) -> Option<CFRetained<CFType>> {
        let k = CFString::from_str(key);
        // SAFETY: a valid entry and key; null allocator = default.
        unsafe { IORegistryEntryCreateCFProperty(self.0, Some(&k), None, 0) }
    }

    /// Property of this entry or, failing that, of its nearest ancestor.
    pub fn inherited(&self, key: &str) -> Option<CFRetained<CFType>> {
        let k = CFString::from_str(key);
        let plane = kIOServicePlane.as_ptr() as *mut _;
        // SAFETY: a valid entry, plane name and key.
        unsafe { IORegistryEntrySearchCFProperty(self.0, plane, Some(&k), None, kIORegistryIterateRecursively | kIORegistryIterateParents) }
    }

    pub fn number(&self, key: &str) -> Option<i64> {
        let v = self.property(key)?;
        as_number(&v)
    }

    pub fn string(&self, key: &str) -> Option<String> {
        let v = self.property(key)?;
        as_string(&v)
    }
}

pub fn as_number(v: &CFType) -> Option<i64> {
    v.downcast_ref::<CFNumber>()?.as_i64()
}

pub fn as_string(v: &CFType) -> Option<String> {
    Some(v.downcast_ref::<CFString>()?.to_string())
}

/// All registered services of an IOKit class, e.g. `c"IOHIDDevice"`.
pub fn services(class: &CStr) -> Vec<Service> {
    let mut out = Vec::new();
    let mut it: io_iterator_t = 0;
    // SAFETY: the matching dictionary is consumed; `it` is a valid out pointer.
    let r = unsafe { IOServiceGetMatchingServices(kIOMainPortDefault, IOServiceMatching(class.as_ptr()).map(upcast), &mut it) };
    if r != OK || it == 0 {
        return out;
    }
    loop {
        let s = IOIteratorNext(it);
        if s == 0 {
            break;
        }
        out.push(Service(s));
    }
    IOObjectRelease(it);
    out
}
