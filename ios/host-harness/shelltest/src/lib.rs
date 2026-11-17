#![no_std]

//! Native library-OS shell test: drives the real libkimi command interpreter
//! through the host syscall dispatcher to verify the FS/net contract end-to-end.

use libkimi as k;

#[no_mangle]
pub extern "C" fn kimi_shell_test() {
    k::print("\n=== KimiOS libOS shell test ===\n");
    k::shell::exec(b"uname");
    k::shell::exec(b"write /disk/hello.txt Hello from KimiOS libOS");
    k::shell::exec(b"cat /disk/hello.txt");
    k::shell::exec(b"stat /disk/hello.txt");
    k::shell::exec(b"mkdir /disk/sub");
    k::shell::exec(b"write /disk/sub/a.txt nested file");
    k::shell::exec(b"ls /disk");
    k::shell::exec(b"ls");
    k::shell::exec(b"cat /readme");
    k::shell::exec(b"df");
    k::shell::exec(b"mv /disk/hello.txt /disk/renamed.txt");
    k::shell::exec(b"cat /disk/renamed.txt");
    k::shell::exec(b"rm /disk/renamed.txt");
    k::shell::exec(b"rmdir /disk/sub");
    k::shell::exec(b"date");
    k::shell::exec(b"uptime");
    k::shell::exec(b"ps");
    k::shell::exec(b"dns example.com");
    k::shell::exec(b"fetch example.com /");
    k::print("=== done ===\n");
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    k::exit(99)
}
