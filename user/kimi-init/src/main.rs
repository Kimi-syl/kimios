#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libkimi as k;

fn trim(input: &[u8]) -> &[u8] {
    let mut start = 0;
    let mut end = input.len();

    while start < end && (input[start] == b' ' || input[start] == b'\t') {
        start += 1;
    }
    while end > start
        && (input[end - 1] == b' ' || input[end - 1] == b'\t' || input[end - 1] == b'\r')
    {
        end -= 1;
    }

    &input[start..end]
}

fn split_word(input: &[u8]) -> (&[u8], &[u8]) {
    let mut index = 0;
    while index < input.len() && input[index] != b' ' && input[index] != b'\t' {
        index += 1;
    }
    let word = &input[..index];

    let mut rest = index;
    while rest < input.len() && (input[rest] == b' ' || input[rest] == b'\t') {
        rest += 1;
    }

    (word, &input[rest..])
}

/// Accept both `hello` and `/hello`.
fn with_slash<'a>(argument: &[u8], buffer: &'a mut [u8]) -> Option<&'a str> {
    if argument.is_empty() {
        return None;
    }

    let needs_slash = argument[0] != b'/';
    let length = argument.len() + needs_slash as usize;
    if length > buffer.len() {
        return None;
    }

    let mut offset = 0;
    if needs_slash {
        buffer[0] = b'/';
        offset = 1;
    }
    buffer[offset..offset + argument.len()].copy_from_slice(argument);

    core::str::from_utf8(&buffer[..length]).ok()
}

fn parse_u64(input: &[u8]) -> Option<u64> {
    if input.is_empty() {
        return None;
    }
    let mut value = 0u64;
    for &byte in input {
        if !byte.is_ascii_digit() {
            return None;
        }
        value = value.checked_mul(10)?.checked_add((byte - b'0') as u64)?;
    }
    Some(value)
}

fn report_status(pid: u64, status: Option<u64>) {
    match status {
        Some(code) => {
            if pid == 0 {
                k::print("child exited with status ");
            } else {
                k::print("pid ");
                k::print_u64(pid);
                k::print(" exited with status ");
            }
            k::print_u64(code);
            k::print("\n");
        }
        None => k::print("wait failed\n"),
    }
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    k::print("\nKimiOS user shell (libkimi), pid ");
    k::print_u64(k::getpid());
    k::print("\n");
    k::print("Commands: ls, cat, write, rm, df, time, sleep, spawn, run, wait, ps, kill, mem, exec, help, exit\n");

    let mut line = [0u8; 128];
    let mut path_buffer = [0u8; 96];
    let mut file_buffer = [0u8; 512];
    let mut listing = [0u8; 256];

    loop {
        k::print("$ ");
        let length = k::read_line(&mut line);
        let input = trim(&line[..length]);
        if input.is_empty() {
            continue;
        }

        let (command, rest) = split_word(input);
        let argument = trim(rest);

        if command == &b"help"[..] {
            k::print("  ls             list RamFS files\n");
            k::print("  cat <file>     print a file (RamFS or /disk/...)\n");
            k::print("  write <path> <text>  create/truncate a /disk file\n");
            k::print("  rm <file>      remove a /disk file\n");
            k::print("  df             show disk geometry, files and free blocks\n");
            k::print("  time           show milliseconds since boot\n");
            k::print("  sleep <ms>     sleep while other processes keep running\n");
            k::print("  spawn <file>   create a process from an ELF or PE\n");
            k::print("  run <file>     spawn and wait for a process\n");
            k::print("  wait <pid>     wait for a child (0 = any)\n");
            k::print("  mem            report free physical frames\n");
            k::print("  exec <file>    replace the shell with an ELF or PE\n");
            k::print("  exit           halt the machine\n");
        } else if command == &b"ls"[..] {
            let count = k::list(&mut listing);
            for name in listing[..count].split(|&byte| byte == 0) {
                if !name.is_empty() {
                    k::print_bytes(name);
                    k::print("\n");
                }
            }
        } else if command == &b"cat"[..] {
            let Some(path) = with_slash(argument, &mut path_buffer) else {
                k::print("usage: cat <file>\n");
                continue;
            };

            let Some(handle) = k::open(path) else {
                k::print("cat: file not found\n");
                continue;
            };

            let mut offset = 0;
            loop {
                let count = k::read_file(handle, offset, &mut file_buffer);
                if count == 0 {
                    break;
                }
                k::print_bytes(&file_buffer[..count]);
                offset += count;
            }
            k::close(handle);
        } else if command == &b"write"[..] {
            let (path_word, text) = split_word(argument);
            let Some(path) = with_slash(path_word, &mut path_buffer) else {
                k::print("usage: write /disk/<file> <text>\n");
                continue;
            };
            let Some(handle) = k::create(path) else {
                k::print("write: no disk or bad path\n");
                continue;
            };
            let count = k::write_file(handle, 0, text);
            if count != text.len() {
                k::print("write: short write\n");
            }
            if k::write_file(handle, text.len(), b"\n") != 1 {
                k::print("write: short write\n");
            }
            k::close(handle);
        } else if command == &b"df"[..] {
            match k::diskinfo() {
                Some(info) => {
                    let total = info >> 32;
                    let files = info & 0xffff_ffff;
                    k::print("disk sectors: ");
                    k::print_u64(total);
                    k::print(", files: ");
                    k::print_u64(files);
                    k::print("\n");
                }
                None => k::print("df: no disk mounted\n"),
            }
            if let Some(free) = k::free_blocks() {
                k::print("free data blocks: ");
                k::print_u64(free);
                k::print("\n");
            }
        } else if command == &b"rm"[..] {
            let Some(path) = with_slash(argument, &mut path_buffer) else {
                k::print("usage: rm <file>\n");
                continue;
            };
            if k::unlink(path) {
                k::print("removed ");
                k::print(path);
                k::print("\n");
            } else {
                k::print("rm: failed (disk files only)\n");
            }
        } else if command == &b"time"[..] {
            k::print("uptime ms: ");
            k::print_u64(k::uptime_ms());
            k::print("\n");
        } else if command == &b"sleep"[..] {
            let Some(ms) = parse_u64(argument) else {
                k::print("usage: sleep <ms>\n");
                continue;
            };
            let before = k::uptime_ms();
            k::sleep_ms(ms);
            let after = k::uptime_ms();
            k::print("slept ");
            k::print_u64(after.saturating_sub(before));
            k::print(" ms\n");
        } else if command == &b"spawn"[..] {
            let Some(path) = with_slash(argument, &mut path_buffer) else {
                k::print("usage: spawn <file>\n");
                continue;
            };

            match k::spawn(path) {
                Some(pid) => {
                    k::print("spawned pid ");
                    k::print_u64(pid);
                    k::print("\n");
                }
                None => k::print("spawn failed\n"),
            }
        } else if command == &b"mem"[..] {
            k::print("free frames: ");
            k::print_u64(k::free_frames());
            k::print("\n");
        } else if command == &b"wait"[..] {
            let Some(pid) = parse_u64(argument) else {
                k::print("usage: wait <pid>\n");
                continue;
            };
            report_status(pid, k::wait(pid));
        } else if command == &b"run"[..] {
            let Some(path) = with_slash(argument, &mut path_buffer) else {
                k::print("usage: run <file>\n");
                continue;
            };

            match k::spawn(path) {
                Some(pid) => {
                    k::print("spawned pid ");
                    k::print_u64(pid);
                    k::print("\n");
                    report_status(pid, k::wait(pid));
                }
                None => k::print("spawn failed\n"),
            }
        } else if command == &b"exec"[..] {
            let Some(path) = with_slash(argument, &mut path_buffer) else {
                k::print("usage: exec <file>\n");
                continue;
            };

            k::print("exec: loading ");
            k::print(path);
            k::print("\n");
            k::exec(path);
            k::print("exec failed\n");
        } else if command == &b"ps"[..] {
            let mut rows = [0u64; 64];
            let count = k::ps(&mut rows);
            k::print("PID PPID STATE EXIT\n");
            for index in 0..count {
                let base = index * 4;
                k::print_u64(rows[base]);
                k::print(" ");
                k::print_u64(rows[base + 1]);
                k::print(" ");
                k::print(match rows[base + 2] {
                    0 => "free ",
                    1 => "ready",
                    2 => "run  ",
                    3 => "block",
                    _ => "zombi",
                });
                k::print(" ");
                k::print_u64(rows[base + 3]);
                k::print("\n");
            }
        } else if command == &b"kill"[..] {
            let Some(pid) = parse_u64(argument) else {
                k::print("usage: kill <pid>\n");
                continue;
            };
            if k::kill(pid) {
                k::print("killed pid ");
                k::print_u64(pid);
                k::print("\n");
            } else {
                k::print("kill: no such live process\n");
            }
        } else if command == &b"exit"[..] {
            k::print("bye\n");
            k::exit(0);
        } else {
            k::print("unknown command: ");
            k::print_bytes(command);
            k::print("\n");
        }
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    k::print("shell panic\n");
    k::exit(1);
}
