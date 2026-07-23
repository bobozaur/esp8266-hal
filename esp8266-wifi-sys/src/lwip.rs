//! Checked-in bindings for the small lwIP netif surface used by the Rust AP
//! bridge. Generated from the pinned SDK with rust-bindgen 0.72.1.

pub const ERR_OK: err_t = 0;
pub const ERR_ARG: err_t = -16;

pub const ERR_NEED_SCHED: u32 = 123;
pub const NETIF_DEBUG: u32 = 0;
pub const NETIF_MAX_HWADDR_LEN: u32 = 6;
pub const NETIF_NAMESIZE: u32 = 6;
pub const NETIF_FLAG_UP: u32 = 1;
pub const NETIF_FLAG_BROADCAST: u32 = 2;
pub const NETIF_FLAG_LINK_UP: u32 = 4;
pub const NETIF_FLAG_ETHARP: u32 = 8;
pub const NETIF_FLAG_ETHERNET: u32 = 16;
pub const NETIF_FLAG_IGMP: u32 = 32;
pub const NETIF_FLAG_MLD6: u32 = 64;
pub const NETIF_ADDR_IDX_MAX: u32 = 127;
pub const NETIF_NO_INDEX: u32 = 0;
pub type u8_t = u8;
pub type s8_t = i8;
pub type u16_t = u16;
pub type u32_t = u32;
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct ip4_addr {
    pub addr: u32_t,
}
pub type ip4_addr_t = ip4_addr;
pub type err_t = s8_t;
pub type ip_addr_t = ip4_addr_t;
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct pbuf {
    pub next: *mut pbuf,
    pub payload: *mut core::ffi::c_void,
    pub tot_len: u16_t,
    pub len: u16_t,
    pub type_internal: u8_t,
    pub flags: u8_t,
    pub ref_: u8_t,
    pub if_idx: u8_t,
    pub l2_owner: *mut netif,
    pub l2_buf: *mut core::ffi::c_void,
}
pub const netif_mac_filter_action_NETIF_DEL_MAC_FILTER: netif_mac_filter_action = 0;
pub const netif_mac_filter_action_NETIF_ADD_MAC_FILTER: netif_mac_filter_action = 1;
pub type netif_mac_filter_action = core::ffi::c_uint;
pub type netif_init_fn = ::core::option::Option<unsafe extern "C" fn(netif: *mut netif) -> err_t>;
pub type netif_input_fn =
    ::core::option::Option<unsafe extern "C" fn(p: *mut pbuf, inp: *mut netif) -> err_t>;
pub type netif_output_fn = ::core::option::Option<
    unsafe extern "C" fn(netif: *mut netif, p: *mut pbuf, ipaddr: *const ip4_addr_t) -> err_t,
>;
pub type netif_linkoutput_fn =
    ::core::option::Option<unsafe extern "C" fn(netif: *mut netif, p: *mut pbuf) -> err_t>;
pub type netif_igmp_mac_filter_fn = ::core::option::Option<
    unsafe extern "C" fn(
        netif: *mut netif,
        group: *const ip4_addr_t,
        action: netif_mac_filter_action,
    ) -> err_t,
>;
pub type dhcp_event_fn = ::core::option::Option<unsafe extern "C" fn()>;
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct netif {
    pub next: *mut netif,
    pub ip_addr: ip_addr_t,
    pub netmask: ip_addr_t,
    pub gw: ip_addr_t,
    pub input: netif_input_fn,
    pub output: netif_output_fn,
    pub linkoutput: netif_linkoutput_fn,
    pub state: *mut core::ffi::c_void,
    pub client_data: [*mut core::ffi::c_void; 2usize],
    pub dhcps_pcb: *mut udp_pcb,
    pub dhcp_event: dhcp_event_fn,
    pub hostname: *const core::ffi::c_char,
    pub mtu: u16_t,
    pub hwaddr: [u8_t; 6usize],
    pub hwaddr_len: u8_t,
    pub flags: u8_t,
    pub name: [core::ffi::c_char; 2usize],
    pub num: u8_t,
    pub rs_count: u8_t,
    pub igmp_mac_filter: netif_igmp_mac_filter_fn,
    pub loop_first: *mut pbuf,
    pub loop_last: *mut pbuf,
    pub l2_buffer_free_notify: ::core::option::Option<
        unsafe extern "C" fn(lwip_netif: *mut netif, user_buf: *mut core::ffi::c_void),
    >,
    pub last_ip_addr: ip_addr_t,
}
extern "C" {
    pub fn netif_init();
}

// Values emitted by vendor/patched-sources/abi_layout_probe.c against the
// pinned SDK configuration. These fail at compile time if the checked-in Rust
// declarations drift from the 32-bit ABI consumed by libesp8266_sdk.a.
#[cfg(target_arch = "xtensa")]
const _: () = {
    assert!(core::mem::size_of::<pbuf>() == 24);
    assert!(core::mem::align_of::<pbuf>() == 4);
    assert!(core::mem::offset_of!(pbuf, l2_owner) == 16);
    assert!(core::mem::offset_of!(pbuf, l2_buf) == 20);

    assert!(core::mem::size_of::<netif>() == 88);
    assert!(core::mem::align_of::<netif>() == 4);
    assert!(core::mem::offset_of!(netif, state) == 28);
    assert!(core::mem::offset_of!(netif, client_data) == 32);
    assert!(core::mem::offset_of!(netif, dhcps_pcb) == 40);
    assert!(core::mem::offset_of!(netif, mtu) == 52);
    assert!(core::mem::offset_of!(netif, hwaddr) == 54);
    assert!(core::mem::offset_of!(netif, igmp_mac_filter) == 68);
    assert!(core::mem::offset_of!(netif, loop_first) == 72);
    assert!(core::mem::offset_of!(netif, l2_buffer_free_notify) == 80);
    assert!(core::mem::offset_of!(netif, last_ip_addr) == 84);
};
extern "C" {
    pub fn netif_add_noaddr(
        netif: *mut netif,
        state: *mut core::ffi::c_void,
        init: netif_init_fn,
        input: netif_input_fn,
    ) -> *mut netif;
}
extern "C" {
    pub fn netif_add(
        netif: *mut netif,
        ipaddr: *const ip4_addr_t,
        netmask: *const ip4_addr_t,
        gw: *const ip4_addr_t,
        state: *mut core::ffi::c_void,
        init: netif_init_fn,
        input: netif_input_fn,
    ) -> *mut netif;
}
extern "C" {
    pub fn netif_set_addr(
        netif: *mut netif,
        ipaddr: *const ip4_addr_t,
        netmask: *const ip4_addr_t,
        gw: *const ip4_addr_t,
    );
}
extern "C" {
    pub fn netif_remove(netif: *mut netif);
}
extern "C" {
    pub fn netif_find(name: *const core::ffi::c_char) -> *mut netif;
}
extern "C" {
    pub fn netif_set_default(netif: *mut netif);
}
extern "C" {
    pub fn netif_set_ipaddr(netif: *mut netif, ipaddr: *const ip4_addr_t);
}
extern "C" {
    pub fn netif_set_netmask(netif: *mut netif, netmask: *const ip4_addr_t);
}
extern "C" {
    pub fn netif_set_gw(netif: *mut netif, gw: *const ip4_addr_t);
}
extern "C" {
    pub fn netif_set_up(netif: *mut netif);
}
extern "C" {
    pub fn netif_set_down(netif: *mut netif);
}
extern "C" {
    pub fn netif_set_link_up(netif: *mut netif);
}
extern "C" {
    pub fn netif_set_link_down(netif: *mut netif);
}
extern "C" {
    pub fn netif_loop_output(netif: *mut netif, p: *mut pbuf) -> err_t;
}
extern "C" {
    pub fn netif_poll(netif: *mut netif);
}
extern "C" {
    pub fn netif_input(p: *mut pbuf, inp: *mut netif) -> err_t;
}
extern "C" {
    pub fn netif_name_to_index(name: *const core::ffi::c_char) -> u8_t;
}
extern "C" {
    pub fn netif_index_to_name(idx: u8_t, name: *mut core::ffi::c_char) -> *mut core::ffi::c_char;
}
extern "C" {
    pub fn netif_get_by_index(idx: u8_t) -> *mut netif;
}
pub type tcpip_init_done_fn =
    ::core::option::Option<unsafe extern "C" fn(arg: *mut core::ffi::c_void)>;
pub type tcpip_callback_fn =
    ::core::option::Option<unsafe extern "C" fn(ctx: *mut core::ffi::c_void)>;
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct tcpip_callback_msg {
    _unused: [u8; 0],
}
extern "C" {
    pub fn tcpip_init(tcpip_init_done: tcpip_init_done_fn, arg: *mut core::ffi::c_void);
}
extern "C" {
    pub fn tcpip_inpkt(p: *mut pbuf, inp: *mut netif, input_fn: netif_input_fn) -> err_t;
}
extern "C" {
    pub fn tcpip_input(p: *mut pbuf, inp: *mut netif) -> err_t;
}
extern "C" {
    pub fn tcpip_try_callback(function: tcpip_callback_fn, ctx: *mut core::ffi::c_void) -> err_t;
}
extern "C" {
    pub fn tcpip_callback(function: tcpip_callback_fn, ctx: *mut core::ffi::c_void) -> err_t;
}
extern "C" {
    pub fn tcpip_callbackmsg_new(
        function: tcpip_callback_fn,
        ctx: *mut core::ffi::c_void,
    ) -> *mut tcpip_callback_msg;
}
extern "C" {
    pub fn tcpip_callbackmsg_delete(msg: *mut tcpip_callback_msg);
}
extern "C" {
    pub fn tcpip_callbackmsg_trycallback(msg: *mut tcpip_callback_msg) -> err_t;
}
extern "C" {
    pub fn tcpip_callbackmsg_trycallback_fromisr(msg: *mut tcpip_callback_msg) -> err_t;
}
#[repr(C, packed)]
#[derive(Debug, Copy, Clone)]
pub struct ip4_addr_packed {
    pub addr: u32_t,
}
pub type ip4_addr_p_t = ip4_addr_packed;
#[repr(C, packed)]
#[derive(Debug, Copy, Clone)]
pub struct ip4_addr_wordaligned {
    pub addrw: [u16_t; 2usize],
}
extern "C" {
    pub fn etharp_output(netif: *mut netif, q: *mut pbuf, ipaddr: *const ip4_addr_t) -> err_t;
}
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct udp_pcb {
    pub _address: u8,
}
