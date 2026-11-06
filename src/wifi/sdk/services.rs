//! Small hardware services required by the native wireless ABI.
//!
//! CRC and RNG behavior follows ESP8266_RTOS_SDK's crc.c and hw_random.c,
//! Copyright 2018-2020 Espressif Systems (Shanghai) PTE LTD, Apache-2.0.
//! Its license is retained in esp8266-wifi-sys/vendor/licenses/.

#[cfg(target_arch = "xtensa")]
#[no_mangle]
pub extern "C" fn esp_random() -> u32 {
    extern "C" {
        static adc_rand_noise: u32;
    }

    // Preserve both SDK entropy inputs. Reading a different software PRNG or
    // only the register would change the native cryptographic RNG contract.
    unsafe {
        core::ptr::addr_of!(adc_rand_noise).read_volatile()
            ^ (0x3ff2_0e44 as *const u32).read_volatile()
    }
}

fn fill_random(bytes: &mut [u8], mut next_word: impl FnMut() -> u32) {
    for chunk in bytes.chunks_mut(4) {
        chunk.copy_from_slice(&next_word().to_le_bytes()[..chunk.len()]);
    }
}

#[cfg(target_arch = "xtensa")]
#[no_mangle]
pub unsafe extern "C" fn esp_fill_random(buffer: *mut core::ffi::c_void, length: usize) {
    if length == 0 {
        return;
    }
    // SAFETY: native callers supply writable byte-addressable RAM for length
    // bytes, and no mutable borrow is retained after filling the buffer.
    let bytes = unsafe { core::slice::from_raw_parts_mut(buffer.cast(), length) };
    fill_random(bytes, || esp_random());
}

#[cfg_attr(target_arch = "xtensa", no_mangle)]
pub unsafe extern "C" fn esp_crc8(data: *const u8, length: u32) -> u8 {
    // This reflected CRC is used for short EFUSE and provisioning fields. A
    // bitwise update removes the SDK's aligned flash-table access machinery.
    let mut crc = 0_u8;
    for offset in 0..length as usize {
        // SAFETY: the native ABI supplies length readable bytes. A zero-length
        // input needs no valid pointer, matching the SDK's loop.
        crc ^= unsafe { data.add(offset).read() };
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0x8c & 0_u8.wrapping_sub(crc & 1));
        }
    }
    crc
}

#[cfg(target_arch = "xtensa")]
#[no_mangle]
pub unsafe extern "C" fn uart_tx_wait_idle(uart: u8) {
    // The PHY passes UART0 and UART1 while changing peripheral clocks. This
    // ABI borrows registers; the application keeps ownership of both UARTs.
    let base = 0x6000_0000 + usize::from(uart) * 0xf00;
    let status = (base + 0x1c) as *const u32;
    let divider = (base + 0x14) as *const u32;
    unsafe {
        while status.read_volatile() & (0xff << 16) != 0 {
            core::hint::spin_loop();
        }
        // FIFO empty does not mean the last frame left the shift register.
        crate::uart::finish_frame(divider.read_volatile() & 0x000f_ffff);
    }
}

// libcore's delay routine is the only native importer of libhal's counter
// object. Compare-register helpers were linked only because they shared it.
#[cfg(target_arch = "xtensa")]
#[no_mangle]
pub extern "C" fn xthal_get_ccount() -> u32 {
    let count;
    unsafe {
        core::arch::asm!("rsr {0}, ccount", out(reg) count, options(nomem, nostack));
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_crc8_matches_maxim_vectors() {
        assert_eq!(unsafe { esp_crc8(core::ptr::null(), 0) }, 0);
        for (bytes, expected) in [
            (&b"123456789"[..], 0xa1),
            (&[1][..], 0x5e),
            (&[255][..], 0x35),
        ] {
            assert_eq!(
                unsafe { esp_crc8(bytes.as_ptr(), bytes.len() as u32) },
                expected
            );
        }
    }

    #[test]
    fn random_fill_uses_fresh_words_and_preserves_partial_word_bounds() {
        for length in 0..=9 {
            let mut buffer = [0xa5_u8; 11];
            let mut calls = 0;
            fill_random(&mut buffer[1..1 + length], || {
                let word = 0x0302_0100 + calls * 0x0404_0404;
                calls += 1;
                word
            });
            assert_eq!(calls, length.div_ceil(4) as u32);
            for (index, byte) in buffer[1..1 + length].iter().enumerate() {
                assert_eq!(*byte, index as u8);
            }
            assert_eq!(buffer[0], 0xa5);
            assert!(buffer[1 + length..].iter().all(|byte| *byte == 0xa5));
        }
    }
}
