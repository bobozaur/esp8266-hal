//! Rust entry point and hardware initialization.
//!
//! `rust_main_entry()` is called from `call_start_cpu()` in `startup.c` after
//! BSS clear, heap init, and newlib init.
//!
//! # Architecture (Embassy-first)
//!
//! Unlike a traditional RTOS design where the scheduler owns the main loop,
//! here the user's `async fn main()` owns the thread of execution via an
//! embassy executor. The FreeRTOS compat scheduler runs in the background:
//!
//! 1. `rust_main_entry()` performs hardware init directly (no FreeRTOS task)
//! 2. `init_compat_scheduler()` starts the compat scheduler (idle + timer
//!    daemon tasks, ISRs) — but returns control to the caller
//! 3. The user's entry point runs an embassy executor on the main thread
//! 4. WiFi blob tasks are created dynamically and run via ISR-driven
//!    context switches (PendSV / tick ISR)

use core::ffi::c_void;

use super::port;

// ---------------------------------------------------------------------------
// Newlib reent initialization
//
// The WiFi blob calls C library functions (calloc, printf, etc.) that use
// __getreent() → _global_impure_ptr. Without initialization, this pointer
// is NULL and the blob crashes/hangs on the first C library call.
// ---------------------------------------------------------------------------

extern "C" {
    static mut _global_impure_ptr: *mut u8;
}

/// Minimal newlib reent struct. `struct _reent` is 240 bytes on ESP8266's
/// newlib. Key fields:
///   offset 0x04: _stdin  (FILE*)
///   offset 0x08: _stdout (FILE*)
///   offset 0x0C: _stderr (FILE*)
///   offset 0x18: __sdidinit (int, must be 1)
///
/// The blob writes 32-bit pointers and ints into this storage. Keep it at
/// least 4-byte aligned so those raw stores cannot fault on layouts where a
/// plain `[u8; 240]` would land at an odd address.
#[repr(C, align(4))]
struct ReentStorage([u8; 240]);

static mut REENT_STRUCT: ReentStorage = ReentStorage([0; 240]);

/// Initialize `_global_impure_ptr` so that `__getreent()` returns a valid
/// (if minimal) reent struct. Must be called before any WiFi blob function.
pub fn init_newlib_reent() {
    unsafe {
        extern "C" {
            static __sf_fake_stdin: u8;
            static __sf_fake_stdout: u8;
            static __sf_fake_stderr: u8;
        }

        REENT_STRUCT = ReentStorage([0; 240]);
        let base = REENT_STRUCT.0.as_mut_ptr();

        // _stdin at offset 0x04
        *(base.add(0x04) as *mut *const u8) = &__sf_fake_stdin;
        // _stdout at offset 0x08
        *(base.add(0x08) as *mut *const u8) = &__sf_fake_stdout;
        // _stderr at offset 0x0C
        *(base.add(0x0C) as *mut *const u8) = &__sf_fake_stderr;
        // __sdidinit at offset 0x18 — must be 1
        *(base.add(0x18) as *mut i32) = 1;

        _global_impure_ptr = base;
    }
}

// ---------------------------------------------------------------------------
// FFI declarations for SDK init functions
// ---------------------------------------------------------------------------

extern "C" {
    fn esp_phy_init_clk();
    fn base_gpio_init() -> i32;
    fn esp_mac_init() -> i32;
    fn esp_reset_reason_early() -> u32;
    #[cfg(feature = "wifi-debug-log")]
    fn esp_reset_reason() -> u32;
    fn esp_reset_reason_init();
    fn esp_task_wdt_init() -> i32;
    fn esp_pthread_init() -> i32;
    fn __init_array_start();
    fn __init_array_end();
    // C BSS boundaries (from SDK linker symbols, provided by our link.x)
    static mut _bss_start: u8;
    static mut _bss_end: u8;
}

/// `ESP_RST_FAST_SW` — fast software reboot (enum value 11).
const ESP_RST_FAST_SW: u32 = 11;

/// `ESP_OK` — success return code.
const ESP_OK: i32 = 0;

// ---------------------------------------------------------------------------
// Hardware initialization (previously inside main_task_entry)
// ---------------------------------------------------------------------------

/// Perform all hardware initialization.
///
/// This was previously done inside a FreeRTOS task (`main_task_entry`), but
/// none of it actually requires a task context — it's all register writes
/// and ROM calls that work fine before the scheduler starts.
///
/// # Safety
///
/// Must be called exactly once, very early in boot, before any WiFi or
/// scheduler operations.
pub unsafe fn hardware_init() {
    unsafe {
        let reset_reason_early = esp_reset_reason_early();

        crate::logger::trace_byte(b'A');
        // ---- C++ constructors (.init_array / .ctors) ----
        let mut func = __init_array_start as *const unsafe extern "C" fn();
        let end = __init_array_end as *const unsafe extern "C" fn();
        while func < end {
            (*func)();
            func = func.add(1);
        }

        // ---- PHY clock ----
        crate::logger::trace_byte(b'B');
        esp_phy_init_clk();

        // ---- GPIO ----
        crate::logger::trace_byte(b'C');
        assert!(base_gpio_init() == 0, "base_gpio_init failed");

        // ---- MAC address (skip on fast software reboot) ----
        crate::logger::trace_byte(b'D');
        if reset_reason_early != ESP_RST_FAST_SW {
            assert!(esp_mac_init() == ESP_OK, "esp_mac_init failed");
        }

        // ---- Reset reason bookkeeping ----
        crate::logger::trace_byte(b'E');
        esp_reset_reason_init();
        crate::wifi_debugln!("rr {} {}", reset_reason_early, esp_reset_reason());

        // ---- Task watchdog ----
        crate::logger::trace_byte(b'F');
        esp_task_wdt_init();

        // ---- Pthread support ----
        crate::logger::trace_byte(b'G');
        assert!(esp_pthread_init() == 0, "esp_pthread_init failed");
        crate::logger::trace_byte(b'g');
    }
}

/// Non-panicking variant of [`hardware_init`] for use from the proc-macro
/// generated entry point. Logs errors via println instead of asserting.
///
/// # Safety
///
/// Same requirements as [`hardware_init`].
pub unsafe fn hardware_init_safe() {
    unsafe {
        let reset_reason_early = esp_reset_reason_early();

        crate::wifi_debugln!("hw[1] init_array");
        let mut func = __init_array_start as *const unsafe extern "C" fn();
        let end = __init_array_end as *const unsafe extern "C" fn();
        while func < end {
            (*func)();
            func = func.add(1);
        }

        crate::wifi_debugln!("hw[2] phy_clk");
        esp_phy_init_clk();

        crate::wifi_debugln!("hw[3] gpio");
        let _ret = base_gpio_init();
        crate::wifi_debugln!("hw[3] gpio = {}", _ret);

        crate::wifi_debugln!("hw[4] mac");
        if reset_reason_early != ESP_RST_FAST_SW {
            let _ret = esp_mac_init();
            crate::wifi_debugln!("hw[4] mac = {}", _ret);
        }

        crate::wifi_debugln!("hw[5] reset_reason");
        esp_reset_reason_init();
        crate::wifi_debugln!("rr {} {}", reset_reason_early, esp_reset_reason());

        crate::wifi_debugln!("hw[6] wdt");
        esp_task_wdt_init();

        crate::wifi_debugln!("hw[7] pthread");
        let _ret = esp_pthread_init();
        crate::wifi_debugln!("hw[7] pthread = {}", _ret);

        crate::wifi_debugln!("hw_init: done");
    }
}

// ---------------------------------------------------------------------------
// Compat scheduler initialization
// ---------------------------------------------------------------------------

/// Whether the compat scheduler has been initialized.
static mut COMPAT_INITIALIZED: bool = false;

/// Initialize the FreeRTOS compat scheduler in background mode.
///
/// This creates the idle task, timer daemon task, attaches ISR handlers,
/// and starts the tick timer — but **returns control to the caller** instead
/// of entering an infinite scheduler loop.
///
/// WiFi blob tasks created later (via `__wifi_task_create`) will run
/// preemptively via ISR-driven context switches.
///
/// This is called automatically by [`crate::wifi::controller::WifiController::init`],
/// but can also be called manually if you need the compat layer before WiFi
/// initialization.
///
/// # Safety
///
/// Must be called after `hardware_init()`. Safe to call multiple times
/// (subsequent calls are no-ops).
pub unsafe fn init_compat_scheduler() {
    unsafe {
        if COMPAT_INITIALIZED {
            return;
        }
        COMPAT_INITIALIZED = true;

        port::start_compat_scheduler();
    }
}

// ---------------------------------------------------------------------------
// SDK-aligned main task bootstrap
// ---------------------------------------------------------------------------

/// ESP8266 RTOS SDK main-task settings from `esp_task.h` / generated sdkconfig:
///
/// - `ESP_TASK_MAIN_PRIO = ESP_TASK_PRIO_MIN + 1`
/// - `ESP_TASK_MAIN_STACK = CONFIG_ESP_MAIN_TASK_STACK_SIZE + TASK_EXTRA_STACK_SIZE`
/// - our generated `sdkconfig.h` sets `CONFIG_ESP_MAIN_TASK_STACK_SIZE = 3584`
///
/// Source references:
/// - `components/esp_common/include/esp_task.h`
/// - `components/esp8266/source/startup.c`
const SDK_MAIN_TASK_STACK_BYTES: usize = 3584;
/// Rust async `uiT` needs the SDK's C main-task stack budget.
///
/// Basis:
/// - the cleaned `tcp_server` path still leaves `2732` bytes of untouched
///   `uiT` stack at the observed worst point (`step444`: `ui_size=5296`)
/// - `ppT` creation was simultaneously short by ~`1400` bytes
/// - ESP-NOW initialization uses the WiFi SDK initialization path deeply enough
///   to require near-SDK main-task stack headroom
///
/// The WiFi blob can use deep SDK init paths before the executor yields to
/// smaller feature tasks. Keep the stack aligned with the SDK main task rather
/// than trading away correctness for a small heap gain.
const RUST_ASYNC_MAIN_TASK_STACK_BYTES: usize = SDK_MAIN_TASK_STACK_BYTES;
const RUST_ASYNC_MAIN_TASK_STACK_DEPTH: u16 = RUST_ASYNC_MAIN_TASK_STACK_BYTES as u16;
const SDK_MAIN_TASK_PRIORITY: u32 = 1;

/// Run the user's main executor on a real compat task stack, matching the SDK
/// startup model more closely than executing on the ROM boot stack.
///
/// This helper:
/// 1. prepares the compat scheduler with the temporary boot-thread task
/// 2. creates a `uiT` task with SDK main-task priority and a Rust-adjusted stack
/// 3. activates the scheduler only after `uiT` already exists
/// 4. self-deletes the temporary boot-thread task so execution continues on
///    regular task stacks
///
/// It never returns.
pub unsafe fn run_sdk_main_task(entry: unsafe extern "C" fn(*mut c_void)) -> ! {
    unsafe {
        if COMPAT_INITIALIZED {
            loop {
                core::arch::asm!("waiti 0", options(nomem, nostack));
            }
        }
        COMPAT_INITIALIZED = true;

        port::prepare_scheduler_with_boot_task();

        let ret = super::task::xTaskCreate(
            entry,
            b"uiT\0".as_ptr() as *const i8,
            RUST_ASYNC_MAIN_TASK_STACK_DEPTH,
            core::ptr::null_mut(),
            SDK_MAIN_TASK_PRIORITY,
            core::ptr::null_mut(),
        );
        assert!(ret == super::task::PD_PASS, "uiT create failed");

        port::activate_prepared_scheduler_only();

        // Retire the temporary boot-thread slot, then enter the selected
        // first task directly like SDK `xPortStartScheduler()` does via
        // `vTaskSwitchContext()` + `_xt_enter_first_task()`.
        super::task::retire_current_unowned_task_for_first_switch();
        port::enter_first_prepared_task();
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Entry point called from `call_start_cpu` in startup.c.
///
/// Performs hardware initialization, then calls the user's `main()` function
/// directly — no FreeRTOS task wrapping. The user is expected to set up an
/// embassy executor in `main()`.
///
/// This function never returns (the user's main should loop forever via the
/// embassy executor).
#[no_mangle]
pub unsafe extern "C" fn rust_main_entry() -> ! {
    unsafe {
        crate::logger::trace_byte(b'R');

        // Hardware init (previously done inside a FreeRTOS main task).
        crate::logger::trace_byte(b'H');
        hardware_init();
        crate::logger::trace_byte(b'h');

        // User entry point — expected to set up embassy executor and never return.
        extern "C" {
            fn main() -> !;
        }
        crate::logger::trace_byte(b'M');
        main();
    }
}

// ---------------------------------------------------------------------------
// Entry point macro
// ---------------------------------------------------------------------------

/// Macro to define the ESP8266 entry point.
///
/// **Prefer [`#[esp8266_wifi::main]`](crate::wifi::main)** for typical async
/// applications — it handles executor setup automatically. Use this macro
/// only when you need manual control over the executor (custom configuration,
/// multiple executors, etc.).
///
/// Generates the `extern "C" fn main()` that the compat entry point calls.
/// The body should be an expression that never returns (typically an embassy
/// executor `.run()` call).
///
/// # Example
///
/// ```rust,no_run
/// use embassy_executor::Executor;
/// use static_cell::StaticCell;
///
/// esp8266_wifi::esp8266_entry! {
///     static EXECUTOR: StaticCell<Executor> = StaticCell::new();
///     let executor = EXECUTOR.init(Executor::new());
///     executor.run(|spawner| {
///         spawner.must_spawn(my_main_task(spawner));
///     })
/// }
/// ```
#[macro_export]
macro_rules! esp8266_entry {
    ($($tt:tt)*) => {
        #[no_mangle]
        pub extern "C" fn main() -> ! {
            $($tt)*
        }
    };
}
