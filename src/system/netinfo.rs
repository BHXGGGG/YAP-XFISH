//! 本机局域网 IPv4（仪表盘「内网 IP」卡片）。
//!
//! 不能用「UDP connect 外网再读本地地址」的办法：开启 TUN 后默认路由指向虚拟网卡，
//! 拿到的会是 172.19.0.1。这里枚举网卡，只取「已启用、有默认网关、非回环/隧道」的
//! 网卡上的 IPv4；TUN 网卡没有网关，自然被排除。

use std::net::Ipv4Addr;
use windows_sys::Win32::Foundation::{ERROR_BUFFER_OVERFLOW, NO_ERROR};
use windows_sys::Win32::NetworkManagement::IpHelper::{
    GetAdaptersAddresses, GAA_FLAG_INCLUDE_GATEWAYS, GAA_FLAG_SKIP_ANYCAST,
    GAA_FLAG_SKIP_DNS_SERVER, GAA_FLAG_SKIP_MULTICAST, IF_TYPE_SOFTWARE_LOOPBACK, IF_TYPE_TUNNEL,
    IP_ADAPTER_ADDRESSES_LH,
};
use windows_sys::Win32::NetworkManagement::Ndis::IfOperStatusUp;
use windows_sys::Win32::Networking::WinSock::{AF_INET, SOCKADDR_IN};

pub fn lan_ipv4() -> Option<Ipv4Addr> {
    let flags = GAA_FLAG_INCLUDE_GATEWAYS
        | GAA_FLAG_SKIP_ANYCAST
        | GAA_FLAG_SKIP_MULTICAST
        | GAA_FLAG_SKIP_DNS_SERVER;
    let mut size: u32 = 16 * 1024;
    let mut buf: Vec<u64> = Vec::new();
    for _ in 0..3 {
        // u64 对齐，满足 IP_ADAPTER_ADDRESSES_LH 的对齐要求
        buf = vec![0u64; (size as usize).div_ceil(8)];
        let ret = unsafe {
            GetAdaptersAddresses(
                AF_INET as u32,
                flags,
                std::ptr::null(),
                buf.as_mut_ptr() as *mut IP_ADAPTER_ADDRESSES_LH,
                &mut size,
            )
        };
        match ret {
            NO_ERROR => break,
            ERROR_BUFFER_OVERFLOW => continue,
            _ => return None,
        }
    }

    let mut best: Option<Ipv4Addr> = None;
    let mut cur = buf.as_ptr() as *const IP_ADAPTER_ADDRESSES_LH;
    unsafe {
        while !cur.is_null() {
            let a = &*cur;
            // wintun（sing-box 的 tun0）在系统里是普通「已启用 + 有网关」的网卡，
            // 只能按描述 / 名称排除，否则会优先返回 172.19.0.1。
            let usable = a.OperStatus == IfOperStatusUp
                && a.IfType != IF_TYPE_SOFTWARE_LOOPBACK
                && a.IfType != IF_TYPE_TUNNEL
                && !a.FirstGatewayAddress.is_null()
                && !is_tun_adapter(a);
            if usable {
                let mut u = a.FirstUnicastAddress;
                while !u.is_null() {
                    let sa = (*u).Address.lpSockaddr;
                    if !sa.is_null() && (*sa).sa_family == AF_INET {
                        let sin = &*(sa as *const SOCKADDR_IN);
                        let ip = Ipv4Addr::from(u32::from_be(sin.sin_addr.S_un.S_addr));
                        if !ip.is_loopback() && !ip.is_link_local() {
                            // 优先私有地址（家庭 / 公司局域网）
                            if ip.is_private() {
                                return Some(ip);
                            }
                            best.get_or_insert(ip);
                        }
                    }
                    u = (*u).Next;
                }
            }
            cur = a.Next;
        }
    }
    best
}

/// tun0 的描述是 "sing-tun Tunnel"。按描述 / 友好名判断，避免依赖网卡的
/// IfType 或 172.19.0.x 地址（后者是可配置的）。
unsafe fn is_tun_adapter(a: &IP_ADAPTER_ADDRESSES_LH) -> bool {
    let desc = wide_lossy(a.Description);
    let name = wide_lossy(a.FriendlyName);
    [desc, name].iter().any(|s| {
        let s = s.to_ascii_lowercase();
        s.contains("sing-tun") || s.contains("wintun")
    })
}

unsafe fn wide_lossy(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    let mut len = 0usize;
    while *p.add(len) != 0 {
        len += 1;
    }
    String::from_utf16_lossy(std::slice::from_raw_parts(p, len))
}

#[cfg(test)]
mod tests {
    /// 本机验证：`cargo test lan_ipv4_prints -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn lan_ipv4_prints() {
        println!("lan_ipv4 = {:?}", super::lan_ipv4());
    }
}
