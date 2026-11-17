#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libkimi as k;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    if let Some(pid) = k::spawn("/gui") {
        k::print("\n[init] launching GUI, pid ");
        k::print_u64(pid);
        k::print(" (ESC returns to this shell)\n");
        let _ = k::wait(pid);
        k::print("[init] GUI closed, terminal ready\n");
    }

    k::print("\nKimiOS user shell (libkimi), pid ");
    k::print_u64(k::getpid());
    k::print("\n");
    k::print("Commands: ls, cat, write, rm, mv, mkdir, rmdir, df, net, dns, fetch, apk, dvm, kpkg, date, stat, time, sleep, spawn, run, wait, ps, kill, mem, exec, help, exit\n");

    let mut line = [0u8; 128];
    loop {
        k::print("$ ");
        let length = k::read_line(&mut line);
        k::shell::exec(&line[..length]);
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    k::print("shell panic\n");
    k::exit(1);
}
