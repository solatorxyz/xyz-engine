#![no_std]
#[cfg(target_os = "solana")]
#[panic_handler]
fn ph(_: &core::panic::PanicInfo) -> ! { loop {} }
#[no_mangle]
pub unsafe extern "C" fn entrypoint(_: *mut u8) -> u64 { 0 }
