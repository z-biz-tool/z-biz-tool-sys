use serde::{Deserialize, Serialize};

/// 网卡地址信息。字段为 `None` 表示该平台/该接口无法取得，绝不以假值占位。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct InterfaceAddress {
    pub name: String,
    pub ipv4: Option<String>,
    pub ipv6: Option<String>,
    pub mac: Option<String>,
    pub up: bool,
    pub loopback: bool,
}

pub fn list_interfaces() -> Vec<InterfaceAddress> {
    #[cfg(unix)]
    {
        unix::list_interfaces()
    }
    #[cfg(not(unix))]
    {
        Vec::new()
    }
}

fn format_mac(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{:02X}", b))
        .collect::<Vec<_>>()
        .join(":")
}

#[cfg(unix)]
mod unix {
    use super::{format_mac, InterfaceAddress};
    use std::collections::BTreeMap;
    use std::ffi::CStr;
    use std::net::{Ipv4Addr, Ipv6Addr};
    use std::{ptr, slice};

    // BSD/macOS 的 sockaddr_dl：sdl_len@0, sdl_family@1, sdl_index@2..3,
    // sdl_type@4, sdl_nlen@5, sdl_alen@6, sdl_slen@7, sdl_data@8。
    #[cfg(any(target_os = "macos", target_os = "ios", target_os = "freebsd"))]
    const SDL_DATA_OFFSET: usize = 8;

    #[derive(Default)]
    struct Partial {
        ipv4: Option<String>,
        ipv6: Option<String>,
        mac: Option<String>,
        up: bool,
        loopback: bool,
    }

    pub fn list_interfaces() -> Vec<InterfaceAddress> {
        let mut table: BTreeMap<String, Partial> = BTreeMap::new();
        let mut ifap: *mut libc::ifaddrs = ptr::null_mut();

        if unsafe { libc::getifaddrs(&mut ifap) } != 0 {
            return Vec::new();
        }

        let mut cursor = ifap;
        while !cursor.is_null() {
            let ifa = unsafe { &*cursor };
            let name = if ifa.ifa_name.is_null() {
                String::new()
            } else {
                unsafe { CStr::from_ptr(ifa.ifa_name) }
                    .to_string_lossy()
                    .into_owned()
            };

            if !name.is_empty() {
                let slot = table.entry(name).or_default();
                let flags = ifa.ifa_flags;
                if flags & (libc::IFF_UP as libc::c_uint) != 0 {
                    slot.up = true;
                }
                if flags & (libc::IFF_LOOPBACK as libc::c_uint) != 0 {
                    slot.loopback = true;
                }

                if !ifa.ifa_addr.is_null() {
                    let sa = ifa.ifa_addr;
                    match unsafe { (*sa).sa_family } as libc::c_int {
                        libc::AF_INET => {
                            let sin = unsafe { &*(sa as *const libc::sockaddr_in) };
                            let octets = u32::from_be(sin.sin_addr.s_addr).to_be_bytes();
                            slot.ipv4 = Some(Ipv4Addr::from(octets).to_string());
                        }
                        libc::AF_INET6 => {
                            let sin6 = unsafe { &*(sa as *const libc::sockaddr_in6) };
                            slot.ipv6 =
                                Some(Ipv6Addr::from(sin6.sin6_addr.s6_addr).to_string());
                        }
                        family if family == link_family() => {
                            if let Some(mac) = unsafe { hardware_address(sa) } {
                                slot.mac = Some(mac);
                            }
                        }
                        _ => {}
                    }
                }
            }
            cursor = unsafe { (*cursor).ifa_next };
        }

        unsafe { libc::freeifaddrs(ifap) };

        table
            .into_iter()
            .map(|(name, p)| InterfaceAddress {
                name,
                ipv4: p.ipv4,
                ipv6: p.ipv6,
                mac: p.mac,
                up: p.up,
                loopback: p.loopback,
            })
            .collect()
    }

    #[cfg(any(target_os = "macos", target_os = "ios"))]
    fn link_family() -> libc::c_int {
        libc::AF_LINK as libc::c_int
    }

    #[cfg(not(any(target_os = "macos", target_os = "ios")))]
    fn link_family() -> libc::c_int {
        libc::AF_PACKET as libc::c_int
    }

    #[cfg(any(target_os = "macos", target_os = "ios", target_os = "freebsd"))]
    unsafe fn hardware_address(sa: *const libc::sockaddr) -> Option<String> {
        let base = sa as *const u8;
        let total = unsafe { *base } as usize;
        let nlen = unsafe { *base.add(5) } as usize;
        let alen = unsafe { *base.add(6) } as usize;
        if alen == 6 && total >= SDL_DATA_OFFSET + nlen + alen {
            let ptr = unsafe { base.add(SDL_DATA_OFFSET + nlen) };
            let bytes = unsafe { slice::from_raw_parts(ptr, alen) };
            if bytes.iter().all(|b| *b == 0) {
                None
            } else {
                Some(format_mac(bytes))
            }
        } else {
            None
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "freebsd")))]
    unsafe fn hardware_address(sa: *const libc::sockaddr) -> Option<String> {
        let ll = unsafe { &*(sa as *const libc::sockaddr_ll) };
        if ll.sll_halen as usize == 6 {
            let bytes = &ll.sll_addr[..6];
            if bytes.iter().all(|b| *b == 0) {
                None
            } else {
                Some(format_mac(bytes))
            }
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mac_formats_as_uppercase_hex_pairs() {
        assert_eq!(format_mac(&[0, 17, 255, 0x0a, 1, 2]), "00:11:FF:0A:01:02");
    }

    #[cfg(unix)]
    #[test]
    fn enumerates_loopback_with_real_addresses() {
        let all = list_interfaces();
        assert!(!all.is_empty(), "expected at least one interface");
        let lo = all.iter().find(|i| i.loopback).expect("loopback missing");
        assert_eq!(lo.ipv4.as_deref(), Some("127.0.0.1"));
        for i in &all {
            if let Some(mac) = &i.mac {
                assert_eq!(mac.len(), 17, "malformed MAC {:?}", mac);
                assert_ne!(mac, "AA:BB:CC:DD:EE:FF");
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn every_interface_has_a_name() {
        assert!(list_interfaces().iter().all(|i| !i.name.is_empty()));
    }
}
