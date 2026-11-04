use esp8266_hal::wifi::{ScanOptions, ScanTimings};
#[test]
fn scan_options_preserve_active_timing_bounds() {
    let options = ScanOptions::all().timings(ScanTimings::active(200, 500));

    assert_eq!(options.scan_timings().active_min_ms(), 200);
    assert_eq!(options.scan_timings().active_max_ms(), 500);
}
